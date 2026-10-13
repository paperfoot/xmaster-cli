use crate::context::AppContext;
use crate::errors::XmasterError;
use crate::intel::judge;
use crate::output::{self, CsvRenderable, OutputFormat, Tableable};
use crate::providers::jev::Jev;
use crate::providers::xapi::{TweetData, XApi};
use serde::Serialize;
use std::sync::Arc;

#[derive(Serialize)]
struct RepliesResult {
    #[serde(rename = "id")]
    tweet_id: String,
    replies: Vec<ReplyItem>,
    total: usize,
    /// Replies JEV judged not worth reading (thanks, emoji, spam, no
    /// substance) and left out. Pass --all to see every reply.
    #[serde(skip_serializing_if = "Option::is_none")]
    trimmed: Option<usize>,
}

#[derive(Serialize)]
struct ReplyItem {
    id: String,
    author: String,
    text: String,
    likes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    date: Option<String>,
}

impl Tableable for RepliesResult {
    fn to_table(&self) -> comfy_table::Table {
        let mut table = comfy_table::Table::new();
        table.set_header(vec!["Author", "Text", "Likes"]);
        for r in &self.replies {
            let text_preview = if r.text.chars().count() > 120 {
                let truncated: String = r.text.chars().take(120).collect();
                format!("{truncated}...")
            } else {
                r.text.clone()
            };
            table.add_row(vec![
                format!("@{}", r.author),
                text_preview,
                r.likes.to_string(),
            ]);
        }
        table
    }
}

impl CsvRenderable for RepliesResult {
    fn csv_headers() -> Vec<&'static str> {
        vec!["id", "author", "text", "likes", "date"]
    }
    fn csv_rows(&self) -> Vec<Vec<String>> {
        self.replies
            .iter()
            .map(|r| {
                vec![
                    r.id.clone(),
                    r.author.clone(),
                    r.text.clone(),
                    r.likes.to_string(),
                    r.date.clone().unwrap_or_default(),
                ]
            })
            .collect()
    }
}

pub async fn execute(
    ctx: Arc<AppContext>,
    format: OutputFormat,
    id: &str,
    count: usize,
    all: bool,
) -> Result<(), XmasterError> {
    let api = XApi::new(ctx.clone());
    let tweet_id = id.to_string();

    let mut tweets = api.get_replies(&tweet_id, count).await?;
    let trimmed = if all { None } else { jev_trim(&ctx, &tweet_id, &mut tweets).await };

    let replies: Vec<ReplyItem> = tweets
        .into_iter()
        .map(|t| {
            let likes = t
                .public_metrics
                .as_ref()
                .map(|m| m.like_count)
                .unwrap_or(0);
            ReplyItem {
                id: t.id,
                author: t.author_username.unwrap_or_else(|| "unknown".into()),
                text: t.text,
                likes,
                date: t.created_at,
            }
        })
        .collect();

    let total = replies.len();
    let result = RepliesResult {
        tweet_id,
        replies,
        total,
        trimmed,
    };
    output::render_csv(format, &result, None);
    Ok(())
}

/// Drop replies JEV judges not worth reading; returns how many it dropped.
/// Unjudged replies stay. `None` (nothing dropped) without JEV.
async fn jev_trim(ctx: &AppContext, tweet_id: &str, tweets: &mut Vec<TweetData>) -> Option<usize> {
    let jev = Jev::from_ctx(ctx)?;
    if tweets.is_empty() {
        return None;
    }
    let parent = crate::providers::fxtwitter::fetch_post(tweet_id)
        .await
        .map(|(post, _)| post.text)
        .unwrap_or_default();
    let input: Vec<(&str, &str)> = tweets
        .iter()
        .take(judge::MAX_ITEMS)
        .map(|t| (t.author_username.as_deref().unwrap_or("unknown"), t.text.as_str()))
        .collect();
    let keep = judge::worth_reading(&jev, &parent, &input).await?;
    let before = tweets.len();
    let mut verdicts = keep.into_iter().chain(std::iter::repeat(None));
    tweets.retain(|_| verdicts.next().flatten() != Some(false));
    Some(before - tweets.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn replies_untrimmed_without_jev() {
        let mut config = crate::config::AppConfig::default();
        config.settings.disable_jev = true;
        let ctx = AppContext::new(config).unwrap();
        let reply: TweetData = serde_json::from_value(serde_json::json!({"id": "1", "text": "thanks"})).unwrap();
        let mut tweets = vec![reply];
        assert!(jev_trim(&ctx, "9", &mut tweets).await.is_none());
        assert_eq!(tweets.len(), 1);
    }
}
