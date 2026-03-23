use crate::cli::parse_tweet_id;
use crate::context::AppContext;
use crate::errors::XmasterError;
use crate::output::{self, CsvRenderable, OutputFormat, Tableable};
use crate::providers::xapi::XApi;
use serde::Serialize;
use std::sync::Arc;

#[derive(Serialize)]
struct ActionResult {
    action: String,
    tweet_id: String,
    success: bool,
}

impl Tableable for ActionResult {
    fn to_table(&self) -> comfy_table::Table {
        let mut table = comfy_table::Table::new();
        table.set_header(vec!["Action", "Tweet ID", "Status"]);
        table.add_row(vec![
            self.action.as_str(),
            self.tweet_id.as_str(),
            if self.success { "OK" } else { "Failed" },
        ]);
        table
    }
}

impl CsvRenderable for ActionResult {
    fn csv_headers() -> Vec<&'static str> {
        vec!["action", "tweet_id", "status"]
    }

    fn csv_rows(&self) -> Vec<Vec<String>> {
        vec![vec![
            self.action.clone(),
            self.tweet_id.clone(),
            if self.success { "OK" } else { "Failed" }.to_string(),
        ]]
    }
}

fn render_success(format: OutputFormat, action_name: &str, tweet_id: String) {
    // Silently log engagement to intelligence store
    if let Ok(store) = crate::intel::store::IntelStore::open() {
        let _ = store.log_engagement(action_name, Some(&tweet_id), None, None, None);
    }

    let display = ActionResult {
        action: action_name.to_string(),
        tweet_id,
        success: true,
    };
    output::render_csv(format, &display, None);
}

/// Print an undo hint to stderr (only in table mode so it doesn't pollute JSON/CSV stdout).
fn undo_hint(format: OutputFormat, message: &str) {
    if format == OutputFormat::Table {
        eprintln!("{message}");
    }
}

/// Add a contextual hint when engagement actions fail with 403.
fn maybe_add_hint(err: XmasterError, action: &str) -> XmasterError {
    if let XmasterError::AuthMissing { provider, ref message } = err {
        if message.contains("403") {
            return XmasterError::Api {
                provider,
                code: "forbidden",
                message: format!(
                    "{message}. Hint: You may have already {action}d this tweet"
                ),
            };
        }
    }
    err
}

pub async fn delete(ctx: Arc<AppContext>, format: OutputFormat, id: &str) -> Result<(), XmasterError> {
    let api = XApi::new(ctx);
    let tweet_id = parse_tweet_id(id);
    api.delete_tweet(&tweet_id).await?;
    render_success(format, "delete", tweet_id);
    undo_hint(format, "Note: This cannot be undone");
    Ok(())
}

pub async fn like(ctx: Arc<AppContext>, format: OutputFormat, id: &str) -> Result<(), XmasterError> {
    let api = XApi::new(ctx);
    let tweet_id = parse_tweet_id(id);
    api.like_tweet(&tweet_id).await.map_err(|e| maybe_add_hint(e, "like"))?;
    render_success(format, "like", tweet_id.clone());
    undo_hint(format, &format!("Undo: xmaster unlike {tweet_id}"));
    Ok(())
}

pub async fn unlike(ctx: Arc<AppContext>, format: OutputFormat, id: &str) -> Result<(), XmasterError> {
    let api = XApi::new(ctx);
    let tweet_id = parse_tweet_id(id);
    api.unlike_tweet(&tweet_id).await.map_err(|e| maybe_add_hint(e, "unlike"))?;
    render_success(format, "unlike", tweet_id.clone());
    undo_hint(format, &format!("Undo: xmaster like {tweet_id}"));
    Ok(())
}

pub async fn retweet(ctx: Arc<AppContext>, format: OutputFormat, id: &str) -> Result<(), XmasterError> {
    let api = XApi::new(ctx);
    let tweet_id = parse_tweet_id(id);
    api.retweet(&tweet_id).await.map_err(|e| maybe_add_hint(e, "retweet"))?;
    render_success(format, "retweet", tweet_id.clone());
    undo_hint(format, &format!("Undo: xmaster unretweet {tweet_id}"));
    Ok(())
}

pub async fn unretweet(ctx: Arc<AppContext>, format: OutputFormat, id: &str) -> Result<(), XmasterError> {
    let api = XApi::new(ctx);
    let tweet_id = parse_tweet_id(id);
    api.unretweet(&tweet_id).await.map_err(|e| maybe_add_hint(e, "unretweet"))?;
    render_success(format, "unretweet", tweet_id.clone());
    undo_hint(format, &format!("Undo: xmaster retweet {tweet_id}"));
    Ok(())
}

pub async fn bookmark(ctx: Arc<AppContext>, format: OutputFormat, id: &str) -> Result<(), XmasterError> {
    let api = XApi::new(ctx);
    let tweet_id = parse_tweet_id(id);
    api.bookmark_tweet(&tweet_id).await?;
    render_success(format, "bookmark", tweet_id.clone());
    undo_hint(format, &format!("Undo: xmaster unbookmark {tweet_id}"));
    Ok(())
}

pub async fn unbookmark(ctx: Arc<AppContext>, format: OutputFormat, id: &str) -> Result<(), XmasterError> {
    let api = XApi::new(ctx);
    let tweet_id = parse_tweet_id(id);
    api.unbookmark_tweet(&tweet_id).await?;
    render_success(format, "unbookmark", tweet_id.clone());
    undo_hint(format, &format!("Undo: xmaster bookmark {tweet_id}"));
    Ok(())
}
