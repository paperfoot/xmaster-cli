use crate::context::AppContext;
use crate::errors::XmasterError;
use crate::output::{self, CsvRenderable, OutputFormat, Tableable};
use crate::providers::xapi::XApi;
use serde::Serialize;
use std::sync::Arc;

/// Parse a human-friendly duration like "12h", "24h", "7d" into an RFC3339 timestamp.
/// Also accepts ISO 8601 timestamps as-is.
pub fn parse_since(s: &str) -> Result<String, String> {
    use chrono::Utc;
    let s = s.trim();
    // Try parsing as a duration shorthand
    if let Some(num_str) = s.strip_suffix('h') {
        let hours: i64 = num_str.parse().map_err(|_| format!("Invalid hours: {s}"))?;
        let ts = Utc::now() - chrono::Duration::hours(hours);
        return Ok(ts.to_rfc3339_opts(chrono::SecondsFormat::Secs, true));
    }
    if let Some(num_str) = s.strip_suffix('d') {
        let days: i64 = num_str.parse().map_err(|_| format!("Invalid days: {s}"))?;
        let ts = Utc::now() - chrono::Duration::days(days);
        return Ok(ts.to_rfc3339_opts(chrono::SecondsFormat::Secs, true));
    }
    if let Some(num_str) = s.strip_suffix('m') {
        let mins: i64 = num_str.parse().map_err(|_| format!("Invalid minutes: {s}"))?;
        let ts = Utc::now() - chrono::Duration::minutes(mins);
        return Ok(ts.to_rfc3339_opts(chrono::SecondsFormat::Secs, true));
    }
    // Assume ISO 8601 or date string
    Ok(s.to_string())
}

#[derive(Serialize)]
struct TweetList {
    tweets: Vec<TweetRow>,
}

#[derive(Serialize)]
struct TweetRow {
    id: String,
    author: String,
    text: String,
    impressions: u64,
    likes: u64,
    retweets: u64,
    date: String,
}

impl Tableable for TweetList {
    fn to_table(&self) -> comfy_table::Table {
        let mut table = comfy_table::Table::new();
        table.set_header(vec!["ID", "Author", "Text", "Views", "Likes", "RTs", "Date"]);
        for t in &self.tweets {
            let truncated = if t.text.len() > 80 {
                format!("{}...", &t.text[..77])
            } else {
                t.text.clone()
            };
            table.add_row(vec![
                t.id.clone(),
                t.author.clone(),
                truncated,
                t.impressions.to_string(),
                t.likes.to_string(),
                t.retweets.to_string(),
                t.date.clone(),
            ]);
        }
        table
    }
}

impl CsvRenderable for TweetList {
    fn csv_headers() -> Vec<&'static str> {
        vec!["id", "author", "text", "impressions", "likes", "retweets", "date"]
    }

    fn csv_rows(&self) -> Vec<Vec<String>> {
        self.tweets
            .iter()
            .map(|t| {
                vec![
                    t.id.clone(),
                    t.author.clone(),
                    t.text.clone(),
                    t.impressions.to_string(),
                    t.likes.to_string(),
                    t.retweets.to_string(),
                    t.date.clone(),
                ]
            })
            .collect()
    }
}

fn tweets_to_list(tweets: Vec<crate::providers::xapi::TweetData>) -> TweetList {
    TweetList {
        tweets: tweets.into_iter().map(|t| {
            let metrics = t.public_metrics.as_ref();
            TweetRow {
                id: t.id,
                author: t.author_username
                    .map(|u| format!("@{u}"))
                    .unwrap_or_else(|| t.author_id.unwrap_or_default()),
                text: t.text,
                impressions: metrics.map(|m| m.impression_count).unwrap_or(0),
                likes: metrics.map(|m| m.like_count).unwrap_or(0),
                retweets: metrics.map(|m| m.retweet_count).unwrap_or(0),
                date: t.created_at.unwrap_or_default(),
            }
        }).collect(),
    }
}

pub async fn timeline(
    ctx: Arc<AppContext>,
    format: OutputFormat,
    user: Option<&str>,
    count: usize,
    since: Option<&str>,
    before: Option<&str>,
    sort: Option<&str>,
) -> Result<(), XmasterError> {
    let start_time = since.map(|s| parse_since(s)).transpose()
        .map_err(|e| XmasterError::Config(e))?;
    let end_time = before.map(|s| parse_since(s)).transpose()
        .map_err(|e| XmasterError::Config(e))?;

    let api = XApi::new(ctx.clone());
    let tweets = match user {
        Some(username) => {
            let u = api.get_user_by_username(username).await?;
            api.get_user_tweets_paginated(&u.id, count, start_time.as_deref(), end_time.as_deref()).await?
        }
        None => api.get_home_timeline(count).await?,
    };
    let mut list = tweets_to_list(tweets);

    // Client-side sort
    if let Some(sort_by) = sort {
        match sort_by {
            "impressions" | "views" => list.tweets.sort_by(|a, b| b.impressions.cmp(&a.impressions)),
            "likes" => list.tweets.sort_by(|a, b| b.likes.cmp(&a.likes)),
            "retweets" | "rts" => list.tweets.sort_by(|a, b| b.retweets.cmp(&a.retweets)),
            "date" => {} // already sorted by date from API
            _ => {}
        }
    }

    output::render_csv(format, &list, None);
    Ok(())
}

pub async fn mentions(
    ctx: Arc<AppContext>,
    format: OutputFormat,
    count: usize,
    since_id: Option<&str>,
) -> Result<(), XmasterError> {
    let api = XApi::new(ctx.clone());
    let user_id = api.get_authenticated_user_id().await?;
    let tweets = api.get_user_mentions_since(&user_id, count, since_id).await?;
    output::render_csv(format, &tweets_to_list(tweets), None);
    Ok(())
}

