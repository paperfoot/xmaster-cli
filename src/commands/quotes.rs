use crate::cli::parse_tweet_id;
use crate::context::AppContext;
use crate::errors::XmasterError;
use crate::intel::store::IntelStore;
use crate::output::{self, OutputFormat, Tableable};
use crate::providers::xapi::XApi;
use serde::Serialize;
use std::sync::Arc;

#[derive(Serialize)]
struct QuoteRow {
    id: String,
    author: String,
    text: String,
    likes: u64,
    impressions: u64,
}

#[derive(Serialize)]
struct QuoteList {
    source_tweet_id: String,
    count: usize,
    cached_into_library: bool,
    quotes: Vec<QuoteRow>,
}

impl Tableable for QuoteList {
    fn to_table(&self) -> comfy_table::Table {
        let mut table = comfy_table::Table::new();
        table.set_header(vec!["ID", "Author", "Text", "Likes", "Views"]);
        for q in &self.quotes {
            let text = if q.text.chars().count() > 100 {
                format!("{}...", crate::utils::safe_truncate(&q.text, 97))
            } else {
                q.text.clone()
            };
            table.add_row(vec![
                q.id.clone(),
                q.author.clone(),
                text,
                q.likes.to_string(),
                q.impressions.to_string(),
            ]);
        }
        table
    }
}

pub async fn execute(
    ctx: Arc<AppContext>,
    format: OutputFormat,
    id: &str,
    count: usize,
) -> Result<(), XmasterError> {
    let source_tweet_id = parse_tweet_id(id);
    let api = XApi::new(ctx.clone());
    let tweets = api.get_tweet_quotes(&source_tweet_id, count).await?;

    // Persist into the discovered_posts library so `xmaster inspire` can
    // later surface these. Failure to cache is non-fatal.
    let cached = if let Ok(store) = IntelStore::open() {
        store
            .record_discovered_posts("quotes", &tweets)
            .is_ok()
    } else {
        false
    };

    if tweets.is_empty() {
        return Err(XmasterError::NotFound(format!(
            "No quote tweets found for {source_tweet_id}"
        )));
    }

    let rows: Vec<QuoteRow> = tweets
        .into_iter()
        .map(|t| {
            let metrics = t.public_metrics.as_ref();
            QuoteRow {
                id: t.id,
                author: t
                    .author_username
                    .map(|u| format!("@{u}"))
                    .unwrap_or_else(|| "@?".into()),
                text: t.text,
                likes: metrics.map(|m| m.like_count).unwrap_or(0),
                impressions: metrics.map(|m| m.impression_count).unwrap_or(0),
            }
        })
        .collect();

    let result = QuoteList {
        source_tweet_id,
        count: rows.len(),
        cached_into_library: cached,
        quotes: rows,
    };
    output::render(format, &result, None);
    Ok(())
}
