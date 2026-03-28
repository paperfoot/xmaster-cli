use crate::context::AppContext;
use crate::errors::XmasterError;
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
}

impl Tableable for SearchResults {
    fn to_table(&self) -> comfy_table::Table {
        let mut table = comfy_table::Table::new();
        table.set_header(vec!["ID", "Author", "Text", "Views", "Likes", "RTs", "Replies", "Date"]);
        for t in &self.tweets {
            let truncated: String = if t.text.chars().count() > 80 {
                t.text.chars().take(77).collect::<String>() + "..."
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
                t.replies.to_string(),
                t.date.clone(),
            ]);
        }
        table
    }
}

impl CsvRenderable for SearchResults {
    fn csv_headers() -> Vec<&'static str> {
        vec!["id", "author", "text", "impressions", "likes", "retweets", "replies", "date"]
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
    let start_time = since.map(|s| crate::commands::timeline::parse_since(s)).transpose()
        .map_err(|e| XmasterError::Config(e))?;
    let end_time = before.map(|s| crate::commands::timeline::parse_since(s)).transpose()
        .map_err(|e| XmasterError::Config(e))?;
    let api = XApi::new(ctx.clone());
    let tweets = api.search_tweets_paginated(query, mode, count, start_time.as_deref(), end_time.as_deref()).await?;
    let display = SearchResults {
        query: query.to_string(),
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
                replies: metrics.map(|m| m.reply_count).unwrap_or(0),
                date: t.created_at.unwrap_or_default(),
            }
        }).collect(),
    };
    output::render_csv(format, &display, None);
    Ok(())
}
