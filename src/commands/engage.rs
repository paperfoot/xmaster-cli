use crate::commands::{post_url, reclassify_403, WriteRoute};
use crate::context::AppContext;
use crate::errors::XmasterError;
use crate::output::{self, CsvRenderable, OutputFormat, Tableable};
use crate::providers::xapi::XApi;
use serde::Serialize;
use std::sync::Arc;

/// What an agent needs after a write: what happened, where, how to undo it
/// and which path carried it, so it never needs a second read.
#[derive(Serialize)]
struct ActionResult {
    action: &'static str,
    id: String,
    url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    undo: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    via: Option<&'static str>,
}

impl Tableable for ActionResult {
    fn to_table(&self) -> comfy_table::Table {
        let mut table = comfy_table::Table::new();
        table.set_header(vec!["Action", "Post", "Status"]);
        table.add_row(vec![self.action, self.url.as_str(), "OK"]);
        table
    }
}

impl CsvRenderable for ActionResult {
    fn csv_headers() -> Vec<&'static str> {
        vec!["action", "id", "url", "undo"]
    }

    fn csv_rows(&self) -> Vec<Vec<String>> {
        vec![vec![
            self.action.to_string(),
            self.id.clone(),
            self.url.clone(),
            self.undo.clone().unwrap_or_default(),
        ]]
    }
}

fn render_success(format: OutputFormat, action: &'static str, id: &str, undo: Option<&str>, via: Option<&'static str>) {
    // Silently log engagement to intelligence store
    if let Ok(store) = crate::intel::store::IntelStore::open() {
        let _ = store.log_engagement(action, Some(id), None, None, None);
    }
    let display = ActionResult {
        action,
        id: id.to_string(),
        url: post_url(id),
        undo: undo.map(|u| format!("xmaster {u} {id}")),
        via,
    };
    if format == OutputFormat::Table {
        if let Some(u) = &display.undo {
            eprintln!("Undo: {u}");
        }
    }
    output::render_csv(format, &display, None);
}

pub async fn delete(ctx: Arc<AppContext>, format: OutputFormat, id: &str) -> Result<(), XmasterError> {
    XApi::new(ctx).delete_tweet(id).await.map_err(reclassify_403)?;
    render_success(format, "delete", id, None, None);
    Ok(())
}

pub async fn like(ctx: Arc<AppContext>, format: OutputFormat, id: &str) -> Result<(), XmasterError> {
    let via = WriteRoute::web_only(&ctx);
    XApi::new(ctx).like_tweet(id).await.map_err(reclassify_403)?;
    render_success(format, "like", id, Some("unlike"), via);
    Ok(())
}

pub async fn unlike(ctx: Arc<AppContext>, format: OutputFormat, id: &str) -> Result<(), XmasterError> {
    let via = WriteRoute::web_only(&ctx);
    XApi::new(ctx).unlike_tweet(id).await.map_err(reclassify_403)?;
    render_success(format, "unlike", id, Some("like"), via);
    Ok(())
}

pub async fn retweet(ctx: Arc<AppContext>, format: OutputFormat, id: &str) -> Result<(), XmasterError> {
    XApi::new(ctx).retweet(id).await.map_err(reclassify_403)?;
    render_success(format, "repost", id, Some("unrepost"), None);
    Ok(())
}

pub async fn unretweet(ctx: Arc<AppContext>, format: OutputFormat, id: &str) -> Result<(), XmasterError> {
    XApi::new(ctx).unretweet(id).await.map_err(reclassify_403)?;
    render_success(format, "unrepost", id, Some("repost"), None);
    Ok(())
}

pub async fn bookmark(ctx: Arc<AppContext>, format: OutputFormat, id: &str) -> Result<(), XmasterError> {
    XApi::new(ctx).bookmark_tweet(id).await.map_err(reclassify_403)?;
    render_success(format, "bookmark", id, Some("unbookmark"), None);
    Ok(())
}

pub async fn unbookmark(ctx: Arc<AppContext>, format: OutputFormat, id: &str) -> Result<(), XmasterError> {
    XApi::new(ctx).unbookmark_tweet(id).await.map_err(reclassify_403)?;
    render_success(format, "unbookmark", id, Some("bookmark"), None);
    Ok(())
}
