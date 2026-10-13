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

use super::web::Web;

/// Max retry attempts for transient errors (429, 5xx).
const MAX_RETRIES: u32 = 3;

const BASE: &str = "https://api.x.com/2";
const UPLOAD_URL: &str = "https://upload.twitter.com/1.1/media/upload.json";

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
pub struct ArticleEntityResponse {
    pub id: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub tweet_id: Option<String>,
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
pub struct TrendData {
    pub name: String,
    #[serde(default)]
    pub tweet_count: Option<u64>,
    #[serde(default)]
    pub description: Option<String>,
}

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

fn parse_article_entity(
    result: &Value,
    code: &'static str,
) -> Result<ArticleEntityResponse, XmasterError> {
    let id = result
        .get("rest_id")
        .or_else(|| result.get("id"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| XmasterError::Api {
            provider: "x-web",
            code,
            message: format!(
                "No Article entity id in response: {}",
                crate::utils::safe_truncate(&result.to_string(), 300)
            ),
        })?;

    let title = result
        .get("title")
        .or_else(|| result.pointer("/metadata/title"))
        .and_then(|v| v.as_str())
        .map(ToString::to_string);

    let tweet_id = result
        .pointer("/metadata/tweet_results/result/rest_id")
        .or_else(|| result.pointer("/tweet_results/result/rest_id"))
        .and_then(|v| v.as_str())
        .map(ToString::to_string);

    Ok(ArticleEntityResponse {
        id: id.to_string(),
        title,
        tweet_id,
    })
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
    ///
    /// Public so command files can avoid duplicating OAuth1 boilerplate.
    /// Prefer higher-level helpers (`request_data`, `request_list`) when
    /// deserializing into known types.
    pub async fn request(
        &self,
        method: Method,
        url: &str,
        body: Option<Value>,
    ) -> Result<Value, XmasterError> {
        let mut last_err: Option<XmasterError> = None;

        for attempt in 0..MAX_RETRIES {
            match self.request_once(method.clone(), url, body.clone()).await {
                Ok(val) => return Ok(val),
                // A 5xx on a POST may come after X already created the post:
                // retrying risks a duplicate. Only 429 (not processed) retries.
                Err(e @ XmasterError::ServerError { .. }) if method == Method::POST => return Err(e),
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
                            // Long windows: hand the reset time to the caller
                            // (exit 4) instead of stalling an agent for minutes.
                            if reset_at.saturating_sub(now) > 15 {
                                return Err(e);
                            }
                            if *reset_at > now {
                                // Keep jitter in millis — integer-dividing it
                                // into seconds always yielded 0, so concurrent
                                // clients woke in lockstep on the same reset.
                                let wait = *reset_at - now;
                                delay = Duration::from_millis(wait * 1000 + jitter_ms);
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

        // Capture x-transaction-id for error diagnostics (X returns this on
        // every response; it's the only way to correlate with X support).
        let txn_id = resp
            .headers()
            .get("x-transaction-id")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());

        if status == 401 || status == 403 {
            let text = resp.text().await.unwrap_or_default();
            let txn_suffix = txn_id
                .as_deref()
                .map(|id| format!(" [txn: {id}]"))
                .unwrap_or_default();
            let message = if text.contains("oauth1-permissions") {
                format!(
                    "HTTP {status} Forbidden: {text}. \
                    Fix: Your Access Token was likely generated before enabling Read+Write. \
                    Go to developer.x.com → your app → Keys and tokens → Regenerate Access Token and Secret, \
                    then run: xmaster config set keys.access_token NEW_TOKEN && \
                    xmaster config set keys.access_token_secret NEW_SECRET"
                )
            } else {
                format!("HTTP {status}: {text}{txn_suffix}")
            };
            return Err(XmasterError::AuthMissing {
                provider: "x",
                message,
            });
        }

        // 402: pay-per-use credits exhausted, or the endpoint needs a paid add-on
        // (X API moved to credit-based pricing on 2026-02-06).
        if status == 402 {
            let text = resp.text().await.unwrap_or_default();
            let txn_suffix = txn_id
                .as_deref()
                .map(|id| format!(" [txn: {id}]"))
                .unwrap_or_default();
            return Err(XmasterError::PaymentRequired {
                message: format!("HTTP 402: {text}{txn_suffix}"),
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
            let txn_suffix = txn_id
                .as_deref()
                .map(|id| format!(" [txn: {id}]"))
                .unwrap_or_default();
            return Err(XmasterError::Api {
                provider: "x",
                code: "api_error",
                message: format!("HTTP {status}: {msg}{txn_suffix}"),
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

        // Empty response body (e.g. home timeline on free tier, or an
        // account with no eligible feed) deserializes to Value::Null. Treat
        // that as an empty result rather than a JSON shape error.
        if val.is_null() {
            return Ok((Vec::new(), None));
        }

        // Grab includes before consuming val
        let includes = val.get("includes").cloned();

        let envelope: ApiResponse<Vec<T>> = serde_json::from_value(val)?;
        Ok((envelope.data.unwrap_or_default(), includes))
    }

    // -- Cached user ID -----------------------------------------------------

    pub async fn get_authenticated_user_id(&self) -> Result<String, XmasterError> {
        self.cached_user_id
            .get_or_try_init(|| async {
                if self.ctx.config.web_reads() {
                    match Web::new(&self.ctx)?.viewer_id().await {
                        Ok(id) => return Ok(id),
                        Err(e) => self.web_read_failed("user ID lookup", e)?,
                    }
                }
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

    /// A failed free web read falls back to the paid API, unless there is no
    /// API configured to fall back to.
    fn web_read_failed(&self, what: &str, e: XmasterError) -> Result<(), XmasterError> {
        if !self.ctx.config.has_x_auth() {
            return Err(e);
        }
        warn!("Web {what} failed ({e}); falling back to the paid API");
        Ok(())
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

    /// GET a tweet-listing endpoint and merge author/media includes.
    async fn get_tweet_list(&self, url: &str) -> Result<Vec<TweetData>, XmasterError> {
        let (mut tweets, includes) =
            self.request_list::<TweetData>(Method::GET, url, None).await?;
        Self::merge_authors(&mut tweets, &includes);
        Ok(tweets)
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

        // Writes the API blocks or can't do go through the browser session
        // first (settings.web_writes): replies to people who never @mentioned
        // you (403 since Feb 2026), quotes (removed 2026-04-20) and >280-char
        // Premium posts (CreateNoteTweet). Polls stay on the API.
        let long_premium = text.chars().count() > 280 && self.ctx.config.account.premium;
        let api_only = poll_options.is_some_and(|o| !o.is_empty());
        let wants_web = reply_to.is_some() || quote_tweet_id.is_some() || long_premium;
        let mut web_err = None;
        if !api_only && self.ctx.config.web_first(wants_web) {
            match Web::new(&self.ctx)?
                .create_tweet(text, reply_to, quote_tweet_id, media_ids, long_premium)
                .await
            {
                Ok((id, text)) => return Ok(TweetResponse { id, text }),
                // Quotes and long posts have no API path to fall back to
                Err(e) if quote_tweet_id.is_some() || long_premium => return Err(e),
                // X may have accepted it: a second path would double-post
                Err(e) if Self::maybe_delivered(&e) => {
                    return Err(XmasterError::Api {
                        provider: "x-web",
                        code: "write_unconfirmed",
                        message: format!(
                            "{e}. The post may have gone through: check your profile \
                            before retrying"
                        ),
                    })
                }
                Err(e) => {
                    warn!("Web post failed ({e}); trying the API");
                    web_err = Some(e);
                }
            }
        }

        let result = self
            .request_data(Method::POST, &format!("{BASE}/tweets"), Some(body))
            .await;
        match result {
            Err(ref err) if wants_web && Self::is_write_restricted(err) => Err(web_err.unwrap_or_else(|| {
                XmasterError::ReplyRestricted(
                    "X blocks API replies to people who haven't @mentioned you, removed API \
                    quote-posting (2026-04-20), and rejects >280-char posts on /2/tweets. \
                    Run `xmaster config web-login` so these go through your browser session."
                        .into(),
                )
            })),
            other => other,
        }
    }

    /// Could X have accepted a write despite this error (timeout after send,
    /// 5xx, a 200 we couldn't read)? Then trying another path risks a duplicate.
    fn maybe_delivered(err: &XmasterError) -> bool {
        match err {
            XmasterError::Http(e) => !e.is_connect() && !e.is_builder(),
            XmasterError::ServerError { .. } => true,
            XmasterError::Api { code, message, .. } => {
                matches!(*code, "no_tweet_result" | "no_rest_id") || message.contains("(HTTP 5")
            }
            _ => false,
        }
    }

    /// Detect whether an error is an X write restriction: the Feb 2026 reply
    /// restriction (403 on replies to non-mentioners) or the Apr 2026 removal
    /// of quote-posting from self-serve tiers.
    /// Note: oauth1-permissions 403s are handled separately in request_once() with
    /// a specific regeneration hint, so they won't reach here. Any 403 that makes
    /// it to this check is a non-permission 403, likely a write restriction.
    fn is_write_restricted(err: &XmasterError) -> bool {
        match err {
            XmasterError::AuthMissing { message, .. } => {
                message.contains("403") && !message.contains("oauth1-permissions")
            }
            XmasterError::Api { message, .. } => {
                message.contains("403")
                    || message.contains("reply")
                    || message.contains("not allowed")
                    || message.contains("not permitted")
                    || message.contains("280")
            }
            _ => false,
        }
    }

    /// Create an Article draft via the official X API v2 endpoint
    /// (`POST /2/articles/draft`, added 2026-06-11). Unlike the web path this
    /// needs no browser cookies — it is signed with the normal OAuth1 keys.
    pub async fn create_article_draft_v2(
        &self,
        title: &str,
        content_state: Value,
    ) -> Result<ArticleEntityResponse, XmasterError> {
        let val = self
            .request(
                Method::POST,
                &format!("{BASE}/articles/draft"),
                Some(json!({ "title": title, "content_state": content_state })),
            )
            .await?;

        let id = val
            .pointer("/data/id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| XmasterError::Api {
                provider: "x",
                code: "no_article_id",
                message: format!(
                    "Unexpected Articles API draft response: {}",
                    crate::utils::safe_truncate(&val.to_string(), 300)
                ),
            })?;

        Ok(ArticleEntityResponse {
            id: id.to_string(),
            title: val
                .pointer("/data/title")
                .and_then(|v| v.as_str())
                .map(str::to_string),
            tweet_id: None,
        })
    }

    /// Publish a draft Article via the official X API v2 endpoint
    /// (`POST /2/articles/{id}/publish`). Returns the ID of the post X
    /// creates for the published Article. This makes the Article PUBLIC.
    pub async fn publish_article(&self, article_id: &str) -> Result<String, XmasterError> {
        let val = self
            .request(
                Method::POST,
                &format!("{BASE}/articles/{article_id}/publish"),
                None,
            )
            .await?;

        val.pointer("/data/post_id")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .ok_or_else(|| XmasterError::Api {
                provider: "x",
                code: "no_post_id",
                message: format!(
                    "Unexpected Articles API publish response: {}",
                    crate::utils::safe_truncate(&val.to_string(), 300)
                ),
            })
    }

    pub async fn create_article_draft(
        &self,
        title: &str,
        content_state: Value,
    ) -> Result<ArticleEntityResponse, XmasterError> {
        let val = self
            .article_graphql(
                "ArticleEntityDraftCreate",
                json!({
                    "content_state": content_state,
                    "title": title,
                }),
            )
            .await?;

        let result = val
            .pointer("/data/articleentity_create_draft/article_entity_results/result")
            .ok_or_else(|| XmasterError::Api {
                provider: "x-web",
                code: "no_article_result",
                message: format!(
                    "Unexpected Article draft response: {}",
                    crate::utils::safe_truncate(&val.to_string(), 300)
                ),
            })?;

        parse_article_entity(result, "articleentity_create_draft")
    }

    pub async fn update_article_cover_media(
        &self,
        article_entity_id: &str,
        media_id: Option<&str>,
        media_category: Option<&str>,
    ) -> Result<ArticleEntityResponse, XmasterError> {
        let cover_media = match (media_id, media_category) {
            (Some(media_id), Some(media_category)) => {
                json!({ "media_id": media_id, "media_category": media_category })
            }
            _ => Value::Null,
        };

        let val = self
            .article_graphql(
                "ArticleEntityUpdateCoverMedia",
                json!({
                    "articleEntityId": article_entity_id,
                    "coverMedia": cover_media,
                }),
            )
            .await?;

        let result = val
            .pointer("/data/articleentity_update_cover_media")
            .ok_or_else(|| XmasterError::Api {
                provider: "x-web",
                code: "no_article_result",
                message: format!(
                    "Unexpected Article cover response: {}",
                    crate::utils::safe_truncate(&val.to_string(), 300)
                ),
            })?;

        parse_article_entity(result, "articleentity_update_cover_media")
    }

    /// Article entity mutations exist only on the web (private GraphQL).
    async fn article_graphql(&self, op: &str, variables: Value) -> Result<Value, XmasterError> {
        let features = json!({
            "profile_label_improvements_pcf_label_in_post_enabled": true,
            "responsive_web_profile_redirect_enabled": false,
            "rweb_tipjar_consumption_enabled": true,
            "verified_phone_label_enabled": false,
            "responsive_web_graphql_skip_user_profile_image_extensions_enabled": false,
            "responsive_web_graphql_timeline_navigation_enabled": true,
        });
        let toggles = json!({ "withPayments": true, "withAuxiliaryUserLabels": true });
        Web::new(&self.ctx)?
            .graphql(op, variables, Some(features), Some(toggles), "https://x.com/compose/articles")
            .await
    }

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
        Self::merge_authors(std::slice::from_mut(&mut tweet), &includes);
        Ok(tweet)
    }

    /// Direct replies to a post. The web session reads the post's
    /// conversation view (free, any age); the API fallback searches the last
    /// 7 days.
    ///
    /// API path: the conversation_id of a reply equals the root tweet of the
    /// thread, not the reply's own ID. So we first fetch the target tweet to
    /// learn its conversation_id, search the whole conversation, then filter
    /// to only direct replies to the target tweet.
    pub async fn get_replies(
        &self,
        tweet_id: &str,
        count: usize,
    ) -> Result<Vec<TweetData>, XmasterError> {
        if self.ctx.config.web_reads() {
            match Web::new(&self.ctx)?.replies(tweet_id, count).await {
                Ok(t) => return Ok(t),
                Err(e) => self.web_read_failed("replies read", e)?,
            }
        }
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

    /// POST a single-id body to the authenticated user's `{collection}`.
    async fn add_to_collection(
        &self,
        collection: &str,
        body: serde_json::Value,
    ) -> Result<(), XmasterError> {
        let uid = self.get_authenticated_user_id().await?;
        self.request(
            Method::POST,
            &format!("{BASE}/users/{uid}/{collection}"),
            Some(body),
        )
        .await
        .map_err(|e| Self::annotate_removed_write(collection, e))?;
        Ok(())
    }

    /// DELETE `{id}` from the authenticated user's `{collection}`.
    async fn remove_from_collection(&self, collection: &str, id: &str) -> Result<(), XmasterError> {
        let uid = self.get_authenticated_user_id().await?;
        self.request(
            Method::DELETE,
            &format!("{BASE}/users/{uid}/{collection}/{id}"),
            None,
        )
        .await
        .map_err(|e| Self::annotate_removed_write(collection, e))?;
        Ok(())
    }

    /// X removed Likes and Follows writes from all self-serve API tiers on
    /// 2026-04-20 (Enterprise keeps them). A 403 on those collections is the
    /// platform restriction, not a token problem — say so instead of sending
    /// the user off to regenerate credentials.
    fn annotate_removed_write(collection: &str, err: XmasterError) -> XmasterError {
        if !matches!(collection, "likes" | "following") {
            return err;
        }
        match err {
            XmasterError::AuthMissing { provider, message } if message.contains("403") => {
                XmasterError::AuthMissing {
                    provider,
                    message: format!(
                        "{message}. Note: X removed Likes and Follows from all self-serve \
                        API tiers on 2026-04-20, so no credential change will fix this 403. \
                        Run `xmaster config web-login` (and keep settings.web_writes at \
                        auto) so likes and follows go through your browser session."
                    ),
                }
            }
            other => other,
        }
    }

    // Likes and follows left the self-serve API on 2026-04-20; reposts and
    // bookmarks are free on the web. All go through the browser session first
    // (settings.web_writes); reposts and bookmarks fall back to the API.

    pub async fn like_tweet(&self, tweet_id: &str) -> Result<(), XmasterError> {
        if self.ctx.config.web_first(true) {
            return Web::new(&self.ctx)?.like(tweet_id).await;
        }
        self.add_to_collection("likes", json!({ "tweet_id": tweet_id }))
            .await
    }

    pub async fn unlike_tweet(&self, tweet_id: &str) -> Result<(), XmasterError> {
        if self.ctx.config.web_first(true) {
            return Web::new(&self.ctx)?.unlike(tweet_id).await;
        }
        self.remove_from_collection("likes", tweet_id).await
    }

    pub async fn retweet(&self, tweet_id: &str) -> Result<(), XmasterError> {
        if self.ctx.config.web_first(true) {
            match Web::new(&self.ctx)?.retweet(tweet_id).await {
                Ok(()) => return Ok(()),
                Err(e) => warn!("Web repost failed ({e}); trying the API"),
            }
        }
        self.add_to_collection("retweets", json!({ "tweet_id": tweet_id }))
            .await
    }

    pub async fn unretweet(&self, tweet_id: &str) -> Result<(), XmasterError> {
        if self.ctx.config.web_first(true) {
            match Web::new(&self.ctx)?.unretweet(tweet_id).await {
                Ok(()) => return Ok(()),
                Err(e) => warn!("Web undo-repost failed ({e}); trying the API"),
            }
        }
        self.remove_from_collection("retweets", tweet_id).await
    }

    pub async fn bookmark_tweet(&self, tweet_id: &str) -> Result<(), XmasterError> {
        if self.ctx.config.web_first(true) {
            match Web::new(&self.ctx)?.bookmark(tweet_id).await {
                Ok(()) => return Ok(()),
                Err(e) => warn!("Web bookmark failed ({e}); trying the API"),
            }
        }
        self.add_to_collection("bookmarks", json!({ "tweet_id": tweet_id }))
            .await
    }

    pub async fn unbookmark_tweet(&self, tweet_id: &str) -> Result<(), XmasterError> {
        if self.ctx.config.web_first(true) {
            match Web::new(&self.ctx)?.unbookmark(tweet_id).await {
                Ok(()) => return Ok(()),
                Err(e) => warn!("Web unbookmark failed ({e}); trying the API"),
            }
        }
        self.remove_from_collection("bookmarks", tweet_id).await
    }

    // -- Follow/unfollow ----------------------------------------------------

    /// Follow by handle. The web path needs no paid user-ID lookup.
    pub async fn follow_username(&self, username: &str) -> Result<(), XmasterError> {
        if self.ctx.config.web_first(true) {
            return Web::new(&self.ctx)?.follow(username).await;
        }
        let user = self.get_user_by_username(username).await?;
        self.add_to_collection("following", json!({ "target_user_id": user.id }))
            .await
    }

    pub async fn unfollow_username(&self, username: &str) -> Result<(), XmasterError> {
        if self.ctx.config.web_first(true) {
            return Web::new(&self.ctx)?.unfollow(username).await;
        }
        let user = self.get_user_by_username(username).await?;
        self.remove_from_collection("following", &user.id).await
    }

    // -- User lookup --------------------------------------------------------

    /// Username → profile. Tries FxTwitter first (free); the API lookup is
    /// the fallback. Every command that resolves a handle goes through here.
    pub async fn get_user_by_username(&self, username: &str) -> Result<UserResponse, XmasterError> {
        let username = username.trim_start_matches('@');
        if !self.ctx.config.settings.disable_fxtwitter {
            if let Some(user) = super::fxtwitter::fetch_user(username).await {
                return Ok(user);
            }
        }
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
        self.get_tweet_list(&url).await
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
        // The web Mentions tab is the logged-in account's own
        if self.ctx.config.web_reads() {
            let web = Web::new(&self.ctx)?;
            if web.viewer_id().await.is_ok_and(|id| id == user_id) {
                match web.mentions(count, since_id).await {
                    Ok(t) => return Ok(t),
                    Err(e) => self.web_read_failed("mentions read", e)?,
                }
            }
        }
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
        self.get_tweet_list(&url).await
    }

    pub async fn get_home_timeline(
        &self,
        count: usize,
        start_time: Option<&str>,
        end_time: Option<&str>,
    ) -> Result<Vec<TweetData>, XmasterError> {
        if self.ctx.config.web_reads() {
            match Web::new(&self.ctx)?.home_latest(count, start_time, end_time).await {
                Ok(t) => return Ok(t),
                Err(e) => self.web_read_failed("home timeline read", e)?,
            }
        }
        let user_id = self.get_authenticated_user_id().await?;
        let max = count.clamp(1, 100);
        let time_params = Self::time_params(start_time, end_time);
        let url = format!(
            "{BASE}/users/{user_id}/timelines/reverse_chronological?max_results={max}&{tf}&{exp}&{uf}{time_params}",
            tf = Self::tweet_fields(),
            exp = Self::tweet_expansions(),
            uf = Self::user_fields_param(),
        );
        self.get_tweet_list(&url).await
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

    /// Get tweets by other users that repost the authenticated user's content.
    /// Wraps GET /2/users/reposts_of_me (user-context endpoint, no :id in path).
    pub async fn get_reposts_of_me(
        &self,
        count: usize,
    ) -> Result<Vec<TweetData>, XmasterError> {
        let max = count.clamp(10, 100);
        let url = format!(
            "{BASE}/users/reposts_of_me?max_results={max}&{tf}&{exp}&{uf}",
            tf = Self::tweet_fields(),
            exp = Self::tweet_expansions(),
            uf = Self::user_fields_param(),
        );
        self.get_tweet_list(&url).await
    }

    /// Personalized trends for the authenticated user.
    /// Wraps GET /2/users/personalized_trends (requires X Premium).
    pub async fn get_personalized_trends(
        &self,
    ) -> Result<Vec<TrendData>, XmasterError> {
        let url = format!("{BASE}/users/personalized_trends");
        let val = self.request(Method::GET, &url, None).await?;
        let trends = val
            .get("data")
            .and_then(|d| d.as_array())
            .cloned()
            .unwrap_or_default();
        let parsed: Vec<TrendData> = trends
            .into_iter()
            .filter_map(|t| {
                Some(TrendData {
                    name: t.get("trend_name").or_else(|| t.get("name"))?.as_str()?.to_string(),
                    tweet_count: t.get("post_count").and_then(|v| v.as_u64()),
                    description: t.get("category").and_then(|v| v.as_str()).map(|s| s.to_string()),
                })
            })
            .collect();
        Ok(parsed)
    }

    // -- Search -------------------------------------------------------------

    // Note: GET /2/tweets/counts/recent requires OAuth 2.0 App-Only auth
    // and is called directly from src/commands/volume.rs using oauth2_get.
    // Not routed through XApi because XApi uses OAuth 1.0a User Context.

    /// Search for users by keyword. Wraps GET /2/users/search.
    /// Returns users with full public metrics so callers get follower counts.
    pub async fn search_users(
        &self,
        query: &str,
        count: usize,
    ) -> Result<Vec<UserResponse>, XmasterError> {
        let max = count.clamp(1, 100);
        let encoded = percent_encoding::utf8_percent_encode(
            query,
            percent_encoding::NON_ALPHANUMERIC,
        );
        let url = format!(
            "{BASE}/users/search?query={encoded}&max_results={max}&{uf}",
            uf = Self::user_fields_param(),
        );
        let (users, _includes) =
            self.request_list::<UserResponse>(Method::GET, &url, None).await?;
        Ok(users)
    }

    pub async fn search_tweets(
        &self,
        query: &str,
        mode: &str,
        count: usize,
    ) -> Result<Vec<TweetData>, XmasterError> {
        self.search_tweets_paginated(query, mode, count, None, None).await
    }

    /// Build the optional `&start_time` / `&end_time` query fragment.
    fn time_params(start_time: Option<&str>, end_time: Option<&str>) -> String {
        let mut s = String::new();
        if let Some(st) = start_time {
            s.push_str(&format!("&start_time={st}"));
        }
        if let Some(et) = end_time {
            s.push_str(&format!("&end_time={et}"));
        }
        s
    }

    /// Cursor-paginate a tweet listing endpoint up to `count` results.
    /// `build_url` receives the per-page max_results and returns the request
    /// URL; this helper appends the pagination_token and merges authors.
    async fn paginate_tweets(
        &self,
        count: usize,
        min_page_size: usize,
        build_url: impl Fn(usize) -> String,
    ) -> Result<Vec<TweetData>, XmasterError> {
        let mut all_tweets = Vec::new();
        let mut next_token: Option<String> = None;
        let max_pages = (count / 100).max(1) + 1;

        for _ in 0..max_pages {
            // A page can overshoot `count` because page_size clamps UP to the
            // endpoint's minimum — guard before subtracting or this underflows.
            if all_tweets.len() >= count { break; }
            let remaining = count - all_tweets.len();
            let page_size = remaining.clamp(min_page_size, 100);

            let mut url = build_url(page_size);
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

    pub async fn search_tweets_paginated(
        &self,
        query: &str,
        mode: &str,
        count: usize,
        start_time: Option<&str>,
        end_time: Option<&str>,
    ) -> Result<Vec<TweetData>, XmasterError> {
        if self.ctx.config.web_reads() {
            let top = matches!(mode, "relevancy" | "relevant" | "top" | "popular");
            match Web::new(&self.ctx)?.search(query, top, count, start_time, end_time).await {
                Ok(t) => return Ok(t),
                Err(e) => self.web_read_failed("search", e)?,
            }
        }
        let encoded_query =
            percent_encoding::utf8_percent_encode(query, percent_encoding::NON_ALPHANUMERIC)
                .to_string();
        let sort = match mode {
            "relevancy" | "relevant" => "relevancy",
            _ => "recency",
        };
        let time_params = Self::time_params(start_time, end_time);

        self.paginate_tweets(count, 10, |page_size| {
            format!(
                "{BASE}/tweets/search/recent?query={encoded_query}&max_results={page_size}&sort_order={sort}&{tf}&{exp}&{uf}{time_params}",
                tf = Self::tweet_fields(),
                exp = Self::tweet_expansions(),
                uf = Self::user_fields_param(),
            )
        })
        .await
    }

    pub async fn get_user_tweets_paginated(
        &self,
        user_id: &str,
        count: usize,
        start_time: Option<&str>,
        end_time: Option<&str>,
    ) -> Result<Vec<TweetData>, XmasterError> {
        if self.ctx.config.web_reads() {
            match Web::new(&self.ctx)?.user_tweets(user_id, count, start_time, end_time).await {
                Ok(t) => return Ok(t),
                Err(e) => self.web_read_failed("user timeline read", e)?,
            }
        }
        let time_params = Self::time_params(start_time, end_time);

        self.paginate_tweets(count, 5, |page_size| {
            format!(
                "{BASE}/users/{user_id}/tweets?max_results={page_size}&{tf}&{exp}&{uf}{time_params}",
                tf = Self::tweet_fields(),
                exp = Self::tweet_expansions(),
                uf = Self::user_fields_param(),
            )
        })
        .await
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
        let category = if is_video {
            "tweet_video"
        } else if mime == "image/gif" {
            // Without this, animated GIFs over the simple-upload threshold
            // were chunk-uploaded as tweet_image and lost their animation.
            "tweet_gif"
        } else {
            "tweet_image"
        };

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
