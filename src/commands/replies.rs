use crate::cli::parse_tweet_id;
use crate::context::AppContext;
use crate::errors::XmasterError;
use crate::output::{self, CsvRenderable, OutputFormat, Tableable};
use crate::providers::xapi::XApi;
use serde::Serialize;
use std::sync::Arc;

#[derive(Serialize)]
struct RepliesResult {
    #[serde(rename = "id")]
    tweet_id: String,
    replies: Vec<ReplyItem>,
    total: usize,
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
) -> Result<(), XmasterError> {
    let api = XApi::new(ctx.clone());
    let tweet_id = parse_tweet_id(id);

    let tweets = api.get_replies(&tweet_id, count).await?;

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
    };
    output::render_csv(format, &result, None);
    Ok(())
}
