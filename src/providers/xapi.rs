use crate::context::AppContext;
use crate::errors::XmasterError;
use base64::Engine as _;
use reqwest::Method;
use reqwest_oauth1::OAuthClientProvider;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, OnceCell};
use tracing::warn;

/// Max retry attempts for transient errors (429, 5xx).
const MAX_RETRIES: u32 = 3;

const BASE: &str = "https://api.x.com/2";
const UPLOAD_URL: &str = "https://upload.twitter.com/1.1/media/upload.json";

/// Public bearer token used by the X web app (same for all users, not secret).
const WEB_BEARER: &str = "AAAAAAAAAAAAAAAAAAAAANRILgAAAAAAnNwIzUejRCOuH5E6I8xnZz4puTs%3D1Zv7ttfk8LF81IUq16cHjhLTvJu4FA33AGWWjCpTnA";

/// Default GraphQL CreateTweet query ID (hash rotates every few weeks on X deploys).
/// Can be overridden via config: keys.graphql_create_tweet_id
const DEFAULT_GRAPHQL_CREATE_TWEET_ID: &str = "oB-5XsHNAbjvARJEc8CZFw";

/// Default GraphQL CreateNoteTweet query ID for Premium long-form posts (>280 chars).
/// Can be overridden via config: keys.graphql_create_note_tweet_id
const DEFAULT_GRAPHQL_CREATE_NOTE_TWEET_ID: &str = "iCUB42lIfXf9qPKctjE5rQ";

// ---------------------------------------------------------------------------
// Rate limit info
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct RateLimitInfo {
    /// Total requests allowed in the window.
    pub limit: u32,
    /// Requests remaining in the current window.
    pub remaining: u32,
    /// Unix timestamp when the window resets.
    pub reset: u64,
}

// ---------------------------------------------------------------------------
// Response / data types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TweetResponse {
    pub id: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TweetData {
    pub id: String,
    pub text: String,
    #[serde(default)]
    pub author_id: Option<String>,
    #[serde(default)]
    pub author_username: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub conversation_id: Option<String>,
    #[serde(default)]
    pub referenced_tweets: Option<Vec<ReferencedTweet>>,
    #[serde(default)]
    pub public_metrics: Option<TweetMetrics>,
    /// Author's follower count (populated from includes.users)
    #[serde(default)]
    pub author_followers: Option<u64>,
    /// Media URLs (populated from includes.media)
    #[serde(default)]
    pub media_urls: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReferencedTweet {
    #[serde(rename = "type")]
    pub ref_type: String,
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TweetMetrics {
    #[serde(default)]
    pub like_count: u64,
    #[serde(default)]
    pub retweet_count: u64,
    #[serde(default)]
    pub reply_count: u64,
    #[serde(default)]
    pub impression_count: u64,
    #[serde(default)]
    pub bookmark_count: u64,
}

// ---------------------------------------------------------------------------
// Batch lookup types (GET /2/tweets?ids=...)
// Kept separate from `TweetMetrics` / `TweetData` because they map a richer
// field set (public + non_public + created_at) and need Default for the
// 403→public-only fallback path used by `get_posts_by_ids`.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize, Default)]
pub struct TweetLookup {
    pub id: String,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub public_metrics: Option<TweetLookupPublicMetrics>,
    #[serde(default)]
    pub non_public_metrics: Option<TweetLookupNonPublicMetrics>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct TweetLookupPublicMetrics {
    #[serde(default)]
    pub like_count: u64,
    #[serde(default)]
    pub retweet_count: u64,
    #[serde(default)]
    pub reply_count: u64,
    #[serde(default)]
    pub impression_count: u64,
    #[serde(default)]
    pub quote_count: u64,
    #[serde(default)]
    pub bookmark_count: u64,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct TweetLookupNonPublicMetrics {
    #[serde(default)]
    pub url_link_clicks: u64,
    #[serde(default)]
    pub user_profile_clicks: u64,
}

#[derive(Deserialize, Default)]
struct TweetLookupBatchEnvelope {
    #[serde(default)]
    data: Vec<TweetLookup>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserResponse {
    pub id: String,
    pub name: String,
    pub username: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub public_metrics: Option<UserMetrics>,
    #[serde(default)]
    pub profile_image_url: Option<String>,
    #[serde(default)]
    pub verified: Option<bool>,
    #[serde(default)]
    pub created_at: Option<String>,
}

pub type UserData = UserResponse;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserMetrics {
    #[serde(default)]
    pub followers_count: u64,
    #[serde(default)]
    pub following_count: u64,
    #[serde(default)]
    pub tweet_count: u64,
    #[serde(default)]
    pub listed_count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DmConversation {
    pub id: String,
    #[serde(default)]
    pub participant_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DmMessage {
    pub id: String,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub sender_id: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
}

// ---------------------------------------------------------------------------
// Internal API envelope types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct ApiResponse<T> {
    data: Option<T>,
    #[serde(default)]
    errors: Option<Vec<ApiErrorDetail>>,
    #[serde(default)]
    meta: Option<ApiMeta>,
}

#[derive(Deserialize, Default)]
struct ApiMeta {
    #[serde(default)]
    next_token: Option<String>,
}

#[derive(Deserialize)]
struct ApiErrorDetail {
    #[serde(default)]
    message: String,
    #[serde(default)]
    detail: Option<String>,
}

#[derive(Deserialize)]
struct MediaUploadResponse {
    media_id_string: Option<String>,
    media_id: Option<u64>,
}

// ---------------------------------------------------------------------------
// XApi client
// ---------------------------------------------------------------------------

pub struct XApi {
    ctx: Arc<AppContext>,
    cached_user_id: OnceCell<String>,
    last_rate_limit: Mutex<Option<RateLimitInfo>>,
}

impl XApi {
    pub fn new(ctx: Arc<AppContext>) -> Self {
        Self {
            ctx,
            cached_user_id: OnceCell::new(),
            last_rate_limit: Mutex::new(None),
        }
    }

    // -- OAuth helpers ------------------------------------------------------

    fn secrets(&self) -> reqwest_oauth1::Secrets<'_> {
        let k = &self.ctx.config.keys;
        reqwest_oauth1::Secrets::new(&k.api_key, &k.api_secret)
            .token(&k.access_token, &k.access_token_secret)
    }

    fn require_auth(&self) -> Result<(), XmasterError> {
        if !self.ctx.config.has_x_auth() {
            return Err(XmasterError::AuthMissing {
                provider: "x",
                message: "X API credentials not configured".into(),
            });
        }
        Ok(())
    }

    // -- Rate limit header parser -------------------------------------------

    fn parse_rate_limit_headers(headers: &reqwest::header::HeaderMap) -> Option<RateLimitInfo> {
        let limit = headers
            .get("x-rate-limit-limit")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u32>().ok())?;
        let remaining = headers
            .get("x-rate-limit-remaining")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u32>().ok())?;
        let reset = headers
            .get("x-rate-limit-reset")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok())?;
        Some(RateLimitInfo { limit, remaining, reset })
    }

    // -- Retry wrapper ------------------------------------------------------

    /// Low-level signed request with automatic retry on transient errors.
    /// Returns the parsed JSON `Value`.
    async fn request(
        &self,
        method: Method,
        url: &str,
        body: Option<Value>,
    ) -> Result<Value, XmasterError> {
        let mut last_err: Option<XmasterError> = None;

        for attempt in 0..MAX_RETRIES {
            match self.request_once(method.clone(), url, body.clone()).await {
                Ok(val) => return Ok(val),
                Err(e) if e.is_retryable() && attempt + 1 < MAX_RETRIES => {
                    // Exponential backoff: 1s, 2s, 4s base + random 0-500ms jitter
                    let base_ms = 1000u64 * (1u64 << attempt);
                    let jitter_ms = rand::random::<u64>() % 500;
                    let mut delay = Duration::from_millis(base_ms + jitter_ms);

                    // For 429, honour Retry-After / x-rate-limit-reset if available
                    if let XmasterError::RateLimited { reset_at, .. } = &e {
                        if *reset_at > 0 {
                            let now = std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_secs();
                            if *reset_at > now {
                                let wait = (*reset_at - now).min(60) + (jitter_ms / 1000);
                                delay = Duration::from_secs(wait);
                            }
                        }
                    }

                    warn!(
                        attempt = attempt + 1,
                        max = MAX_RETRIES,
                        delay_ms = delay.as_millis() as u64,
                        error = %e,
                        "Retrying after transient error"
                    );
                    tokio::time::sleep(delay).await;
                    last_err = Some(e);
                }
                Err(e) => return Err(e),
            }
        }

        Err(last_err.unwrap_or_else(|| XmasterError::Api {
            provider: "x",
            code: "retry_exhausted",
            message: "All retry attempts failed".into(),
        }))
    }

    /// Single HTTP request attempt (no retry). Parses rate-limit headers.
    async fn request_once(
        &self,
        method: Method,
        url: &str,
        body: Option<Value>,
    ) -> Result<Value, XmasterError> {
        self.require_auth()?;

        let resp = match method {
            Method::GET => {
                self.ctx.client.clone().oauth1(self.secrets())
                    .get(url)
                    .send().await?
            }
            Method::POST => {
                let mut b = self.ctx.client.clone().oauth1(self.secrets()).post(url);
                if let Some(ref json) = body {
                    b = b.header("Content-Type", "application/json")
                        .body(serde_json::to_string(json)?);
                }
                b.send().await?
            }
            Method::DELETE => {
                self.ctx.client.clone().oauth1(self.secrets())
                    .delete(url)
                    .send().await?
            }
            Method::PUT => {
                let mut b = self.ctx.client.clone().oauth1(self.secrets()).put(url);
                if let Some(ref json) = body {
                    b = b.header("Content-Type", "application/json")
                        .body(serde_json::to_string(json)?);
                }
                b.send().await?
            }
            _ => {
                return Err(XmasterError::Api {
                    provider: "x",
                    code: "unsupported_method",
                    message: format!("Unsupported HTTP method: {method}"),
                });
            }
        };

        let status = resp.status();

        // Parse and store rate limit headers from every response.
        if let Some(rl) = Self::parse_rate_limit_headers(resp.headers()) {
            *self.last_rate_limit.lock().await = Some(rl);
        }

        if status == 401 || status == 403 {
            let text = resp.text().await.unwrap_or_default();
            let message = if text.contains("oauth1-permissions") {
                format!(
                    "HTTP {status} Forbidden: {text}. \
                    Fix: Your Access Token was likely generated before enabling Read+Write. \
                    Go to developer.x.com → your app → Keys and tokens → Regenerate Access Token and Secret, \
                    then run: xmaster config set keys.access_token NEW_TOKEN && \
                    xmaster config set keys.access_token_secret NEW_SECRET"
                )
            } else {
                format!("HTTP {status}: {text}")
            };
            return Err(XmasterError::AuthMissing {
                provider: "x",
                message,
            });
        }

        if status == 429 {
            let reset_at = resp
                .headers()
                .get("x-rate-limit-reset")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<u64>().ok())
                .or_else(|| {
                    resp.headers()
                        .get("retry-after")
                        .and_then(|v| v.to_str().ok())
                        .and_then(|v| v.parse::<u64>().ok())
                        .map(|secs| {
                            std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_secs()
                                + secs
                        })
                })
                .unwrap_or(0);
            return Err(XmasterError::RateLimited {
                provider: "x",
                reset_at,
            });
        }

        // 5xx server errors — retryable
        if status.as_u16() >= 500 {
            return Err(XmasterError::ServerError {
                status: status.as_u16(),
            });
        }

        let text = resp.text().await?;

        if text.is_empty() {
            return Ok(Value::Null);
        }

        let val: Value = serde_json::from_str(&text).map_err(|_| XmasterError::Api {
            provider: "x",
            code: "json_parse",
            message: format!("Failed to parse response: {}", crate::utils::safe_truncate(&text, 200)),
        })?;

        if !status.is_success() {
            let msg = val["detail"]
                .as_str()
                .or_else(|| val["title"].as_str())
                .unwrap_or("Unknown error");
            return Err(XmasterError::Api {
                provider: "x",
                code: "api_error",
                message: format!("HTTP {status}: {msg}"),
            });
        }

        Ok(val)
    }

    /// Extract `data` field from an API response, returning a deserialized `T`.
    async fn request_data<T: serde::de::DeserializeOwned>(
        &self,
        method: Method,
        url: &str,
        body: Option<Value>,
    ) -> Result<T, XmasterError> {
        let val = self.request(method, url, body).await?;
        let envelope: ApiResponse<T> = serde_json::from_value(val.clone())?;

        if let Some(errors) = &envelope.errors {
            if envelope.data.is_none() {
                let msg = errors
                    .iter()
                    .map(|e| e.detail.as_deref().unwrap_or(&e.message))
                    .collect::<Vec<_>>()
                    .join("; ");
                return Err(XmasterError::Api {
                    provider: "x",
                    code: "api_error",
                    message: msg,
                });
            }
        }

        envelope.data.ok_or_else(|| XmasterError::Api {
            provider: "x",
            code: "no_data",
            message: "Response contained no data field".into(),
        })
    }

    /// Extract `data` as a `Vec<T>`, returning empty vec when data is absent.
    async fn request_list<T: serde::de::DeserializeOwned>(
        &self,
        method: Method,
        url: &str,
        body: Option<Value>,
    ) -> Result<(Vec<T>, Option<Value>), XmasterError> {
        let val = self.request(method, url, body).await?;

        // Grab includes before consuming val
        let includes = val.get("includes").cloned();

        let envelope: ApiResponse<Vec<T>> = serde_json::from_value(val)?;
        Ok((envelope.data.unwrap_or_default(), includes))
    }

    // -- Cached user ID -----------------------------------------------------

    pub async fn get_authenticated_user_id(&self) -> Result<String, XmasterError> {
        self.cached_user_id
            .get_or_try_init(|| async {
                let user: UserResponse = self.request_data(
                    Method::GET,
                    &format!("{BASE}/users/me?user.fields=id"),
                    None,
                ).await?;
                Ok(user.id)
            })
            .await
            .cloned()
    }

    // -- Tweet fields query string helpers ----------------------------------

    fn tweet_fields() -> &'static str {
        "tweet.fields=created_at,public_metrics,author_id,conversation_id,referenced_tweets,entities,lang,attachments"
    }

    fn tweet_expansions() -> &'static str {
        "expansions=author_id,attachments.media_keys&media.fields=url,preview_image_url,type,alt_text"
    }

    fn user_fields_param() -> &'static str {
        "user.fields=created_at,description,public_metrics,verified,profile_image_url,username,name"
    }

    /// Merge author usernames and media URLs from `includes` into tweet data.
    fn merge_authors(tweets: &mut [TweetData], includes: &Option<Value>) {
        let tweets_len = tweets.len();
        if let Some(inc) = includes {
            if let Some(users) = inc.get("users").and_then(|u| u.as_array()) {
                for tweet in tweets.iter_mut() {
                    if let Some(aid) = &tweet.author_id {
                        for user in users {
                            if user.get("id").and_then(|i| i.as_str()) == Some(aid) {
                                tweet.author_username =
                                    user.get("username").and_then(|u| u.as_str()).map(String::from);
                                tweet.author_followers = user
                                    .get("public_metrics")
                                    .and_then(|m| m.get("followers_count"))
                                    .and_then(|f| f.as_u64());
                            }
                        }
                    }
                }
            }
            // Merge media URLs from includes.media
            if let Some(media_list) = inc.get("media").and_then(|m| m.as_array()) {
                // Build a lookup: media_key → url
                let media_map: std::collections::HashMap<&str, &str> = media_list
                    .iter()
                    .filter_map(|m| {
                        let key = m.get("media_key").and_then(|k| k.as_str())?;
                        let url = m
                            .get("url")
                            .or_else(|| m.get("preview_image_url"))
                            .and_then(|u| u.as_str())?;
                        Some((key, url))
                    })
                    .collect();

                if !media_map.is_empty() {
                    for tweet in tweets.iter_mut() {
                        if tweet.media_urls.is_empty() {
                            // For single-tweet responses, just grab all media
                            if tweets_len == 1 {
                                tweet.media_urls = media_map.values().map(|u| u.to_string()).collect();
                            }
                            // For multi-tweet (search), media_keys aren't in TweetData
                            // so we can't match precisely — skip to avoid misattribution
                        }
                    }
                }
            }
        }
    }

    // =======================================================================
    // PUBLIC API METHODS
    // =======================================================================

    // -- Tweets -------------------------------------------------------------

    pub async fn create_tweet(
        &self,
        text: &str,
        reply_to: Option<&str>,
        quote_tweet_id: Option<&str>,
        media_ids: Option<&[String]>,
        poll_options: Option<&[String]>,
        poll_duration: Option<u64>,
    ) -> Result<TweetResponse, XmasterError> {
        let mut body = json!({ "text": text });

        if let Some(reply_id) = reply_to {
            body["reply"] = json!({ "in_reply_to_tweet_id": reply_id });
        }
        if let Some(qid) = quote_tweet_id {
            body["quote_tweet_id"] = json!(qid);
        }
        if let Some(ids) = media_ids {
            if !ids.is_empty() {
                body["media"] = json!({ "media_ids": ids });
            }
        }
        if let Some(opts) = poll_options {
            if !opts.is_empty() {
                body["poll"] = json!({
                    "options": opts,
                    "duration_minutes": poll_duration.unwrap_or(1440),
                });
            }
        }

        let result = self
            .request_data(Method::POST, &format!("{BASE}/tweets"), Some(body))
            .await;

        // Auto-fallback: if this is a reply and we got 403 (reply restriction),
        // retry via GraphQL web endpoint using browser cookies.
        if let Err(ref err) = result {
            if reply_to.is_some() && Self::is_reply_restricted(err) {
                if self.ctx.config.has_web_cookies() {
                    warn!("API reply blocked (X restriction). Falling back to web session...");
                    return self
                        .create_tweet_via_web(text, reply_to, quote_tweet_id, media_ids)
                        .await;
                } else {
                    return Err(XmasterError::ReplyRestricted(
                        "X blocks programmatic replies to users who haven't @mentioned you. \
                        Configure web cookies for automatic fallback."
                            .into(),
                    ));
                }
            }
        }

        result
    }

    /// Detect whether an error is the Feb 2026 reply restriction (403 on replies
    /// to users who haven't mentioned you).
    /// Note: oauth1-permissions 403s are handled separately in request_once() with
    /// a specific regeneration hint, so they won't reach here. Any 403 that makes
    /// it to this check is a non-permission 403, likely the reply restriction.
    fn is_reply_restricted(err: &XmasterError) -> bool {
        match err {
            XmasterError::AuthMissing { message, .. } => {
                message.contains("403") && !message.contains("oauth1-permissions")
            }
            XmasterError::Api { message, .. } => {
                message.contains("403")
                    || message.contains("reply")
                    || message.contains("not allowed")
                    || message.contains("not permitted")
            }
            _ => false,
        }
    }

    /// Post a tweet via X's internal GraphQL web endpoint using browser cookies.
    /// This bypasses the API reply restriction since it behaves like a browser session.
    /// For Premium accounts posting >280 chars, uses CreateNoteTweet mutation instead
    /// of CreateTweet (which hard-caps at 280 regardless of Premium status).
    async fn create_tweet_via_web(
        &self,
        text: &str,
        reply_to: Option<&str>,
        quote_tweet_id: Option<&str>,
        media_ids: Option<&[String]>,
    ) -> Result<TweetResponse, XmasterError> {
        let keys = &self.ctx.config.keys;

        // Select mutation: CreateNoteTweet for Premium long posts, CreateTweet otherwise
        let is_note_tweet = text.chars().count() > 280 && self.ctx.config.account.premium;
        let (query_id, operation_name) = if is_note_tweet {
            let id = if keys.graphql_create_note_tweet_id.is_empty() {
                DEFAULT_GRAPHQL_CREATE_NOTE_TWEET_ID.to_string()
            } else {
                keys.graphql_create_note_tweet_id.clone()
            };
            (id, "CreateNoteTweet")
        } else {
            let id = if keys.graphql_create_tweet_id.is_empty() {
                DEFAULT_GRAPHQL_CREATE_TWEET_ID.to_string()
            } else {
                keys.graphql_create_tweet_id.clone()
            };
            (id, "CreateTweet")
        };

        let ct0 = &keys.web_ct0;
        let auth_token = &keys.web_auth_token;

        // Generate x-client-transaction-id natively (no Python dependency)
        let gql_path = format!("/i/api/graphql/{query_id}/{operation_name}");
        let transaction_id = crate::transaction_id::generate(
            &self.ctx.client, "POST", &gql_path, ct0, auth_token,
        )
        .await
        .map_err(|e| {
            warn!("Native transaction ID failed: {e}");
            e
        })?;

        // Build the GraphQL variables
        let mut variables = json!({
            "tweet_text": text,
            "dark_request": false,
            "media": {
                "media_entities": [],
                "possibly_sensitive": false,
            },
            "semantic_annotation_ids": [],
        });

        if let Some(reply_id) = reply_to {
            variables["reply"] = json!({
                "in_reply_to_tweet_id": reply_id,
                "exclude_reply_user_ids": [],
            });
        }

        if let Some(qid) = quote_tweet_id {
            variables["quote_tweet_id"] = qid.into();
        }

        if let Some(ids) = media_ids {
            let entities: Vec<Value> = ids
                .iter()
                .map(|id| json!({ "media_id": id, "tagged_users": [] }))
                .collect();
            variables["media"]["media_entities"] = json!(entities);
        }

        // CreateNoteTweet requires richtext_options for long-form posts
        if is_note_tweet {
            variables["richtext_options"] = json!({ "richtext_tags": [] });
        }

        let gql_body = json!({
            "variables": variables,
            "features": {
                "communities_web_enable_tweet_community_results_fetch": true,
                "c9s_tweet_anatomy_moderator_badge_enabled": true,
                "responsive_web_edit_tweet_api_enabled": true,
                "graphql_is_translatable_rweb_tweet_is_translatable_enabled": true,
                "view_counts_everywhere_api_enabled": true,
                "longform_notetweets_consumption_enabled": true,
                "responsive_web_twitter_article_tweet_consumption_enabled": true,
                "tweet_awards_web_tipping_enabled": false,
                "creator_subscriptions_quote_tweet_preview_enabled": false,
                "longform_notetweets_rich_text_read_enabled": true,
                "longform_notetweets_inline_media_enabled": true,
                "articles_preview_enabled": true,
                "rweb_video_timestamps_enabled": true,
                "rweb_tipjar_consumption_enabled": true,
                "responsive_web_graphql_exclude_directive_enabled": true,
                "verified_phone_label_enabled": false,
                "freedom_of_speech_not_reach_fetch_enabled": true,
                "standardized_nudges_misinfo": true,
                "tweet_with_visibility_results_prefer_gql_limited_actions_policy_enabled": true,
                "responsive_web_graphql_skip_user_profile_image_extensions_enabled": false,
                "responsive_web_graphql_timeline_navigation_enabled": true,
                "responsive_web_enhance_cards_enabled": false,
            },
            "queryId": &query_id,
        });

        let cookie_header = format!("ct0={ct0}; auth_token={auth_token}");
        let bearer = format!("Bearer {WEB_BEARER}");
        let gql_url = format!("https://x.com{gql_path}");

        let resp = self
            .ctx
            .client
            .post(&gql_url)
            .header("authorization", &bearer)
            .header("x-csrf-token", ct0)
            .header("cookie", &cookie_header)
            .header("content-type", "application/json")
            .header("x-twitter-active-user", "yes")
            .header("x-twitter-auth-type", "OAuth2Session")
            .header("x-client-transaction-id", &transaction_id)
            .header("user-agent", "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/146.0.0.0 Safari/537.36")
            .header("origin", "https://x.com")
            .header("referer", "https://x.com/compose/post")
            .header("x-twitter-client-language", "en")
            .header("accept", "*/*")
            .header("accept-language", "en-US,en;q=0.9")
            .header("sec-ch-ua", "\"Google Chrome\";v=\"146\", \"Chromium\";v=\"146\", \"Not_A Brand\";v=\"24\"")
            .header("sec-ch-ua-mobile", "?0")
            .header("sec-ch-ua-platform", "\"macOS\"")
            .header("sec-fetch-dest", "empty")
            .header("sec-fetch-mode", "cors")
            .header("sec-fetch-site", "same-origin")
            .body(serde_json::to_string(&gql_body)?)
            .send()
            .await?;

        let status = resp.status();
        let body_text = resp.text().await.unwrap_or_default();

        if !status.is_success() {
            // Specific error for Premium long posts that get rejected
            if is_note_tweet && (body_text.contains("186") || body_text.contains("shorter")) {
                return Err(XmasterError::Api {
                    provider: "x-web",
                    code: "note_tweet_rejected",
                    message: "Long post rejected by X. Verify your X Premium subscription is active.".into(),
                });
            }
            return Err(XmasterError::Api {
                provider: "x-web",
                code: "graphql_error",
                message: format!(
                    "Web fallback failed (HTTP {status}): {}",
                    crate::utils::safe_truncate(&body_text, 300)
                ),
            });
        }

        // Parse the GraphQL response to extract tweet ID and text
        let val: Value = serde_json::from_str(&body_text).map_err(|_| XmasterError::Api {
            provider: "x-web",
            code: "json_parse",
            message: format!("Failed to parse GraphQL response: {}", crate::utils::safe_truncate(&body_text, 200)),
        })?;

        // Navigate response: CreateNoteTweet uses data.notetweet_create, CreateTweet uses data.create_tweet
        let result_path = if is_note_tweet {
            "/data/notetweet_create/tweet_results/result"
        } else {
            "/data/create_tweet/tweet_results/result"
        };
        let tweet_result = val
            .pointer(result_path)
            .ok_or_else(|| XmasterError::Api {
                provider: "x-web",
                code: "no_tweet_result",
                message: format!("Unexpected GraphQL response shape: {}", crate::utils::safe_truncate(&body_text, 300)),
            })?;

        let tweet_id = tweet_result
            .get("rest_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| XmasterError::Api {
                provider: "x-web",
                code: "no_rest_id",
                message: "No rest_id in tweet result".into(),
            })?;

        // Try to get the full text from the nested legacy object
        let tweet_text = tweet_result
            .pointer("/legacy/full_text")
            .and_then(|v| v.as_str())
            .unwrap_or(text);

        Ok(TweetResponse {
            id: tweet_id.to_string(),
            text: tweet_text.to_string(),
        })
    }

    /// Look up any tweet by ID (yours or someone else's). Only requests public metrics.
    /// Batch fetch public + non_public metrics for up to 100 tweet IDs per HTTP call.
    /// Results for larger inputs are chunked internally and concatenated.
    ///
    /// Tries the full field set first (`public_metrics,non_public_metrics,created_at`).
    /// `non_public_metrics` is only visible for tweets the authenticated user owns.
    /// If the batch contains only tweets you don't own, X returns 403 — this method
    /// transparently retries the same chunk with `public_metrics` only.
    ///
    /// Uses the raw signed-request path rather than [`Self::request`] because the
    /// generic helper collapses 403 into `AuthMissing`, which would break the
    /// fallback. 401/429/5xx propagate as typed errors so the caller can retry.
    pub async fn get_posts_by_ids(
        &self,
        tweet_ids: &[String],
    ) -> Result<Vec<TweetLookup>, XmasterError> {
        self.require_auth()?;
        if tweet_ids.is_empty() {
            return Ok(Vec::new());
        }

        let mut out: Vec<TweetLookup> = Vec::with_capacity(tweet_ids.len());
        for chunk in tweet_ids.chunks(100) {
            let ids_param = chunk.join(",");
            let url_full = format!(
                "{BASE}/tweets?ids={ids_param}&tweet.fields=public_metrics,non_public_metrics,created_at"
            );
            let resp = self
                .ctx
                .client
                .clone()
                .oauth1(self.secrets())
                .get(&url_full)
                .send()
                .await?;

            let first_status = resp.status();
            let first_body = resp.text().await.unwrap_or_default();

            if first_status.is_success() {
                if let Ok(envelope) =
                    serde_json::from_str::<TweetLookupBatchEnvelope>(&first_body)
                {
                    out.extend(envelope.data);
                    continue;
                }
            }

            if first_status == 401 {
                return Err(XmasterError::AuthMissing {
                    provider: "x",
                    message: format!(
                        "HTTP 401: {}",
                        crate::utils::safe_truncate(&first_body, 200)
                    ),
                });
            }
            if first_status == 429 {
                return Err(XmasterError::RateLimited {
                    provider: "x",
                    reset_at: 0,
                });
            }
            if first_status.as_u16() >= 500 {
                return Err(XmasterError::ServerError {
                    status: first_status.as_u16(),
                });
            }

            // 403 or other client error — retry this chunk with public fields only.
            let url_public =
                format!("{BASE}/tweets?ids={ids_param}&tweet.fields=public_metrics,created_at");
            let resp = self
                .ctx
                .client
                .clone()
                .oauth1(self.secrets())
                .get(&url_public)
                .send()
                .await?;
            let status = resp.status();
            if !status.is_success() {
                let text = resp.text().await.unwrap_or_default();
                return Err(XmasterError::NotFound(format!(
                    "Tweets {ids_param} (HTTP {status}: {})",
                    crate::utils::safe_truncate(&text, 100)
                )));
            }
            let envelope: TweetLookupBatchEnvelope = resp.json().await?;
            out.extend(envelope.data);
        }
        Ok(out)
    }

    pub async fn get_tweet(&self, id: &str) -> Result<TweetData, XmasterError> {
        let url = format!(
            "{BASE}/tweets/{id}?{tf}&{exp}&{uf}",
            tf = Self::tweet_fields(),
            exp = Self::tweet_expansions(),
            uf = Self::user_fields_param(),
        );
        let val = self.request(Method::GET, &url, None).await?;
        let includes = val.get("includes").cloned();
        let mut tweet: TweetData = serde_json::from_value(
            val.get("data")
                .cloned()
                .ok_or_else(|| XmasterError::NotFound(format!("Tweet {id}")))?
        )?;
        Self::merge_authors(&mut [tweet.clone()], &includes);
        if let Some(inc) = &includes {
            Self::merge_authors(std::slice::from_mut(&mut tweet), &Some(inc.clone()));
        }
        Ok(tweet)
    }

    /// Get replies to a tweet (last 7 days).
    ///
    /// The conversation_id of a reply equals the root tweet of the thread, not
    /// the reply's own ID.  So we first fetch the target tweet to learn its
    /// conversation_id, search the whole conversation, then filter to only
    /// direct replies to the target tweet.
    pub async fn get_replies(
        &self,
        tweet_id: &str,
        count: usize,
    ) -> Result<Vec<TweetData>, XmasterError> {
        // 1. Fetch the target tweet to get its conversation_id
        let target = self.get_tweet(tweet_id).await?;
        let conv_id = target
            .conversation_id
            .as_deref()
            .unwrap_or(tweet_id);

        // 2. Search the full conversation
        let max = count.clamp(10, 100);
        let query = format!("conversation_id:{conv_id}");
        let all = self.search_tweets(&query, "recency", max).await?;

        // 3. Filter to direct replies to the target tweet
        let replies: Vec<TweetData> = all
            .into_iter()
            .filter(|t| {
                // Exclude the target tweet itself
                if t.id == tweet_id {
                    return false;
                }
                // Keep tweets that reference the target as replied_to
                t.referenced_tweets
                    .as_ref()
                    .map(|refs| {
                        refs.iter()
                            .any(|r| r.ref_type == "replied_to" && r.id == tweet_id)
                    })
                    .unwrap_or(false)
            })
            .collect();

        Ok(replies)
    }

    pub async fn delete_tweet(&self, id: &str) -> Result<(), XmasterError> {
        self.request(Method::DELETE, &format!("{BASE}/tweets/{id}"), None)
            .await?;
        Ok(())
    }

    // -- Engagement ---------------------------------------------------------

    pub async fn like_tweet(&self, tweet_id: &str) -> Result<(), XmasterError> {
        let uid = self.get_authenticated_user_id().await?;
        self.request(
            Method::POST,
            &format!("{BASE}/users/{uid}/likes"),
            Some(json!({ "tweet_id": tweet_id })),
        )
        .await?;
        Ok(())
    }

    pub async fn unlike_tweet(&self, tweet_id: &str) -> Result<(), XmasterError> {
        let uid = self.get_authenticated_user_id().await?;
        self.request(
            Method::DELETE,
            &format!("{BASE}/users/{uid}/likes/{tweet_id}"),
            None,
        )
        .await?;
        Ok(())
    }

    pub async fn retweet(&self, tweet_id: &str) -> Result<(), XmasterError> {
        let uid = self.get_authenticated_user_id().await?;
        self.request(
            Method::POST,
            &format!("{BASE}/users/{uid}/retweets"),
            Some(json!({ "tweet_id": tweet_id })),
        )
        .await?;
        Ok(())
    }

    pub async fn unretweet(&self, tweet_id: &str) -> Result<(), XmasterError> {
        let uid = self.get_authenticated_user_id().await?;
        self.request(
            Method::DELETE,
            &format!("{BASE}/users/{uid}/retweets/{tweet_id}"),
            None,
        )
        .await?;
        Ok(())
    }

    pub async fn bookmark_tweet(&self, tweet_id: &str) -> Result<(), XmasterError> {
        let uid = self.get_authenticated_user_id().await?;
        self.request(
            Method::POST,
            &format!("{BASE}/users/{uid}/bookmarks"),
            Some(json!({ "tweet_id": tweet_id })),
        )
        .await?;
        Ok(())
    }

    pub async fn unbookmark_tweet(&self, tweet_id: &str) -> Result<(), XmasterError> {
        let uid = self.get_authenticated_user_id().await?;
        self.request(
            Method::DELETE,
            &format!("{BASE}/users/{uid}/bookmarks/{tweet_id}"),
            None,
        )
        .await?;
        Ok(())
    }

    // -- Follow/unfollow ----------------------------------------------------

    pub async fn follow_user(&self, target_user_id: &str) -> Result<(), XmasterError> {
        let uid = self.get_authenticated_user_id().await?;
        self.request(
            Method::POST,
            &format!("{BASE}/users/{uid}/following"),
            Some(json!({ "target_user_id": target_user_id })),
        )
        .await?;
        Ok(())
    }

    pub async fn unfollow_user(&self, target_user_id: &str) -> Result<(), XmasterError> {
        let uid = self.get_authenticated_user_id().await?;
        self.request(
            Method::DELETE,
            &format!("{BASE}/users/{uid}/following/{target_user_id}"),
            None,
        )
        .await?;
        Ok(())
    }

    // -- User lookup --------------------------------------------------------

    pub async fn get_user_by_username(&self, username: &str) -> Result<UserResponse, XmasterError> {
        let url = format!(
            "{BASE}/users/by/username/{username}?{fields}",
            fields = Self::user_fields_param()
        );
        self.request_data(Method::GET, &url, None).await
    }

    pub async fn get_user_by_id(&self, user_id: &str) -> Result<UserResponse, XmasterError> {
        let url = format!(
            "{BASE}/users/{user_id}?{fields}",
            fields = Self::user_fields_param()
        );
        self.request_data(Method::GET, &url, None).await
    }

    pub async fn get_me(&self) -> Result<UserResponse, XmasterError> {
        let url = format!("{BASE}/users/me?{fields}", fields = Self::user_fields_param());
        self.request_data(Method::GET, &url, None).await
    }

    /// Batch user lookup by username. Chunks into groups of 100 internally.
    /// Wraps GET /2/users/by?usernames=u1,u2,...
    pub async fn get_users_by_usernames(
        &self,
        usernames: &[String],
    ) -> Result<Vec<UserResponse>, XmasterError> {
        if usernames.is_empty() {
            return Ok(Vec::new());
        }
        let mut out: Vec<UserResponse> = Vec::with_capacity(usernames.len());
        for chunk in usernames.chunks(100) {
            let names = chunk.join(",");
            let url = format!(
                "{BASE}/users/by?usernames={names}&{fields}",
                fields = Self::user_fields_param()
            );
            let (users, _includes): (Vec<UserResponse>, _) =
                self.request_list::<UserResponse>(Method::GET, &url, None).await?;
            out.extend(users);
        }
        Ok(out)
    }

    // -- Tweet engagement lookups -------------------------------------------

    /// Users who liked a given tweet. Wraps GET /2/tweets/:id/liking_users.
    pub async fn get_tweet_likers(
        &self,
        tweet_id: &str,
        count: usize,
    ) -> Result<Vec<UserResponse>, XmasterError> {
        let max = count.clamp(1, 100);
        let url = format!(
            "{BASE}/tweets/{tweet_id}/liking_users?max_results={max}&{fields}",
            fields = Self::user_fields_param()
        );
        let (users, _includes) =
            self.request_list::<UserResponse>(Method::GET, &url, None).await?;
        Ok(users)
    }

    /// Users who retweeted a given tweet. Wraps GET /2/tweets/:id/retweeted_by.
    pub async fn get_tweet_retweeters(
        &self,
        tweet_id: &str,
        count: usize,
    ) -> Result<Vec<UserResponse>, XmasterError> {
        let max = count.clamp(1, 100);
        let url = format!(
            "{BASE}/tweets/{tweet_id}/retweeted_by?max_results={max}&{fields}",
            fields = Self::user_fields_param()
        );
        let (users, _includes) =
            self.request_list::<UserResponse>(Method::GET, &url, None).await?;
        Ok(users)
    }

    /// Quote tweets of a given tweet. Wraps GET /2/tweets/:id/quote_tweets.
    pub async fn get_tweet_quotes(
        &self,
        tweet_id: &str,
        count: usize,
    ) -> Result<Vec<TweetData>, XmasterError> {
        let max = count.clamp(10, 100);
        let url = format!(
            "{BASE}/tweets/{tweet_id}/quote_tweets?max_results={max}&{tf}&{exp}&{uf}",
            tf = Self::tweet_fields(),
            exp = Self::tweet_expansions(),
            uf = Self::user_fields_param(),
        );
        let (mut tweets, includes) =
            self.request_list::<TweetData>(Method::GET, &url, None).await?;
        Self::merge_authors(&mut tweets, &includes);
        Ok(tweets)
    }

    /// Members of a list. Wraps GET /2/lists/:id/members.
    pub async fn get_list_members(
        &self,
        list_id: &str,
        count: usize,
    ) -> Result<Vec<UserResponse>, XmasterError> {
        let max = count.clamp(1, 100);
        let url = format!(
            "{BASE}/lists/{list_id}/members?max_results={max}&{fields}",
            fields = Self::user_fields_param()
        );
        let (users, _includes) =
            self.request_list::<UserResponse>(Method::GET, &url, None).await?;
        Ok(users)
    }

    // -- Timelines ----------------------------------------------------------

    #[allow(dead_code)]
    pub async fn get_user_tweets(
        &self,
        user_id: &str,
        count: usize,
    ) -> Result<Vec<TweetData>, XmasterError> {
        self.get_user_tweets_paginated(user_id, count, None, None).await
    }

    pub async fn get_user_mentions(
        &self,
        user_id: &str,
        count: usize,
    ) -> Result<Vec<TweetData>, XmasterError> {
        self.get_user_mentions_since(user_id, count, None).await
    }

    pub async fn get_user_mentions_since(
        &self,
        user_id: &str,
        count: usize,
        since_id: Option<&str>,
    ) -> Result<Vec<TweetData>, XmasterError> {
        let max = count.clamp(5, 100);
        let since_param = since_id
            .map(|id| format!("&since_id={id}"))
            .unwrap_or_default();
        let url = format!(
            "{BASE}/users/{user_id}/mentions?max_results={max}&{tf}&{exp}&{uf}{since_param}",
            tf = Self::tweet_fields(),
            exp = Self::tweet_expansions(),
            uf = Self::user_fields_param(),
        );
        let (mut tweets, includes) =
            self.request_list::<TweetData>(Method::GET, &url, None).await?;
        Self::merge_authors(&mut tweets, &includes);
        Ok(tweets)
    }

    pub async fn get_home_timeline(
        &self,
        count: usize,
    ) -> Result<Vec<TweetData>, XmasterError> {
        let user_id = self.get_authenticated_user_id().await?;
        let max = count.clamp(1, 100);
        let url = format!(
            "{BASE}/users/{user_id}/reverse_chronological_timeline?max_results={max}&{tf}&{exp}&{uf}",
            tf = Self::tweet_fields(),
            exp = Self::tweet_expansions(),
            uf = Self::user_fields_param(),
        );
        let (mut tweets, includes) =
            self.request_list::<TweetData>(Method::GET, &url, None).await?;
        Self::merge_authors(&mut tweets, &includes);
        Ok(tweets)
    }

    // -- Followers/following ------------------------------------------------

    pub async fn get_user_followers(
        &self,
        user_id: &str,
        count: usize,
    ) -> Result<Vec<UserData>, XmasterError> {
        let max = count.clamp(1, 1000);
        let url = format!(
            "{BASE}/users/{user_id}/followers?max_results={max}&{uf}",
            uf = Self::user_fields_param(),
        );
        let (users, _) = self.request_list::<UserData>(Method::GET, &url, None).await?;
        Ok(users)
    }

    pub async fn get_user_following(
        &self,
        user_id: &str,
        count: usize,
    ) -> Result<Vec<UserData>, XmasterError> {
        let max = count.clamp(1, 1000);
        let url = format!(
            "{BASE}/users/{user_id}/following?max_results={max}&{uf}",
            uf = Self::user_fields_param(),
        );
        let (users, _) = self.request_list::<UserData>(Method::GET, &url, None).await?;
        Ok(users)
    }

    // -- Search -------------------------------------------------------------

    pub async fn search_tweets(
        &self,
        query: &str,
        mode: &str,
        count: usize,
    ) -> Result<Vec<TweetData>, XmasterError> {
        self.search_tweets_paginated(query, mode, count, None, None).await
    }

    pub async fn search_tweets_paginated(
        &self,
        query: &str,
        mode: &str,
        count: usize,
        start_time: Option<&str>,
        end_time: Option<&str>,
    ) -> Result<Vec<TweetData>, XmasterError> {
        let encoded_query = percent_encoding::utf8_percent_encode(
            query,
            percent_encoding::NON_ALPHANUMERIC,
        );
        let sort = match mode {
            "relevancy" | "relevant" => "relevancy",
            _ => "recency",
        };
        let mut time_params = String::new();
        if let Some(st) = start_time {
            time_params.push_str(&format!("&start_time={st}"));
        }
        if let Some(et) = end_time {
            time_params.push_str(&format!("&end_time={et}"));
        }

        let mut all_tweets = Vec::new();
        let mut next_token: Option<String> = None;
        let max_pages = (count / 100).max(1) + 1;

        for _ in 0..max_pages {
            let remaining = count - all_tweets.len();
            if remaining == 0 { break; }
            let page_size = remaining.clamp(10, 100);

            let mut url = format!(
                "{BASE}/tweets/search/recent?query={encoded_query}&max_results={page_size}&sort_order={sort}&{tf}&{exp}&{uf}{time_params}",
                tf = Self::tweet_fields(),
                exp = Self::tweet_expansions(),
                uf = Self::user_fields_param(),
            );
            if let Some(ref token) = next_token {
                url.push_str(&format!("&pagination_token={token}"));
            }

            let val = self.request(Method::GET, &url, None).await?;
            let includes = val.get("includes").cloned();
            let envelope: ApiResponse<Vec<TweetData>> = serde_json::from_value(val)?;
            let mut tweets = envelope.data.unwrap_or_default();
            Self::merge_authors(&mut tweets, &includes);
            all_tweets.extend(tweets);

            next_token = envelope.meta.and_then(|m| m.next_token);
            if next_token.is_none() { break; }
        }

        all_tweets.truncate(count);
        Ok(all_tweets)
    }

    pub async fn get_user_tweets_paginated(
        &self,
        user_id: &str,
        count: usize,
        start_time: Option<&str>,
        end_time: Option<&str>,
    ) -> Result<Vec<TweetData>, XmasterError> {
        let mut time_params = String::new();
        if let Some(st) = start_time {
            time_params.push_str(&format!("&start_time={st}"));
        }
        if let Some(et) = end_time {
            time_params.push_str(&format!("&end_time={et}"));
        }

        let mut all_tweets = Vec::new();
        let mut next_token: Option<String> = None;
        let max_pages = (count / 100).max(1) + 1;

        for _ in 0..max_pages {
            let remaining = count - all_tweets.len();
            if remaining == 0 { break; }
            let page_size = remaining.clamp(5, 100);

            let mut url = format!(
                "{BASE}/users/{user_id}/tweets?max_results={page_size}&{tf}&{exp}&{uf}{time_params}",
                tf = Self::tweet_fields(),
                exp = Self::tweet_expansions(),
                uf = Self::user_fields_param(),
            );
            if let Some(ref token) = next_token {
                url.push_str(&format!("&pagination_token={token}"));
            }

            let val = self.request(Method::GET, &url, None).await?;
            let includes = val.get("includes").cloned();
            let envelope: ApiResponse<Vec<TweetData>> = serde_json::from_value(val)?;
            let mut tweets = envelope.data.unwrap_or_default();
            Self::merge_authors(&mut tweets, &includes);
            all_tweets.extend(tweets);

            next_token = envelope.meta.and_then(|m| m.next_token);
            if next_token.is_none() { break; }
        }

        all_tweets.truncate(count);
        Ok(all_tweets)
    }

    // -- Direct Messages ----------------------------------------------------

    pub async fn send_dm(
        &self,
        participant_id: &str,
        text: &str,
    ) -> Result<(), XmasterError> {
        self.request(
            Method::POST,
            &format!("{BASE}/dm_conversations/with/{participant_id}/messages"),
            Some(json!({ "text": text })),
        )
        .await?;
        Ok(())
    }

    pub async fn get_dm_conversations(
        &self,
        count: usize,
    ) -> Result<Vec<DmConversation>, XmasterError> {
        let max = count.clamp(1, 100);
        let url = format!(
            "{BASE}/dm_events?max_results={max}&event_types=MessageCreate&dm_event.fields=id,text,sender_id,created_at,dm_conversation_id,participant_ids"
        );
        let val = self.request(Method::GET, &url, None).await?;

        // DM events don't directly list conversations — we extract unique conversation IDs
        let events = val
            .get("data")
            .and_then(|d| d.as_array())
            .cloned()
            .unwrap_or_default();

        let mut seen = std::collections::HashSet::new();
        let mut convos = Vec::new();

        for event in &events {
            if let Some(cid) = event.get("dm_conversation_id").and_then(|c| c.as_str()) {
                if seen.insert(cid.to_string()) {
                    let participant_ids = event
                        .get("participant_ids")
                        .and_then(|p| p.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|v| v.as_str().map(String::from))
                                .collect()
                        })
                        .unwrap_or_default();
                    convos.push(DmConversation {
                        id: cid.to_string(),
                        participant_ids,
                    });
                }
            }
        }

        Ok(convos)
    }

    pub async fn get_dm_messages(
        &self,
        conversation_id: &str,
        count: usize,
    ) -> Result<Vec<DmMessage>, XmasterError> {
        let max = count.clamp(1, 100);
        let url = format!(
            "{BASE}/dm_conversations/{conversation_id}/dm_events?max_results={max}&event_types=MessageCreate&dm_event.fields=id,text,sender_id,created_at"
        );
        let val = self.request(Method::GET, &url, None).await?;

        let events = val
            .get("data")
            .and_then(|d| d.as_array())
            .cloned()
            .unwrap_or_default();

        let messages: Vec<DmMessage> = events
            .into_iter()
            .filter_map(|e| serde_json::from_value(e).ok())
            .collect();

        Ok(messages)
    }

    // -- Media upload -------------------------------------------------------

    pub async fn upload_media(&self, file_path: &str) -> Result<String, XmasterError> {
        self.require_auth()?;

        let path = Path::new(file_path);
        if !path.exists() {
            return Err(XmasterError::Media(format!("File not found: {file_path}")));
        }

        let file_bytes = tokio::fs::read(path).await?;
        let file_name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "media".into());

        let mime = match path.extension().and_then(|e| e.to_str()) {
            Some("png") => "image/png",
            Some("jpg" | "jpeg") => "image/jpeg",
            Some("gif") => "image/gif",
            Some("webp") => "image/webp",
            Some("mp4") => "video/mp4",
            Some("mov") => "video/quicktime",
            _ => "application/octet-stream",
        };

        let is_video = mime.starts_with("video/");
        let category = if is_video { "tweet_video" } else { "tweet_image" };

        // Validate file size against X upload limits
        let max_size = if is_video {
            512 * 1024 * 1024
        } else if mime == "image/gif" {
            15 * 1024 * 1024
        } else {
            5 * 1024 * 1024
        };
        if file_bytes.len() > max_size {
            return Err(XmasterError::Media(format!(
                "File too large: {}MB (max {}MB for {})",
                file_bytes.len() / 1024 / 1024,
                max_size / 1024 / 1024,
                if is_video { "video" } else { "image" },
            )));
        }

        // For small images, use simple upload
        if !is_video && file_bytes.len() < 5_000_000 {
            return self.simple_upload(&file_bytes, &file_name).await;
        }

        // Chunked upload: INIT → APPEND → FINALIZE
        self.chunked_upload(&file_bytes, mime, category).await
    }

    async fn simple_upload(
        &self,
        data: &[u8],
        file_name: &str,
    ) -> Result<String, XmasterError> {
        let part = reqwest::multipart::Part::bytes(data.to_vec())
            .file_name(file_name.to_string());
        let form = reqwest::multipart::Form::new().part("media", part);

        let resp = self.ctx.client.clone().oauth1(self.secrets())
            .post(UPLOAD_URL)
            .multipart(form)
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(XmasterError::Media(format!(
                "Upload failed (HTTP {status}): {text}"
            )));
        }

        let upload: MediaUploadResponse = resp.json().await?;
        upload
            .media_id_string
            .or_else(|| upload.media_id.map(|id| id.to_string()))
            .ok_or_else(|| XmasterError::Media("No media_id in upload response".into()))
    }

    async fn chunked_upload(
        &self,
        data: &[u8],
        mime: &str,
        category: &str,
    ) -> Result<String, XmasterError> {
        // INIT
        let total = data.len().to_string();
        let resp = self.ctx.client.clone().oauth1(self.secrets())
            .post(UPLOAD_URL)
            .form(&[
                ("command", "INIT"),
                ("media_type", mime),
                ("total_bytes", total.as_str()),
                ("media_category", category),
            ])
            .send()
            .await?;

        if !resp.status().is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(XmasterError::Media(format!("INIT failed: {text}")));
        }

        let init_resp: MediaUploadResponse = resp.json().await?;
        let media_id = init_resp
            .media_id_string
            .or_else(|| init_resp.media_id.map(|id| id.to_string()))
            .ok_or_else(|| XmasterError::Media("No media_id from INIT".into()))?;

        // APPEND in 1MB chunks
        let chunk_size = 1024 * 1024;
        let total_chunks = data.len().div_ceil(chunk_size);
        for (i, chunk) in data.chunks(chunk_size).enumerate() {
            if data.len() > 5_000_000 {
                eprintln!("  Uploading chunk {}/{} ...", i + 1, total_chunks);
            }
            let b64_chunk = base64::engine::general_purpose::STANDARD.encode(chunk);
            let seg = i.to_string();

            let resp = self.ctx.client.clone().oauth1(self.secrets())
                .post(UPLOAD_URL)
                .form(&[
                    ("command", "APPEND"),
                    ("media_id", media_id.as_str()),
                    ("segment_index", seg.as_str()),
                    ("media_data", b64_chunk.as_str()),
                ])
                .send()
                .await?;

            if !resp.status().is_success() {
                let text = resp.text().await.unwrap_or_default();
                return Err(XmasterError::Media(format!(
                    "APPEND segment {i} failed: {text}"
                )));
            }
        }

        // FINALIZE
        let resp = self.ctx.client.clone().oauth1(self.secrets())
            .post(UPLOAD_URL)
            .form(&[("command", "FINALIZE"), ("media_id", media_id.as_str())])
            .send()
            .await?;

        if !resp.status().is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(XmasterError::Media(format!("FINALIZE failed: {text}")));
        }

        // For video, poll processing_info until complete
        let finalize: Value = resp.json().await?;
        if let Some(info) = finalize.get("processing_info") {
            self.wait_for_processing(&media_id, info).await?;
        }

        Ok(media_id)
    }

    async fn wait_for_processing(
        &self,
        media_id: &str,
        initial_info: &Value,
    ) -> Result<(), XmasterError> {
        let mut check_after = initial_info
            .get("check_after_secs")
            .and_then(|v| v.as_u64())
            .unwrap_or(5);

        const MAX_RETRIES: u32 = 30;
        const MAX_TOTAL_SECS: u64 = 300; // 5 minutes
        let mut attempts = 0u32;
        let mut elapsed_secs = 0u64;

        loop {
            tokio::time::sleep(std::time::Duration::from_secs(check_after)).await;
            elapsed_secs += check_after;
            attempts += 1;

            if attempts > MAX_RETRIES || elapsed_secs > MAX_TOTAL_SECS {
                return Err(XmasterError::Media(
                    "Upload processing timed out".into(),
                ));
            }

            let url = format!("{UPLOAD_URL}?command=STATUS&media_id={media_id}");

            let resp = self.ctx.client.clone().oauth1(self.secrets()).get(&url).send().await?;

            if !resp.status().is_success() {
                let text = resp.text().await.unwrap_or_default();
                return Err(XmasterError::Media(format!(
                    "STATUS check failed: {text}"
                )));
            }

            let status: Value = resp.json().await?;
            let state = status
                .get("processing_info")
                .and_then(|p| p.get("state"))
                .and_then(|s| s.as_str())
                .unwrap_or("succeeded");

            match state {
                "succeeded" => return Ok(()),
                "failed" => {
                    let error = status
                        .get("processing_info")
                        .and_then(|p| p.get("error"))
                        .and_then(|e| e.get("message"))
                        .and_then(|m| m.as_str())
                        .unwrap_or("Unknown processing error");
                    return Err(XmasterError::Media(format!(
                        "Media processing failed: {error}"
                    )));
                }
                _ => {
                    check_after = status
                        .get("processing_info")
                        .and_then(|p| p.get("check_after_secs"))
                        .and_then(|v| v.as_u64())
                        .unwrap_or(5);
                }
            }
        }
    }

}
