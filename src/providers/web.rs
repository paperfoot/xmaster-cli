//! X web-session client: the endpoints x.com itself calls, authenticated with
//! the user's browser cookies. It carries the writes the self-serve API blocks
//! (replies to people who never @mentioned you, quotes, likes, follows) and the
//! ones that are free on the web (reposts, bookmarks, plain posts).
//!
//! X rotates GraphQL query IDs and feature flags every few weeks. Like x.com
//! itself, xmaster reads each operation's ID and feature list from main.js and
//! the flag values from the logged-in page, and caches them with the
//! transaction-id keys in `web_session.json`; a request that hits a stale ID
//! refreshes the cache once and retries.

use crate::config::config_dir;
use crate::context::AppContext;
use crate::errors::XmasterError;
use crate::transaction_id::{self, TxKeys};
use reqwest::{RequestBuilder, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use tokio::sync::Mutex;
use tracing::warn;

/// Public bearer token used by the X web app (same for all users, not secret).
const WEB_BEARER: &str = "AAAAAAAAAAAAAAAAAAAAANRILgAAAAAAnNwIzUejRCOuH5E6I8xnZz4puTs%3D1Zv7ttfk8LF81IUq16cHjhLTvJu4FA33AGWWjCpTnA";
const UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/146.0.0.0 Safari/537.36";
const SEC_CH_UA: &str = "\"Google Chrome\";v=\"146\", \"Chromium\";v=\"146\", \"Not_A Brand\";v=\"24\"";

/// Signing keys and query IDs are re-read from x.com after this long.
const STATE_TTL_SECS: u64 = 3600;

/// Last-known query IDs (x.com main.js, 2026-10-13). Used only when discovery
/// fails; the Article IDs live in a lazy chunk that discovery doesn't read.
const FALLBACK_IDS: &[(&str, &str)] = &[
    ("CreateTweet", "wR5Z_lEibLvkew3tF9w-Sg"),
    ("CreateNoteTweet", "H7QXYuKegEKuUJvkuW5_dg"),
    ("FavoriteTweet", "lI07N6Otwv1PhnEgXILM7A"),
    ("UnfavoriteTweet", "ZYKSe-w7KEslx3JhSIk5LA"),
    ("CreateRetweet", "mbRO74GrOvSfRcJnlMapnQ"),
    ("DeleteRetweet", "ZyZigVsNiFO6v1dEks1eWg"),
    ("CreateBookmark", "aoDbu3RHznuiSkQ9aNM67Q"),
    ("DeleteBookmark", "Wlmlj2-xzyS1GN3a6cj-mQ"),
    ("ArticleEntityDraftCreate", "g1l5N8BxGewYuCy5USe_bQ"),
    ("ArticleEntityUpdateCoverMedia", "Es8InPh7mEkK9PxclxFAVQ"),
];

/// Feature flags x.com sends with CreateTweet / CreateNoteTweet.
const TWEET_FEATURES: &[&str] = &[
    "communities_web_enable_tweet_community_results_fetch",
    "c9s_tweet_anatomy_moderator_badge_enabled",
    "responsive_web_edit_tweet_api_enabled",
    "graphql_is_translatable_rweb_tweet_is_translatable_enabled",
    "view_counts_everywhere_api_enabled",
    "longform_notetweets_consumption_enabled",
    "responsive_web_twitter_article_tweet_consumption_enabled",
    "longform_notetweets_rich_text_read_enabled",
    "longform_notetweets_inline_media_enabled",
    "articles_preview_enabled",
    "rweb_video_timestamps_enabled",
    "rweb_tipjar_consumption_enabled",
    "responsive_web_graphql_exclude_directive_enabled",
    "freedom_of_speech_not_reach_fetch_enabled",
    "standardized_nudges_misinfo",
    "tweet_with_visibility_results_prefer_gql_limited_actions_policy_enabled",
    "responsive_web_graphql_timeline_navigation_enabled",
];
const TWEET_FEATURES_OFF: &[&str] = &[
    "tweet_awards_web_tipping_enabled",
    "creator_subscriptions_quote_tweet_preview_enabled",
    "verified_phone_label_enabled",
    "responsive_web_graphql_skip_user_profile_image_extensions_enabled",
    "responsive_web_enhance_cards_enabled",
];

/// One GraphQL operation as declared in main.js.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct OpMeta {
    pub id: String,
    pub features: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct State {
    #[serde(default)]
    fetched_at: u64,
    #[serde(default)]
    main_js: String,
    #[serde(default)]
    ops: HashMap<String, OpMeta>,
    /// Feature-switch values from the logged-in page.
    #[serde(default)]
    switches: HashMap<String, bool>,
    tx: Option<TxKeys>,
}

impl State {
    fn fresh(&self) -> bool {
        self.tx.is_some() && now().saturating_sub(self.fetched_at) < STATE_TTL_SECS
    }
}

static STATE: Mutex<Option<State>> = Mutex::const_new(None);

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn state_path() -> std::path::PathBuf {
    config_dir().join("web_session.json")
}

fn web_err(code: &'static str, message: String) -> XmasterError {
    XmasterError::Api { provider: "x-web", code, message }
}

/// Every GraphQL operation (ID + feature switches) declared in an x.com bundle.
pub fn parse_ops(js: &str) -> HashMap<String, OpMeta> {
    let re = regex::Regex::new(
        r#"queryId:"([A-Za-z0-9_-]+)",operationName:"(\w+)"(?:,operationType:"\w+",metadata:\{featureSwitches:\[([^\]]*)\])?"#,
    )
    .unwrap();
    re.captures_iter(js)
        .map(|c| {
            let features = c
                .get(3)
                .map(|m| m.as_str().split(',').map(|f| f.trim_matches('"').to_string()).filter(|f| !f.is_empty()).collect())
                .unwrap_or_default();
            (c[2].to_string(), OpMeta { id: c[1].to_string(), features })
        })
        .collect()
}

/// Feature-switch values (`"name":{"value":true}`) from the logged-in page.
pub fn parse_switches(html: &str) -> HashMap<String, bool> {
    let re = regex::Regex::new(r#""([a-z0-9_]+)":\{"value":(true|false)\}"#).unwrap();
    re.captures_iter(html)
        .map(|c| (c[1].to_string(), &c[2] == "true"))
        .collect()
}

fn main_js_url(html: &str) -> Option<String> {
    regex::Regex::new(r#"https://abs\.twimg\.com/responsive-web/client-web/main\.[0-9a-f]+\.js"#)
        .unwrap()
        .find(html)
        .map(|m| m.as_str().to_string())
}

/// A response that means our query ID or signing keys went stale.
fn is_stale(status: StatusCode, body: &str) -> bool {
    status == StatusCode::NOT_FOUND
        || body.contains("Query: Unspecified")
        || body.contains("GRAPHQL_VALIDATION_FAILED")
}

fn error_code(errors: &Value) -> i64 {
    errors.pointer("/0/code").and_then(Value::as_i64).unwrap_or(0)
}

/// Turn X's `errors[]` into an error with an actionable message.
fn errors_to_err(op: &str, errors: &Value) -> XmasterError {
    let first = errors.get(0);
    let code = error_code(errors);
    let msg = first
        .and_then(|e| e.get("message"))
        .and_then(Value::as_str)
        .unwrap_or("unknown error");
    let hint = match code {
        186 => " (too long: long posts need X Premium)",
        187 => " (duplicate of a recent post)",
        226 => " (X flagged this as automated: slow down, or post this one in the app)",
        385 | 433 => " (the author limits who can reply to this post)",
        161 => " (X's daily follow limit reached: try again tomorrow)",
        108 | 50 => " (no such user)",
        64 | 326 => " (account locked or suspended: open x.com to clear it)",
        _ => "",
    };
    web_err("web_error", format!("{op} failed: {msg} [code {code}]{hint}"))
}

pub struct Web<'a> {
    ctx: &'a AppContext,
    /// (ct0, auth_token); replaced in place if they're refreshed from the browser.
    cookies: std::sync::Mutex<(String, String)>,
}

impl<'a> Web<'a> {
    pub fn new(ctx: &'a AppContext) -> Result<Self, XmasterError> {
        if !ctx.config.has_web_cookies() {
            return Err(XmasterError::AuthMissing {
                provider: "x-web",
                message: "This needs X web-session cookies. Run: xmaster config web-login".into(),
            });
        }
        let k = &ctx.config.keys;
        Ok(Self { ctx, cookies: std::sync::Mutex::new((k.web_ct0.clone(), k.web_auth_token.clone())) })
    }

    fn ct0(&self) -> String {
        self.cookies.lock().unwrap().0.clone()
    }

    fn cookie(&self) -> String {
        let c = self.cookies.lock().unwrap();
        format!("ct0={}; auth_token={}", c.0, c.1)
    }

    /// Expired session: pick up fresh cookies from the browser if it has them.
    fn refresh_cookies(&self) -> bool {
        let Ok(fresh) = crate::browser_cookies::extract() else { return false };
        let mut c = self.cookies.lock().unwrap();
        if fresh.auth_token == c.1 && fresh.ct0 == c.0 {
            return false;
        }
        let saved = crate::commands::config_cmd::set_silent("keys.web_ct0", &fresh.ct0)
            .and_then(|_| crate::commands::config_cmd::set_silent("keys.web_auth_token", &fresh.auth_token));
        if let Err(e) = saved {
            warn!("Refreshed web cookies but could not save them: {e}");
        }
        *c = (fresh.ct0, fresh.auth_token);
        true
    }

    async fn get_text(&self, url: &str) -> Result<String, XmasterError> {
        let resp = self
            .ctx
            .client
            .get(url)
            .header("user-agent", UA)
            .header("cookie", self.cookie())
            .header("accept-language", "en-US,en;q=0.9")
            .send()
            .await
            .map_err(|e| web_err("page_fetch", format!("Failed to fetch {url}: {e}")))?;
        Ok(resp.text().await.unwrap_or_default())
    }

    /// Cached session state, re-read from x.com when stale or `force`d.
    async fn state(&self, force: bool) -> Result<State, XmasterError> {
        let mut guard = STATE.lock().await;
        if guard.is_none() {
            *guard = std::fs::read(state_path())
                .ok()
                .and_then(|b| serde_json::from_slice(&b).ok());
        }
        if let Some(s) = guard.as_ref().filter(|s| !force && s.fresh()) {
            return Ok(s.clone());
        }

        let prev = guard.take().unwrap_or_default();
        let mut html = self.get_text("https://x.com/home").await?;
        if !html.contains("twitter-site-verification") && self.refresh_cookies() {
            html = self.get_text("https://x.com/home").await?;
        }
        if !html.contains("twitter-site-verification") {
            *guard = Some(prev);
            return Err(XmasterError::AuthMissing {
                provider: "x-web",
                message: "x.com did not return a logged-in page and no fresh cookies were found \
                          in your browsers. Log in to x.com in Chrome, then run: xmaster config web-login"
                    .into(),
            });
        }

        let main_js = main_js_url(&html).unwrap_or_default();
        let ops = if !main_js.is_empty() && (main_js != prev.main_js || prev.ops.is_empty()) {
            let found = parse_ops(&self.get_text(&main_js).await.unwrap_or_default());
            if found.is_empty() { prev.ops } else { found }
        } else {
            prev.ops
        };
        let ondemand = self.get_text(&transaction_id::ondemand_url(&html)?).await?;
        // Keep only the switches some operation declares (the page has ~1k)
        let used: std::collections::HashSet<&String> = ops.values().flat_map(|m| &m.features).collect();
        let switches = parse_switches(&html).into_iter().filter(|(k, _)| used.contains(k)).collect();
        let state = State {
            fetched_at: now(),
            main_js,
            ops,
            switches,
            tx: Some(transaction_id::derive(&html, &ondemand)?),
        };
        if let Ok(bytes) = serde_json::to_vec(&state) {
            let _ = std::fs::write(state_path(), bytes);
        }
        *guard = Some(state.clone());
        Ok(state)
    }

    fn query_id(&self, state: &State, op: &str) -> String {
        let keys = &self.ctx.config.keys;
        let pinned = match op {
            "CreateTweet" => keys.graphql_create_tweet_id.as_str(),
            "CreateNoteTweet" => keys.graphql_create_note_tweet_id.as_str(),
            _ => "",
        };
        if !pinned.is_empty() {
            return pinned.to_string();
        }
        state
            .ops
            .get(op)
            .map(|m| m.id.clone())
            .or_else(|| FALLBACK_IDS.iter().find(|(o, _)| *o == op).map(|(_, id)| id.to_string()))
            .unwrap_or_default()
    }

    fn headers(&self, req: RequestBuilder, tx: &str, referer: &str) -> RequestBuilder {
        req.header("authorization", format!("Bearer {WEB_BEARER}"))
            .header("x-csrf-token", self.ct0())
            .header("cookie", self.cookie())
            .header("x-twitter-active-user", "yes")
            .header("x-twitter-auth-type", "OAuth2Session")
            .header("x-twitter-client-language", "en")
            .header("x-client-transaction-id", tx)
            .header("user-agent", UA)
            .header("origin", "https://x.com")
            .header("referer", referer)
            .header("accept", "*/*")
            .header("accept-language", "en-US,en;q=0.9")
            .header("sec-ch-ua", SEC_CH_UA)
            .header("sec-ch-ua-mobile", "?0")
            .header("sec-ch-ua-platform", "\"macOS\"")
            .header("sec-fetch-dest", "empty")
            .header("sec-fetch-mode", "cors")
            .header("sec-fetch-site", "same-origin")
    }

    /// POST to an x.com web endpoint; on a stale ID/key response, refresh the
    /// session state once and retry. `body` builds the request for a query ID.
    async fn send(
        &self,
        op: &str,
        referer: &str,
        build: impl Fn(&str) -> (&'static str, String, RequestBuilder),
    ) -> Result<Value, XmasterError> {
        for attempt in 0..2 {
            let state = self.state(attempt > 0).await?;
            let (method, path, req) = build(&self.query_id(&state, op));
            let tx = state.tx.as_ref().map(|k| transaction_id::sign(k, method, &path)).unwrap_or_default();
            let resp = self.headers(req, &tx, referer).send().await?;
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();

            if attempt == 0 && is_stale(status, &body) {
                warn!("{op}: stale web query ID or keys (HTTP {status}); refreshing from x.com");
                continue;
            }
            if status == StatusCode::TOO_MANY_REQUESTS {
                return Err(XmasterError::RateLimited { provider: "x-web", reset_at: 0 });
            }
            if status == StatusCode::UNAUTHORIZED {
                return Err(XmasterError::AuthMissing {
                    provider: "x-web",
                    message: "X rejected the web session (HTTP 401). Run: xmaster config web-login".into(),
                });
            }
            let val: Value = serde_json::from_str(&body).map_err(|_| {
                web_err(
                    "web_error",
                    format!("{op} failed (HTTP {status}): {}", crate::utils::safe_truncate(&body, 300)),
                )
            })?;
            // Mutations can return `errors` alongside an HTTP 200
            if let Some(errors) = val.get("errors").filter(|e| e.as_array().is_some_and(|a| !a.is_empty())) {
                // Already liked / already reposted: the goal state holds
                if matches!(error_code(errors), 139 | 327) {
                    return Ok(val);
                }
                if val.pointer("/data").is_none_or(|d| d.is_null() || d.as_object().is_some_and(|o| o.is_empty())) {
                    return Err(errors_to_err(op, errors));
                }
            }
            if !status.is_success() {
                return Err(web_err(
                    "web_error",
                    format!("{op} failed (HTTP {status}): {}", crate::utils::safe_truncate(&body, 300)),
                ));
            }
            return Ok(val);
        }
        unreachable!("the second attempt always returns")
    }

    /// Run a GraphQL mutation. Features come from the operation's declared
    /// switches with the page's values; `fallback_features` is used only when
    /// the operation wasn't found in main.js.
    pub async fn graphql(
        &self,
        op: &str,
        variables: Value,
        fallback_features: Option<Value>,
        field_toggles: Option<Value>,
        referer: &str,
    ) -> Result<Value, XmasterError> {
        let state = self.state(false).await?;
        let features = state
            .ops
            .get(op)
            .filter(|m| !m.features.is_empty())
            .map(|m| {
                let map: serde_json::Map<String, Value> = m
                    .features
                    .iter()
                    .map(|f| (f.clone(), Value::Bool(state.switches.get(f).copied().unwrap_or(false))))
                    .collect();
                Value::Object(map)
            })
            .or(fallback_features);
        self.send(op, referer, |qid| {
            let path = format!("/i/api/graphql/{qid}/{op}");
            let mut body = json!({ "variables": variables, "queryId": qid });
            if let Some(f) = &features {
                body["features"] = f.clone();
            }
            if let Some(t) = &field_toggles {
                body["fieldToggles"] = t.clone();
            }
            let req = self.ctx.client.post(format!("https://x.com{path}")).json(&body);
            ("POST", path, req)
        })
        .await
    }

    /// POST a form to a v1.1 REST endpoint under x.com/i/api (follows).
    async fn rest_form(&self, op: &str, path: &'static str, form: &[(&str, &str)]) -> Result<Value, XmasterError> {
        self.send(op, "https://x.com/", |_| {
            let req = self.ctx.client.post(format!("https://x.com{path}")).form(form);
            ("POST", path.to_string(), req)
        })
        .await
    }

    /// Read-only end-to-end check of the session: cookies, query IDs,
    /// feature flags and transaction signing. Returns the logged-in handle.
    pub async fn probe(&self, handle: &str) -> Result<String, XmasterError> {
        let op = "UserByScreenName";
        let state = self.state(false).await?;
        let features: serde_json::Map<String, Value> = state
            .ops
            .get(op)
            .map(|m| m.features.iter().map(|f| (f.clone(), Value::Bool(state.switches.get(f).copied().unwrap_or(false)))).collect())
            .unwrap_or_default();
        let variables = json!({ "screen_name": handle });
        let val = self
            .send(op, "https://x.com/home", |qid| {
                let path = format!("/i/api/graphql/{qid}/{op}");
                let req = self.ctx.client.get(format!("https://x.com{path}")).query(&[
                    ("variables", variables.to_string()),
                    ("features", Value::Object(features.clone()).to_string()),
                ]);
                ("GET", path, req)
            })
            .await?;
        val.pointer("/data/user/result/core/screen_name")
            .or_else(|| val.pointer("/data/user/result/legacy/screen_name"))
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| web_err("probe", format!("Unexpected response: {}", crate::utils::safe_truncate(&val.to_string(), 200))))
    }

    // -- Writes -------------------------------------------------------------

    /// Post, reply or quote. `note` selects CreateNoteTweet (Premium >280 chars).
    pub async fn create_tweet(
        &self,
        text: &str,
        reply_to: Option<&str>,
        quote_tweet_id: Option<&str>,
        media_ids: Option<&[String]>,
        note: bool,
    ) -> Result<(String, String), XmasterError> {
        let media: Vec<Value> = media_ids
            .unwrap_or_default()
            .iter()
            .map(|id| json!({ "media_id": id, "tagged_users": [] }))
            .collect();
        let mut variables = json!({
            "tweet_text": text,
            "dark_request": false,
            "media": { "media_entities": media, "possibly_sensitive": false },
            "semantic_annotation_ids": [],
        });
        if let Some(id) = reply_to {
            variables["reply"] = json!({ "in_reply_to_tweet_id": id, "exclude_reply_user_ids": [] });
        }
        if let Some(id) = quote_tweet_id {
            variables["attachment_url"] = json!(format!("https://x.com/i/status/{id}"));
        }
        if note {
            variables["richtext_options"] = json!({ "richtext_tags": [] });
        }

        let mut features = serde_json::Map::new();
        for f in TWEET_FEATURES {
            features.insert((*f).into(), Value::Bool(true));
        }
        for f in TWEET_FEATURES_OFF {
            features.insert((*f).into(), Value::Bool(false));
        }

        let op = if note { "CreateNoteTweet" } else { "CreateTweet" };
        let val = self
            .graphql(op, variables, Some(Value::Object(features)), None, "https://x.com/compose/post")
            .await?;
        let pointer = if note {
            "/data/notetweet_create/tweet_results/result"
        } else {
            "/data/create_tweet/tweet_results/result"
        };
        let result = val.pointer(pointer).ok_or_else(|| {
            web_err(
                "no_tweet_result",
                format!("Unexpected {op} response: {}", crate::utils::safe_truncate(&val.to_string(), 300)),
            )
        })?;
        // Visibility-limited results nest the post one level down
        let result = result.get("tweet").unwrap_or(result);
        let id = result
            .get("rest_id")
            .and_then(Value::as_str)
            .ok_or_else(|| web_err("no_rest_id", "No rest_id in post result".into()))?;
        let full_text = result
            .pointer("/note_tweet/note_tweet_results/result/text")
            .or_else(|| result.pointer("/legacy/full_text"))
            .and_then(Value::as_str)
            .unwrap_or(text);
        Ok((id.to_string(), full_text.to_string()))
    }

    async fn tweet_action(&self, op: &str, variables: Value) -> Result<(), XmasterError> {
        self.graphql(op, variables, None, None, "https://x.com/home").await.map(|_| ())
    }

    pub async fn like(&self, id: &str) -> Result<(), XmasterError> {
        self.tweet_action("FavoriteTweet", json!({ "tweet_id": id })).await
    }

    pub async fn unlike(&self, id: &str) -> Result<(), XmasterError> {
        self.tweet_action("UnfavoriteTweet", json!({ "tweet_id": id })).await
    }

    pub async fn retweet(&self, id: &str) -> Result<(), XmasterError> {
        self.tweet_action("CreateRetweet", json!({ "tweet_id": id, "dark_request": false })).await
    }

    pub async fn unretweet(&self, id: &str) -> Result<(), XmasterError> {
        self.tweet_action("DeleteRetweet", json!({ "source_tweet_id": id, "dark_request": false }))
            .await
    }

    pub async fn bookmark(&self, id: &str) -> Result<(), XmasterError> {
        self.tweet_action("CreateBookmark", json!({ "tweet_id": id })).await
    }

    pub async fn unbookmark(&self, id: &str) -> Result<(), XmasterError> {
        self.tweet_action("DeleteBookmark", json!({ "tweet_id": id })).await
    }

    /// Follow by handle — no paid user-ID lookup needed.
    pub async fn follow(&self, username: &str) -> Result<(), XmasterError> {
        self.rest_form(
            "Follow",
            "/i/api/1.1/friendships/create.json",
            &[("screen_name", username), ("skip_status", "1"), ("include_profile_interstitial_type", "1")],
        )
        .await
        .map(|_| ())
    }

    pub async fn unfollow(&self, username: &str) -> Result<(), XmasterError> {
        self.rest_form(
            "Unfollow",
            "/i/api/1.1/friendships/destroy.json",
            &[("screen_name", username), ("skip_status", "1"), ("include_profile_interstitial_type", "1")],
        )
        .await
        .map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ops_from_bundle() {
        let js = r#"e.exports={queryId:"wR5Z_lEibLvkew3tF9w-Sg",operationName:"CreateTweet",operationType:"mutation",metadata:{featureSwitches:["premium_content_api_read_enabled","rweb_cashtags_enabled"],fieldToggles:[]}},
                    n={queryId:"lI07N6Otwv1PhnEgXILM7A",operationName:"FavoriteTweet",operationType:"mutation",metadata:{featureSwitches:[],fieldToggles:[]}}"#;
        let ops = parse_ops(js);
        let ct = &ops["CreateTweet"];
        assert_eq!(ct.id, "wR5Z_lEibLvkew3tF9w-Sg");
        assert_eq!(ct.features, ["premium_content_api_read_enabled", "rweb_cashtags_enabled"]);
        assert_eq!(ops["FavoriteTweet"].id, "lI07N6Otwv1PhnEgXILM7A");
        assert!(ops["FavoriteTweet"].features.is_empty());
    }

    #[test]
    fn parses_switch_values_from_page() {
        let html = r#"{"premium_content_api_read_enabled":{"value":false},"responsive_web_grok_annotations_enabled":{"value":true}}"#;
        let sw = parse_switches(html);
        assert_eq!(sw.get("premium_content_api_read_enabled"), Some(&false));
        assert_eq!(sw.get("responsive_web_grok_annotations_enabled"), Some(&true));
    }

    #[test]
    fn finds_main_bundle_url() {
        let html = r#"<script src="https://abs.twimg.com/responsive-web/client-web/vendor.cdb9.js"></script>
            <script src="https://abs.twimg.com/responsive-web/client-web/main.22feefda9e8e84cca.js"></script>"#;
        assert_eq!(
            main_js_url(html).as_deref(),
            Some("https://abs.twimg.com/responsive-web/client-web/main.22feefda9e8e84cca.js")
        );
    }

    #[test]
    fn stale_detection() {
        assert!(is_stale(StatusCode::NOT_FOUND, ""));
        assert!(is_stale(StatusCode::BAD_REQUEST, r#"{"errors":[{"message":"Query: Unspecified"}]}"#));
        assert!(!is_stale(StatusCode::FORBIDDEN, r#"{"errors":[{"code":226}]}"#));
    }

    #[test]
    fn error_hints() {
        let e = errors_to_err("CreateTweet", &json!([{ "code": 226, "message": "automated" }]));
        assert!(e.to_string().contains("automated"));
        assert!(e.to_string().contains("slow down"));
    }
}
