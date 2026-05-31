use crate::context::AppContext;
use crate::errors::XmasterError;
use crate::intel::store::IntelStore;
use crate::output::{self, CsvRenderable, OutputFormat, Tableable};
use crate::providers::xapi::XApi;
use serde::Serialize;
use std::sync::Arc;

#[derive(Serialize)]
struct SearchResults {
    query: String,
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
    replies: u64,
    date: String,
    age_minutes: i64,
}

/// Compact human age: 45m, 6h, 3.0d. Lets the agent (and humans) see at a
/// glance whether a post is fresh enough to be worth replying to — reply
/// momentum dies after ~30-60 min.
fn human_age(minutes: i64) -> String {
    if minutes < 0 {
        "0m".into()
    } else if minutes < 60 {
        format!("{minutes}m")
    } else if minutes < 1440 {
        format!("{}h", minutes / 60)
    } else {
        format!("{:.1}d", minutes as f64 / 1440.0)
    }
}

impl Tableable for SearchResults {
    fn to_table(&self) -> comfy_table::Table {
        let mut table = comfy_table::Table::new();
        table.set_header(vec!["ID", "Author", "Age", "Text", "Views", "Likes", "RTs", "Replies", "Date"]);
        for t in &self.tweets {
            let truncated: String = if t.text.chars().count() > 80 {
                t.text.chars().take(77).collect::<String>() + "..."
            } else {
                t.text.clone()
            };
            table.add_row(vec![
                t.id.clone(),
                t.author.clone(),
                human_age(t.age_minutes),
                truncated,
                t.impressions.to_string(),
                t.likes.to_string(),
                t.retweets.to_string(),
                t.replies.to_string(),
                t.date.clone(),
            ]);
        }
        table
    }
}

impl CsvRenderable for SearchResults {
    fn csv_headers() -> Vec<&'static str> {
        vec!["id", "author", "age_minutes", "text", "impressions", "likes", "retweets", "replies", "date"]
    }

    fn csv_rows(&self) -> Vec<Vec<String>> {
        self.tweets
            .iter()
            .map(|t| {
                vec![
                    t.id.clone(),
                    t.author.clone(),
                    t.age_minutes.to_string(),
                    t.text.clone(),
                    t.impressions.to_string(),
                    t.likes.to_string(),
                    t.retweets.to_string(),
                    t.replies.to_string(),
                    t.date.clone(),
                ]
            })
            .collect()
    }
}

pub async fn execute(
    ctx: Arc<AppContext>,
    format: OutputFormat,
    query: &str,
    mode: &str,
    count: usize,
    since: Option<&str>,
    before: Option<&str>,
) -> Result<(), XmasterError> {
    let start_time = since.map(crate::commands::timeline::parse_since).transpose()
        .map_err(XmasterError::Config)?;
    let end_time = before.map(crate::commands::timeline::parse_since).transpose()
        .map_err(XmasterError::Config)?;
    let api = XApi::new(ctx.clone());
    let tweets = api.search_tweets_paginated(query, mode, count, start_time.as_deref(), end_time.as_deref()).await?;
    if let Ok(store) = IntelStore::open() {
        let _ = store.record_discovered_posts("search", &tweets);
    }
    let display = SearchResults {
        query: query.to_string(),
        tweets: tweets.into_iter().map(|t| {
            let metrics = t.public_metrics.as_ref();
            let age_minutes = t.created_at.as_deref()
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|dt| (chrono::Utc::now() - dt.with_timezone(&chrono::Utc)).num_minutes())
                .unwrap_or(0);
            TweetRow {
                id: t.id,
                author: t.author_username
                    .map(|u| format!("@{u}"))
                    .unwrap_or_else(|| t.author_id.unwrap_or_default()),
                text: t.text,
                impressions: metrics.map(|m| m.impression_count).unwrap_or(0),
                likes: metrics.map(|m| m.like_count).unwrap_or(0),
                retweets: metrics.map(|m| m.retweet_count).unwrap_or(0),
                replies: metrics.map(|m| m.reply_count).unwrap_or(0),
                date: t.created_at.unwrap_or_default(),
                age_minutes,
            }
        }).collect(),
    };
    output::render_csv(format, &display, None);
    Ok(())
}
