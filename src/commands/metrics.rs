use crate::cli::parse_tweet_id;
use crate::context::AppContext;
use crate::errors::XmasterError;
use crate::output::{self, CsvRenderable, OutputFormat, Tableable};
use reqwest_oauth1::OAuthClientProvider;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Deserialize)]
struct ApiEnvelope {
    data: Option<TweetMetricsData>,
}

#[derive(Debug, Deserialize)]
struct TweetMetricsData {
    id: String,
    #[serde(default)]
    public_metrics: Option<PublicMetrics>,
    #[serde(default)]
    non_public_metrics: Option<NonPublicMetrics>,
}

#[derive(Debug, Deserialize, Default)]
struct PublicMetrics {
    #[serde(default)]
    like_count: u64,
    #[serde(default)]
    retweet_count: u64,
    #[serde(default)]
    reply_count: u64,
    #[serde(default)]
    impression_count: u64,
    #[serde(default)]
    quote_count: u64,
    #[serde(default)]
    bookmark_count: u64,
}

#[derive(Debug, Deserialize, Default)]
struct NonPublicMetrics {
    #[serde(default)]
    url_link_clicks: u64,
    #[serde(default)]
    user_profile_clicks: u64,
}

#[derive(Serialize)]
struct MetricsDisplay {
    #[serde(rename = "id")]
    tweet_id: String,
    impressions: u64,
    likes: u64,
    retweets: u64,
    replies: u64,
    quotes: u64,
    bookmarks: u64,
    profile_clicks: u64,
    url_clicks: u64,
}

impl Tableable for MetricsDisplay {
    fn to_table(&self) -> comfy_table::Table {
        let mut table = comfy_table::Table::new();
        table.set_header(vec!["Metric", "Count"]);
        table.add_row(vec!["Tweet ID", &self.tweet_id]);
        table.add_row(vec!["Impressions", &self.impressions.to_string()]);
        table.add_row(vec!["Likes", &self.likes.to_string()]);
        table.add_row(vec!["Retweets", &self.retweets.to_string()]);
        table.add_row(vec!["Replies", &self.replies.to_string()]);
        table.add_row(vec!["Quotes", &self.quotes.to_string()]);
        table.add_row(vec!["Bookmarks", &self.bookmarks.to_string()]);
        table.add_row(vec!["Profile Clicks", &self.profile_clicks.to_string()]);
        table.add_row(vec!["URL Clicks", &self.url_clicks.to_string()]);
        table
    }
}

impl CsvRenderable for MetricsDisplay {
    fn csv_headers() -> Vec<&'static str> {
        vec!["tweet_id", "impressions", "likes", "retweets", "replies", "quotes", "bookmarks", "profile_clicks", "url_clicks"]
    }
    fn csv_rows(&self) -> Vec<Vec<String>> {
        vec![vec![
            self.tweet_id.clone(),
            self.impressions.to_string(),
            self.likes.to_string(),
            self.retweets.to_string(),
            self.replies.to_string(),
            self.quotes.to_string(),
            self.bookmarks.to_string(),
            self.profile_clicks.to_string(),
            self.url_clicks.to_string(),
        ]]
    }
}

#[derive(Serialize)]
struct MetricsRow {
    #[serde(rename = "id")]
    tweet_id: String,
    impressions: u64,
    likes: u64,
    retweets: u64,
    replies: u64,
    quotes: u64,
    bookmarks: u64,
    profile_clicks: u64,
    url_clicks: u64,
}

#[derive(Serialize)]
struct MetricsBatch {
    rows: Vec<MetricsRow>,
}

impl Tableable for MetricsBatch {
    fn to_table(&self) -> comfy_table::Table {
        let mut table = comfy_table::Table::new();
        table.set_header(vec![
            "Tweet ID", "Impressions", "Likes", "RTs", "Replies", "Quotes", "Bookmarks",
            "Profile Clicks", "URL Clicks",
        ]);
        for r in &self.rows {
            table.add_row(vec![
                r.tweet_id.clone(),
                r.impressions.to_string(),
                r.likes.to_string(),
                r.retweets.to_string(),
                r.replies.to_string(),
                r.quotes.to_string(),
                r.bookmarks.to_string(),
                r.profile_clicks.to_string(),
                r.url_clicks.to_string(),
            ]);
        }
        table
    }
}

impl CsvRenderable for MetricsBatch {
    fn csv_headers() -> Vec<&'static str> {
        vec![
            "tweet_id", "impressions", "likes", "retweets", "replies", "quotes", "bookmarks",
            "profile_clicks", "url_clicks",
        ]
    }
    fn csv_rows(&self) -> Vec<Vec<String>> {
        self.rows
            .iter()
            .map(|r| {
                vec![
                    r.tweet_id.clone(),
                    r.impressions.to_string(),
                    r.likes.to_string(),
                    r.retweets.to_string(),
                    r.replies.to_string(),
                    r.quotes.to_string(),
                    r.bookmarks.to_string(),
                    r.profile_clicks.to_string(),
                    r.url_clicks.to_string(),
                ]
            })
            .collect()
    }
}

fn oauth_secrets(ctx: &AppContext) -> reqwest_oauth1::Secrets<'_> {
    let k = &ctx.config.keys;
    reqwest_oauth1::Secrets::new(&k.api_key, &k.api_secret)
        .token(&k.access_token, &k.access_token_secret)
}

/// Fetch tweet metrics with automatic fallback: tries private+public metrics first
/// (works for your own tweets), falls back to public-only on 403 (others' tweets).
/// Only retries on 403 — auth errors, rate limits, and server errors propagate immediately.
async fn fetch_tweet_metrics(
    ctx: &AppContext,
    tweet_id: &str,
) -> Result<(TweetMetricsData, bool), XmasterError> {
    // Try full metrics first (own tweets)
    let url_full = format!(
        "https://api.x.com/2/tweets/{tweet_id}?tweet.fields=public_metrics,non_public_metrics,organic_metrics"
    );
    let resp = ctx.client.clone().oauth1(oauth_secrets(ctx))
        .get(&url_full).send().await?;

    let first_status = resp.status();
    let first_body = resp.text().await.unwrap_or_default();

    if first_status.is_success() {
        if let Ok(envelope) = serde_json::from_str::<ApiEnvelope>(&first_body) {
            if let Some(tweet) = envelope.data {
                return Ok((tweet, false));
            }
        }
    }

    // Only fall back to public-only on 403 (private metrics on someone else's tweet).
    // For 401, 429, 5xx — propagate the real error.
    if first_status == 401 {
        return Err(XmasterError::AuthMissing {
            provider: "x",
            message: format!("HTTP 401: {}", &first_body[..first_body.len().min(200)]),
        });
    }
    if first_status == 429 {
        return Err(XmasterError::RateLimited { provider: "x", reset_at: 0 });
    }
    if first_status.as_u16() >= 500 {
        return Err(XmasterError::ServerError { status: first_status.as_u16() });
    }

    // 403 or other client error — try public-only fields
    let url_public = format!(
        "https://api.x.com/2/tweets/{tweet_id}?tweet.fields=public_metrics,created_at,author_id,text"
    );
    let resp = ctx.client.clone().oauth1(oauth_secrets(ctx))
        .get(&url_public).send().await?;

    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(XmasterError::NotFound(format!(
            "Tweet {tweet_id} (HTTP {status}: {})",
            &text[..text.len().min(100)]
        )));
    }

    let envelope: ApiEnvelope = resp.json().await?;
    let tweet = envelope.data.ok_or_else(|| XmasterError::NotFound(format!("Tweet {tweet_id}")))?;
    Ok((tweet, true))
}

pub async fn execute_batch(
    ctx: Arc<AppContext>,
    format: OutputFormat,
    ids: &[String],
) -> Result<(), XmasterError> {
    if ids.is_empty() {
        return Err(XmasterError::Config("No tweet IDs provided".into()));
    }
    if ids.len() == 1 {
        return execute(ctx, format, &ids[0]).await;
    }

    if !ctx.config.has_x_auth() {
        return Err(XmasterError::AuthMissing {
            provider: "x",
            message: "X API credentials not configured".into(),
        });
    }

    let mut rows = Vec::new();
    for id in ids {
        let tweet_id = parse_tweet_id(id);
        match fetch_tweet_metrics(&ctx, &tweet_id).await {
            Ok((tweet, _)) => {
                let public = tweet.public_metrics.unwrap_or_default();
                let non_public = tweet.non_public_metrics.unwrap_or_default();
                rows.push(MetricsRow {
                    tweet_id: tweet.id,
                    impressions: public.impression_count,
                    likes: public.like_count,
                    retweets: public.retweet_count,
                    replies: public.reply_count,
                    quotes: public.quote_count,
                    bookmarks: public.bookmark_count,
                    profile_clicks: non_public.user_profile_clicks,
                    url_clicks: non_public.url_link_clicks,
                });
            }
            Err(e) => {
                eprintln!("Warning: {tweet_id}: {e}");
            }
        }
    }

    let batch = MetricsBatch { rows };
    output::render_csv(format, &batch, None);
    Ok(())
}

pub async fn execute(
    ctx: Arc<AppContext>,
    format: OutputFormat,
    id: &str,
) -> Result<(), XmasterError> {
    if !ctx.config.has_x_auth() {
        return Err(XmasterError::AuthMissing {
            provider: "x",
            message: "X API credentials not configured".into(),
        });
    }

    let tweet_id = parse_tweet_id(id);

    // Try with private metrics first (own tweets), fall back to public-only (others' tweets)
    let (tweet, is_public_only) = fetch_tweet_metrics(&ctx, &tweet_id).await?;

    let public = tweet.public_metrics.unwrap_or_default();
    let non_public = tweet.non_public_metrics.unwrap_or_default();

    if is_public_only && format == OutputFormat::Table {
        eprintln!("Note: Showing public metrics only (not your tweet)");
    }

    let display = MetricsDisplay {
        tweet_id: tweet.id,
        impressions: public.impression_count,
        likes: public.like_count,
        retweets: public.retweet_count,
        replies: public.reply_count,
        quotes: public.quote_count,
        bookmarks: public.bookmark_count,
        profile_clicks: non_public.user_profile_clicks,
        url_clicks: non_public.url_link_clicks,
    };
    output::render(format, &display, None);
    Ok(())
}
