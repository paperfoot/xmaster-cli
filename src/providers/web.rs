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
use std::time::Duration;
use tokio::sync::Mutex;
use tracing::{debug, warn};

use super::web_timeline::{self, Page};
use super::xapi::{TweetData, TweetLookup, UserResponse};

/// Public bearer token used by the X web app (same for all users, not secret).
const WEB_BEARER: &str = "AAAAAAAAAAAAAAAAAAAAANRILgAAAAAAnNwIzUejRCOuH5E6I8xnZz4puTs%3D1Zv7ttfk8LF81IUq16cHjhLTvJu4FA33AGWWjCpTnA";
const UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/146.0.0.0 Safari/537.36";
const SEC_CH_UA: &str = "\"Google Chrome\";v=\"146\", \"Chromium\";v=\"146\", \"Not_A Brand\";v=\"24\"";

/// Signing keys and query IDs are re-read from x.com after this long. A stale
/// ID or key is caught per request (refresh once, retry), so this can be long.
const STATE_TTL_SECS: u64 = 6 * 3600;

/// Scripts fetched at once while looking for the signing script.
const SCRIPT_BATCH: usize = 8;

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
    ("DeleteTweet", "nxpZCY2K-I6QoFHAHeojFQ"),
    ("UserByScreenName", "AMIBMjtxEEATh4z8V9GtRg"),
    ("TweetResultByRestId", "xZfFqfOmKQu_OXekGI59mg"),
    ("ArticleEntityDraftCreate", "g1l5N8BxGewYuCy5USe_bQ"),
    ("ArticleEntityUpdateCoverMedia", "Es8InPh7mEkK9PxclxFAVQ"),
    ("HomeLatestTimeline", "NmDBT3APtNxDpMqIEauu7A"),
    ("NotificationsTimeline", "EUX2EwvgT4f9F7sC4A59Eg"),
];

/// Read operations main.js doesn't declare, and the lazy chunk (a `~`-joined
/// chunk-name segment) that does.
const LAZY_OPS: &[(&str, &str)] = &[
    ("HomeLatestTimeline", "bundle.HomeTimeline"),
    ("NotificationsTimeline", "bundle.Notifications"),
];

/// Every timeline query declares the same feature switches; a read whose
/// operation wasn't discovered borrows them from this one.
const TIMELINE_FEATURES_OP: &str = "SearchTimeline";

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

/// Field toggles x.com sends with timeline queries (Article bodies off: the
/// timeline only needs the post).
const READ_FIELD_TOGGLES: &str = r#"{"withPayments":false,"withAuxiliaryUserLabels":false,"withArticleRichContentState":false,"withArticlePlainText":false,"withGrokAnalyze":false,"withDisallowedReplyControls":false}"#;

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
    /// The logged-in account's user ID and handle (from the page's state).
    #[serde(default)]
    viewer_id: String,
    #[serde(default)]
    viewer_handle: String,
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

/// Page switches xmaster itself acts on, cached beside the operations' ones.
pub const CASTLE_CLIENT_EVENTS: &str = "responsive_web_castle_client_event_enabled";
const KEPT_SWITCHES: &[&str] = &[CASTLE_CLIENT_EVENTS];

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

/// URLs of the lazy JS chunks whose `~`-joined name includes `segment`
/// (e.g. `bundle.HomeTimeline`), from the page's chunk-name and hash maps.
pub fn lazy_chunk_urls(html: &str, segment: &str) -> Vec<String> {
    let names = regex::Regex::new(r#"(\d+):"((?:bundle|shared|loader)[\w.~-]*)""#).unwrap();
    names
        .captures_iter(html)
        .filter(|c| c[2].split('~').any(|s| s == segment))
        .filter_map(|c| {
            let hash = regex::Regex::new(&format!(r#"[{{,]{}:"([0-9a-f]{{8,}})""#, &c[1])).ok()?;
            let h = hash.captures(html)?;
            Some(format!("https://abs.twimg.com/responsive-web/client-web/{}.{}a.js", &c[2], &h[1]))
        })
        .collect()
}

/// The logged-in user's ID from the page's `"session":{…"user_id":"…"}`.
fn viewer_id(html: &str) -> Option<String> {
    regex::Regex::new(r#""session":\{.{0,2000}?"user_id":"(\d+)""#)
        .unwrap()
        .captures(html)
        .map(|c| c[1].to_string())
}

/// The logged-in user's handle, from their entry in the page's user store.
fn viewer_handle(html: &str, id: &str) -> Option<String> {
    regex::Regex::new(&format!(r#""{id}":\{{"name":.{{0,3000}}?"screen_name":"(\w+)""#))
        .ok()?
        .captures(html)
        .map(|c| c[1].to_string())
}

/// A script's text, fetched without x.com cookies (a browser doesn't send
/// them to the static-asset host). Empty on any failure.
async fn script_text(client: &reqwest::Client, url: &str) -> String {
    match client.get(url).header("user-agent", UA).send().await {
        Ok(r) if r.status().is_success() => r.text().await.unwrap_or_default(),
        _ => String::new(),
    }
}

/// Fetch scripts concurrently: (url, text) pairs in input order.
async fn fetch_scripts(client: &reqwest::Client, urls: &[String]) -> Vec<(String, String)> {
    let mut set = tokio::task::JoinSet::new();
    for (i, url) in urls.iter().enumerate() {
        let (client, url) = (client.clone(), url.clone());
        set.spawn(async move {
            let text = script_text(&client, &url).await;
            (i, url, text)
        });
    }
    let mut got = set.join_all().await;
    got.sort_by_key(|g| g.0);
    got.into_iter().map(|(_, u, t)| (u, t)).collect()
}

/// The script with the transaction-ID key indices: the legacy build names
/// ondemand.s in the page's chunk map; the x-web (Vite) build imports a
/// sign.o-* script from its transaction-id plugin, one or two imports deep.
async fn signing_script(client: &reqwest::Client, html: &str) -> Result<String, XmasterError> {
    if let Ok(url) = transaction_id::ondemand_url(html) {
        return Ok(script_text(client, &url).await);
    }
    let mut frontier = transaction_id::x_web_scripts(html);
    let mut seen: std::collections::HashSet<String> = frontier.iter().cloned().collect();
    for _depth in 0..3 {
        let mut next = Vec::new();
        for batch in frontier.chunks(SCRIPT_BATCH) {
            let texts = fetch_scripts(client, batch).await;
            if let Some(url) = texts.iter().find_map(|(u, t)| transaction_id::indices_script_ref(t, u)) {
                return Ok(script_text(client, &url).await);
            }
            for (u, t) in &texts {
                next.extend(transaction_id::relative_imports(t, u).into_iter().filter(|i| seen.insert(i.clone())));
            }
        }
        // The transaction-id plugin is the importer: look there first
        next.sort_by_key(|u| !u.contains("transaction"));
        frontier = next;
    }
    Err(web_err("no_signing_script", "Found no transaction-ID signing script in x.com's bundles".into()))
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

/// An operation's declared feature switches with the logged-in page's values.
fn features(state: &State, op: &str) -> Option<Value> {
    let meta = state.ops.get(op).filter(|m| !m.features.is_empty())?;
    Some(Value::Object(
        meta.features
            .iter()
            .map(|f| (f.clone(), Value::Bool(state.switches.get(f).copied().unwrap_or(false))))
            .collect(),
    ))
}

/// What a timeline read does with one post.
enum Take {
    Keep,
    Skip,
    /// Out of range: skip it and fetch no further pages.
    Done,
}

/// RFC 3339 → Unix seconds.
fn unix(ts: Option<&str>) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(ts?).ok().map(|d| d.timestamp())
}

/// Keep posts created inside [since, until); newest-first timelines are done
/// once a post predates `since`.
fn window(since: Option<&str>, until: Option<&str>) -> impl Fn(&TweetData) -> Take {
    let (since, until) = (unix(since), unix(until));
    move |t| {
        let Some(at) = unix(t.created_at.as_deref()) else { return Take::Keep };
        if since.is_some_and(|s| at < s) {
            Take::Done
        } else if until.is_some_and(|u| at >= u) {
            Take::Skip
        } else {
            Take::Keep
        }
    }
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
        if transaction_id::is_challenge_page(&html) {
            *guard = Some(prev);
            return Err(web_err(
                "web_challenge",
                "x.com answered with a Cloudflare challenge page instead of the app. Open x.com \
                 in Chrome on this network, then retry"
                    .into(),
            ));
        }
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

        // Query IDs come from the legacy main.js. The x-web build uses Relay
        // persisted queries with another schema; there the cached and
        // fallback IDs stay in use.
        let main_js = main_js_url(&html).unwrap_or_default();
        let client = &self.ctx.client;
        let new_bundle = !main_js.is_empty() && (main_js != prev.main_js || prev.ops.is_empty());
        let (bundle, signing) = tokio::join!(
            async { if new_bundle { script_text(client, &main_js).await } else { String::new() } },
            signing_script(client, &html),
        );
        let found = parse_ops(&bundle);
        let mut ops = if found.is_empty() { prev.ops } else { found };
        for (op, segment) in LAZY_OPS {
            if ops.contains_key(*op) {
                continue;
            }
            for url in lazy_chunk_urls(&html, segment) {
                ops.extend(parse_ops(&script_text(client, &url).await));
                if ops.contains_key(*op) {
                    break;
                }
            }
        }
        let signing = signing?;
        // Keep only the switches some operation declares (the page has ~1k)
        let used: std::collections::HashSet<&String> = ops.values().flat_map(|m| &m.features).collect();
        let switches = parse_switches(&html)
            .into_iter()
            .filter(|(k, _)| used.contains(k) || KEPT_SWITCHES.contains(&k.as_str()))
            .collect();
        let state = State {
            fetched_at: now(),
            main_js: if main_js.is_empty() { prev.main_js } else { main_js },
            ops,
            switches,
            tx: Some(transaction_id::derive(&html, &signing)?),
            viewer_handle: viewer_id(&html)
                .and_then(|id| viewer_handle(&html, &id))
                .unwrap_or(prev.viewer_handle),
            viewer_id: viewer_id(&html).unwrap_or(prev.viewer_id),
        };
        if let Ok(bytes) = serde_json::to_vec(&state) {
            let _ = std::fs::write(state_path(), bytes);
        }
        *guard = Some(state.clone());
        Ok(state)
    }

    fn query_id(state: &State, op: &str) -> String {
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
            let (method, path, req) = build(&Self::query_id(&state, op));
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
            if body.trim_start().starts_with('<') {
                return Err(web_err(
                    "web_challenge",
                    format!(
                        "{op}: X served a browser challenge page (HTTP {status}) instead of data. \
                         Open x.com in Chrome, then retry"
                    ),
                ));
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
        let features = features(&state, op).or(fallback_features);
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

    /// Run a GraphQL query the way x.com does: a GET with the variables,
    /// feature switches and field toggles in the query string.
    pub async fn query(&self, op: &str, variables: Value, referer: &str) -> Result<Value, XmasterError> {
        let state = self.state(false).await?;
        let features = features(&state, op)
            .or_else(|| features(&state, TIMELINE_FEATURES_OP))
            .unwrap_or_else(|| json!({}));
        let variables = variables.to_string();
        let features = features.to_string();
        self.send(op, referer, |qid| {
            let path = format!("/i/api/graphql/{qid}/{op}");
            let req = self.ctx.client.get(format!("https://x.com{path}")).query(&[
                ("variables", variables.as_str()),
                ("features", features.as_str()),
                ("fieldToggles", READ_FIELD_TOGGLES),
            ]);
            ("GET", path, req)
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
        let val = self
            .query("UserByScreenName", json!({ "screen_name": handle }), "https://x.com/home")
            .await?;
        val.pointer("/data/user/result/core/screen_name")
            .or_else(|| val.pointer("/data/user/result/legacy/screen_name"))
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| web_err("probe", format!("Unexpected response: {}", crate::utils::safe_truncate(&val.to_string(), 200))))
    }

    // -- Reads --------------------------------------------------------------

    /// The logged-in account's (user ID, handle), free from the cached page
    /// state. The handle may be empty.
    async fn viewer(&self) -> Result<(String, String), XmasterError> {
        let mut state = self.state(false).await?;
        if state.viewer_id.is_empty() {
            // State cached before these fields existed
            state = self.state(true).await?;
        }
        if state.viewer_id.is_empty() {
            return Err(web_err("no_viewer_id", "The x.com page carried no user ID".into()));
        }
        Ok((state.viewer_id, state.viewer_handle))
    }

    /// The logged-in account's user ID.
    pub async fn viewer_id(&self) -> Result<String, XmasterError> {
        Ok(self.viewer().await?.0)
    }

    /// The logged-in account's profile.
    pub async fn viewer_profile(&self) -> Result<UserResponse, XmasterError> {
        let (id, handle) = self.viewer().await?;
        if handle.is_empty() {
            return Err(web_err("no_viewer_handle", format!("The x.com page named no handle for user {id}")));
        }
        self.user_by_screen_name(&handle).await
    }

    /// A profile by handle (UserByScreenName).
    pub async fn user_by_screen_name(&self, handle: &str) -> Result<UserResponse, XmasterError> {
        let val = self
            .query("UserByScreenName", json!({ "screen_name": handle }), "https://x.com/home")
            .await?;
        val.pointer("/data/user/result")
            .and_then(web_timeline::parse_user)
            .ok_or_else(|| XmasterError::NotFound(format!("User @{handle}")))
    }

    /// Public metrics and author ID for up to 100 posts per call
    /// (TweetResultsByRestIds). Unavailable posts are left out.
    pub async fn tweet_lookups(&self, ids: &[String]) -> Result<Vec<(TweetLookup, String)>, XmasterError> {
        let mut out = Vec::with_capacity(ids.len());
        for chunk in ids.chunks(100) {
            let variables = json!({
                "tweetIds": chunk,
                "includePromotedContent": false,
                "withBirdwatchNotes": false,
                "withVoice": false,
                "withCommunity": false,
            });
            let val = self.query("TweetResultsByRestIds", variables, "https://x.com/home").await?;
            let results = val.pointer("/data/tweetResult").and_then(Value::as_array).cloned().unwrap_or_default();
            out.extend(results.iter().filter_map(|r| r.get("result")).filter_map(web_timeline::parse_lookup));
        }
        Ok(out)
    }

    /// One post by ID (TweetResultByRestId).
    pub async fn tweet(&self, id: &str) -> Result<TweetData, XmasterError> {
        let variables = json!({
            "tweetId": id,
            "withCommunity": false,
            "includePromotedContent": false,
            "withVoice": false,
        });
        let referer = format!("https://x.com/i/status/{id}");
        let val = self.query("TweetResultByRestId", variables, &referer).await?;
        val.pointer("/data/tweetResult/result")
            .and_then(web_timeline::parse_tweet)
            .ok_or_else(|| XmasterError::NotFound(format!("Post {id}")))
    }

    /// Page through a timeline query until `want` posts are taken, the
    /// timeline ends, or `take` reports the rest is out of range. Pauses
    /// between pages like someone scrolling.
    async fn timeline(
        &self,
        op: &str,
        variables: Value,
        referer: &str,
        want: usize,
        mut take: impl FnMut(&TweetData) -> Take,
    ) -> Result<Vec<TweetData>, XmasterError> {
        let mut out = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut cursor: Option<String> = None;
        for page_no in 0..want / 10 + 3 {
            if page_no > 0 {
                tokio::time::sleep(Duration::from_millis(400 + rand::random::<u64>() % 600)).await;
            }
            let mut vars = variables.clone();
            if let Some(c) = &cursor {
                vars["cursor"] = json!(c);
            }
            let Page { tweets, bottom } = web_timeline::parse_timeline(&self.query(op, vars, referer).await?);
            let (mut fresh, mut done) = (false, false);
            for t in tweets {
                if !seen.insert(t.id.clone()) {
                    continue;
                }
                fresh = true;
                match take(&t) {
                    Take::Keep => out.push(t),
                    Take::Skip => {}
                    Take::Done => done = true,
                }
            }
            if done || !fresh || out.len() >= want || bottom.is_none() || bottom == cursor {
                break;
            }
            cursor = bottom;
        }
        out.truncate(want);
        debug!("{op}: {} posts served by the web session (free)", out.len());
        Ok(out)
    }

    /// SearchTimeline. `top` picks the Top tab, else Latest; the time bounds
    /// (RFC 3339) become `since_time:` / `until_time:` operators.
    pub async fn search(
        &self,
        query: &str,
        top: bool,
        want: usize,
        since: Option<&str>,
        until: Option<&str>,
    ) -> Result<Vec<TweetData>, XmasterError> {
        let mut raw = query.to_string();
        if let Some(t) = unix(since) {
            raw.push_str(&format!(" since_time:{t}"));
        }
        if let Some(t) = unix(until) {
            raw.push_str(&format!(" until_time:{t}"));
        }
        let product = if top { "Top" } else { "Latest" };
        let referer = format!(
            "https://x.com/search?q={}&src=typed_query{}",
            percent_encoding::utf8_percent_encode(query, percent_encoding::NON_ALPHANUMERIC),
            if top { "" } else { "&f=live" }
        );
        let variables = json!({
            "rawQuery": raw,
            "count": 20,
            "querySource": "typed_query",
            "product": product,
            "withGrokTranslatedBio": false,
            "withQuickPromoteEligibilityTweetFields": false,
        });
        self.timeline("SearchTimeline", variables, &referer, want, |_| Take::Keep).await
    }

    /// Direct replies to a post, from its conversation view (TweetDetail):
    /// the whole thread, with no 7-day search window.
    pub async fn replies(&self, tweet_id: &str, want: usize) -> Result<Vec<TweetData>, XmasterError> {
        let variables = json!({
            "focalTweetId": tweet_id,
            "referrer": "tweet",
            "with_rux_injections": false,
            "rankingMode": "Recency",
            "includePromotedContent": true,
            "withCommunity": true,
            "withQuickPromoteEligibilityTweetFields": true,
            "withBirdwatchNotes": true,
            "withVoice": true,
        });
        let referer = format!("https://x.com/i/status/{tweet_id}");
        self.timeline("TweetDetail", variables, &referer, want, |t| {
            let direct = t
                .referenced_tweets
                .as_ref()
                .is_some_and(|r| r.iter().any(|r| r.ref_type == "replied_to" && r.id == tweet_id));
            if direct { Take::Keep } else { Take::Skip }
        })
        .await
    }

    /// A user's posts, replies and reposts, newest first, within the
    /// optional RFC 3339 bounds. UserTweetsAndReplies matches the API's user
    /// timeline (UserTweets leaves replies out); the other people's posts it
    /// shows for context are dropped.
    pub async fn user_tweets(
        &self,
        user_id: &str,
        want: usize,
        since: Option<&str>,
        until: Option<&str>,
    ) -> Result<Vec<TweetData>, XmasterError> {
        let variables = json!({
            "userId": user_id,
            "count": 20,
            "includePromotedContent": true,
            "withCommunity": true,
            "withVoice": true,
        });
        let in_range = window(since, until);
        self.timeline("UserTweetsAndReplies", variables, "https://x.com/home", want, |t| {
            if t.author_id.as_deref() != Some(user_id) {
                return Take::Skip;
            }
            in_range(t)
        })
        .await
    }

    /// The Following tab (HomeLatestTimeline): the API's reverse-chronological
    /// home timeline.
    pub async fn home_latest(
        &self,
        want: usize,
        since: Option<&str>,
        until: Option<&str>,
    ) -> Result<Vec<TweetData>, XmasterError> {
        let variables = json!({
            "count": 20,
            "includePromotedContent": true,
            "latestControlAvailable": true,
            "requestContext": "launch",
            "withCommunity": true,
            "seenTweetIds": [],
        });
        self.timeline("HomeLatestTimeline", variables, "https://x.com/home", want, window(since, until))
            .await
    }

    /// Posts that mention the logged-in account, newest first, after
    /// `since_id`. The Mentions tab and a search for the handle each hide
    /// some posts behind their own quality filters, so this merges both.
    pub async fn mentions(&self, want: usize, since_id: Option<&str>) -> Result<Vec<TweetData>, XmasterError> {
        let since: u64 = since_id.and_then(|s| s.parse().ok()).unwrap_or(0);
        let after = |t: &TweetData| t.id.parse::<u64>().unwrap_or(u64::MAX) > since;
        let variables = json!({ "timeline_type": "Mentions", "count": 40 });
        let mut all = self
            .timeline("NotificationsTimeline", variables, "https://x.com/notifications/mentions", want, |t| {
                if after(t) { Take::Keep } else { Take::Done }
            })
            .await?;
        let (_, handle) = self.viewer().await?;
        if !handle.is_empty() {
            let mut q = format!("@{handle} -from:{handle}");
            if since > 0 {
                q.push_str(&format!(" since_id:{since}"));
            }
            match self.search(&q, false, want, None, None).await {
                Ok(found) => all.extend(found.into_iter().filter(after)),
                Err(e) => warn!("Mention search failed ({e}); using the Mentions tab alone"),
            }
        }
        let mut seen = std::collections::HashSet::new();
        all.retain(|t| seen.insert(t.id.clone()));
        all.sort_by_key(|t| std::cmp::Reverse(t.id.parse::<u64>().unwrap_or(0)));
        all.truncate(want);
        Ok(all)
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

    pub async fn delete_tweet(&self, id: &str) -> Result<(), XmasterError> {
        self.tweet_action("DeleteTweet", json!({ "tweet_id": id, "dark_request": false })).await
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

    /// Mute, unmute, block or unblock by handle: the v1.1 endpoints x.com
    /// itself calls, so no paid user-ID lookup is needed.
    pub async fn moderate(&self, action: Moderation, username: &str) -> Result<(), XmasterError> {
        let (op, path) = match action {
            Moderation::Mute => ("Mute", "/i/api/1.1/mutes/users/create.json"),
            Moderation::Unmute => ("Unmute", "/i/api/1.1/mutes/users/destroy.json"),
            Moderation::Block => ("Block", "/i/api/1.1/blocks/create.json"),
            Moderation::Unblock => ("Unblock", "/i/api/1.1/blocks/destroy.json"),
        };
        self.rest_form(op, path, &[("screen_name", username)]).await.map(|_| ())
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Moderation {
    Mute,
    Unmute,
    Block,
    Unblock,
}

// Media upload support (providers/media.rs).
impl Web<'_> {
    /// `req` with this session's web headers and a transaction ID signed for
    /// `method` + `path`, for the upload.x.com media endpoints.
    pub(crate) async fn upload_headers(
        &self,
        req: RequestBuilder,
        method: &str,
        path: &str,
    ) -> Result<RequestBuilder, XmasterError> {
        let state = self.state(false).await?;
        let tx = state.tx.as_ref().map(|k| transaction_id::sign(k, method, path)).unwrap_or_default();
        Ok(self.headers(req, &tx, "https://x.com/compose/post"))
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
    fn finds_viewer_id_past_nested_objects() {
        let html = r#""session":{"communitiesActions":{"create":false},"guestId":"1","ssoInitTokens":{},"user_id":"12345","x":{}}"#;
        assert_eq!(viewer_id(html).as_deref(), Some("12345"));
        assert_eq!(viewer_id(r#""session":{"isLoaded":true}"#), None);
        let page = r#""users":{"entities":{"12345":{"name":"Me","x":{"a":1},"screen_name":"me_here"}},"#;
        assert_eq!(viewer_handle(page, "12345").as_deref(), Some("me_here"));
        assert_eq!(viewer_handle(page, "999"), None);
    }

    #[test]
    fn finds_lazy_chunks_by_name_segment() {
        let html = r#"{69195:"bundle.HomeTimeline",53852:"shared~bundle.Compose~bundle.HomeTimeline~bundle.LoggedInMain",1:"bundle.HomeTimelineX"}[e]+"."+{53852:"abc12345def",69195:"538dc3798a981289"}"#;
        let urls = lazy_chunk_urls(html, "bundle.HomeTimeline");
        assert_eq!(urls.len(), 2);
        assert!(urls.contains(&"https://abs.twimg.com/responsive-web/client-web/bundle.HomeTimeline.538dc3798a981289a.js".to_string()));
        assert!(urls.iter().any(|u| u.contains("shared~bundle.Compose~bundle.HomeTimeline~bundle.LoggedInMain.abc12345defa.js")));
    }

    #[test]
    fn time_window() {
        let at = |ts: &str| TweetData {
            id: "1".into(), text: String::new(), author_id: None, author_username: None,
            created_at: Some(ts.into()), conversation_id: None, referenced_tweets: None,
            public_metrics: None, author_followers: None, media_urls: vec![],
        };
        let w = window(Some("2026-10-10T00:00:00Z"), Some("2026-10-12T00:00:00Z"));
        assert!(matches!(w(&at("2026-10-11T00:00:00.000Z")), Take::Keep));
        assert!(matches!(w(&at("2026-10-12T00:00:00.000Z")), Take::Skip));
        assert!(matches!(w(&at("2026-10-09T23:59:59.000Z")), Take::Done));
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

impl Web<'_> {
    /// POST a form to an x.com API path once, with the session's headers and
    /// a signed transaction ID; no stale-ID refresh, no retry. For
    /// fire-and-forget client events (`web_events`). Returns the HTTP status.
    pub(crate) async fn post_form(&self, path: &str, form: &[(&str, String)], referer: &str) -> Result<u16, XmasterError> {
        let state = self.state(false).await?;
        let tx = state.tx.as_ref().map(|k| transaction_id::sign(k, "POST", path)).unwrap_or_default();
        let req = self.ctx.client.post(format!("https://x.com{path}")).form(form);
        Ok(self.headers(req, &tx, referer).send().await?.status().as_u16())
    }
}

impl Web<'_> {
    /// A logged-in page switch from the cached session state (`None` when the
    /// state can't be loaded or the switch isn't cached).
    pub(crate) async fn switch(&self, name: &str) -> Option<bool> {
        self.state(false).await.ok()?.switches.get(name).copied()
    }
}
