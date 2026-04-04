use crate::context::AppContext;
use crate::errors::XmasterError;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Public result type
// ---------------------------------------------------------------------------

/// Result returned by all xAI search methods.
pub struct XaiSearchResult {
    /// Extracted text from the xAI Responses API.
    pub text: String,
    /// Citation URLs (x.com / twitter.com links) found in the response.
    pub citations: Vec<String>,
}

// ---------------------------------------------------------------------------
// Internal response types (xAI Responses API shape)
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct XaiResponse {
    output: Option<Vec<XaiOutputItem>>,
}

#[derive(Deserialize)]
struct XaiOutputItem {
    #[serde(rename = "type")]
    item_type: Option<String>,
    content: Option<Vec<XaiContent>>,
    text: Option<String>,
}

#[derive(Deserialize)]
struct XaiContent {
    #[serde(rename = "type")]
    content_type: Option<String>,
    text: Option<String>,
    url: Option<String>,
}

// ---------------------------------------------------------------------------
// XaiSearch
// ---------------------------------------------------------------------------

pub struct XaiSearch {
    ctx: Arc<AppContext>,
}

impl XaiSearch {
    pub fn new(ctx: Arc<AppContext>) -> Self {
        Self { ctx }
    }

    /// Resolve the xAI API key from config, falling back to `XAI_API_KEY` env.
    fn api_key(&self) -> String {
        let key = &self.ctx.config.keys.xai;
        if key.is_empty() {
            std::env::var("XAI_API_KEY").unwrap_or_default()
        } else {
            key.clone()
        }
    }

    // -----------------------------------------------------------------------
    // Core API call
    // -----------------------------------------------------------------------

    /// Call the xAI Responses API (`/v1/responses`) with the `x_search` tool.
    async fn call_responses_api(
        &self,
        prompt: &str,
        x_search_config: Option<serde_json::Value>,
    ) -> Result<XaiResponse, XmasterError> {
        let api_key = self.api_key();
        if api_key.is_empty() {
            return Err(XmasterError::AuthMissing {
                provider: "xai",
                message: "xAI API key not configured".into(),
            });
        }

        // Build the x_search tool object, merging any extra config fields.
        let mut x_search_tool = json!({"type": "x_search"});
        if let Some(config) = x_search_config {
            if let (Some(tool_obj), Some(config_obj)) =
                (x_search_tool.as_object_mut(), config.as_object())
            {
                for (k, v) in config_obj {
                    tool_obj.insert(k.clone(), v.clone());
                }
            }
        }

        let body = json!({
            "model": "grok-4-1-fast",
            "input": [{"role": "user", "content": prompt}],
            "tools": [x_search_tool],
            "store": false,
        });

        // xAI Responses API needs a longer timeout (Grok runs search server-side)
        let xai_client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .map_err(XmasterError::Http)?;

        let resp = xai_client
            .post("https://api.x.ai/v1/responses")
            .header("Authorization", format!("Bearer {api_key}"))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await?;

        if resp.status().as_u16() == 401 {
            return Err(XmasterError::AuthMissing {
                provider: "xai",
                message: "Invalid or expired xAI API key".into(),
            });
        }
        if resp.status().as_u16() == 429 {
            return Err(XmasterError::RateLimited { provider: "xai", reset_at: 0 });
        }
        if !resp.status().is_success() {
            let status = resp.status();
            let body_text = resp.text().await.unwrap_or_default();
            return Err(XmasterError::Api {
                provider: "xai",
                code: "api_error",
                message: format!("HTTP {status}: {body_text}"),
            });
        }

        let xai_resp: XaiResponse = resp.json().await?;
        Ok(xai_resp)
    }

    // -----------------------------------------------------------------------
    // Public search methods
    // -----------------------------------------------------------------------

    /// Search X posts by keywords, hashtags, or topics.
    ///
    /// Automatically parses `from:username` operators out of the query and
    /// maps them to the xAI `allowed_x_handles` filter for deterministic
    /// author restriction.  The remaining keywords become the natural-language
    /// prompt sent to Grok.
    pub async fn search_posts(
        &self,
        query: &str,
        count: usize,
        language: Option<&str>,
        from_date: Option<&str>,
        to_date: Option<&str>,
    ) -> Result<XaiSearchResult, XmasterError> {
        let (handles, clean_query) = parse_from_handles(query);

        let lang_part = language
            .map(|l| format!(" Filter to {l} language posts only."))
            .unwrap_or_default();

        // If only from: handles with no extra keywords, ask for latest posts
        // by that user rather than a topical search.
        let prompt = if clean_query.is_empty() {
            let who = handles.iter()
                .map(|h| format!("@{h}"))
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "Return the {count} most recent posts by {who} on X, \
                 newest first.{lang_part}\n\
                 For each post include: author @username, display name, post text, \
                 date/time, and engagement metrics (likes, reposts, replies) if available.\n\
                 Format the output as markdown."
            )
        } else {
            let handle_hint = if handles.is_empty() {
                String::new()
            } else {
                let who = handles.iter()
                    .map(|h| format!("@{h}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!(" Only include posts by {who}.")
            };
            format!(
                "Search X for the most recent posts about: {clean_query}\n\
                 Return up to {count} results, newest first.{handle_hint}{lang_part}\n\
                 For each post include: author @username, display name, post text, \
                 date/time, and engagement metrics (likes, reposts, replies) if available.\n\
                 Format the output as markdown."
            )
        };

        let handle_refs: Vec<String> = handles.clone();
        let allowed = if handle_refs.is_empty() { None } else { Some(handle_refs.as_slice()) };
        let x_config = build_x_search_config(from_date, to_date, allowed, None);
        let resp = self.call_responses_api(&prompt, x_config).await?;

        Ok(XaiSearchResult {
            text: extract_text(&resp),
            citations: extract_citations(&resp),
        })
    }

    /// Get current trending topics and hashtags on X.
    pub async fn get_trending(
        &self,
        region: Option<&str>,
        category: Option<&str>,
    ) -> Result<XaiSearchResult, XmasterError> {
        let region_part = region
            .map(|r| format!(" in {r}"))
            .unwrap_or_else(|| " globally".into());
        let category_part = category
            .map(|c| format!(" Focus on {c} topics."))
            .unwrap_or_default();

        let prompt = format!(
            "What are the current trending topics and hashtags on X\
             {region_part}?{category_part}\n\
             List the top trending topics with brief descriptions of why they are trending.\n\
             Format the output as markdown."
        );

        // Trending needs no x_search config (no date/handle filters).
        let resp = self.call_responses_api(&prompt, None).await?;

        Ok(XaiSearchResult {
            text: extract_text(&resp),
            citations: extract_citations(&resp),
        })
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build the optional x_search tool configuration (date range, handle filters).
fn build_x_search_config(
    from_date: Option<&str>,
    to_date: Option<&str>,
    allowed_handles: Option<&[String]>,
    excluded_handles: Option<&[String]>,
) -> Option<serde_json::Value> {
    let mut config = serde_json::Map::new();

    if let Some(d) = from_date {
        config.insert("from_date".into(), json!(d));
    }
    if let Some(d) = to_date {
        config.insert("to_date".into(), json!(d));
    }
    if let Some(handles) = allowed_handles {
        if !handles.is_empty() {
            config.insert("allowed_x_handles".into(), json!(handles));
        }
    }
    if let Some(handles) = excluded_handles {
        if !handles.is_empty() {
            config.insert("excluded_x_handles".into(), json!(handles));
        }
    }

    if config.is_empty() {
        None
    } else {
        Some(serde_json::Value::Object(config))
    }
}

/// Extract text content from the xAI Responses API output.
///
/// The response shape is:
/// ```json
/// { "output": [
///     { "type": "message", "content": [
///         { "type": "output_text", "text": "..." }
///     ]}
/// ]}
/// ```
fn extract_text(resp: &XaiResponse) -> String {
    let mut parts = Vec::new();
    if let Some(output) = &resp.output {
        for item in output {
            if item.item_type.as_deref() == Some("message") {
                if let Some(content) = &item.content {
                    for c in content {
                        if c.content_type.as_deref() == Some("output_text") {
                            if let Some(text) = &c.text {
                                parts.push(text.clone());
                            }
                        }
                    }
                }
            } else if let Some(text) = &item.text {
                parts.push(text.clone());
            }
        }
    }
    parts.join("\n")
}

/// Extract citation URLs from the xAI Responses API output.
fn extract_citations(resp: &XaiResponse) -> Vec<String> {
    let mut urls = Vec::new();
    if let Some(output) = &resp.output {
        for item in output {
            if let Some(content) = &item.content {
                for c in content {
                    if matches!(
                        c.content_type.as_deref(),
                        Some("cite") | Some("url")
                    ) {
                        if let Some(url) = &c.url {
                            urls.push(url.clone());
                        }
                    }
                    // Also capture x.com / twitter.com URLs embedded in text.
                    if let Some(text) = &c.text {
                        if text.starts_with("https://x.com/")
                            || text.starts_with("https://twitter.com/")
                        {
                            urls.push(text.clone());
                        }
                    }
                }
            }
        }
    }
    urls
}

/// Parse `from:username` operators out of a query string.
///
/// Returns `(handles, remaining_query)` where `handles` is a list of
/// usernames (without the `@` prefix) and `remaining_query` is the query
/// with all `from:` tokens stripped and whitespace normalised.
fn parse_from_handles(query: &str) -> (Vec<String>, String) {
    let mut handles = Vec::new();
    let mut remaining = Vec::new();

    for token in query.split_whitespace() {
        if let Some(handle) = token.strip_prefix("from:") {
            let h = handle.trim_start_matches('@');
            if !h.is_empty() {
                handles.push(h.to_string());
            }
        } else {
            remaining.push(token);
        }
    }

    (handles, remaining.join(" "))
}
