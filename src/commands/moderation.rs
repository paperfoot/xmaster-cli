use crate::cli::parse_tweet_id;
use crate::context::AppContext;
use crate::errors::XmasterError;
use crate::output::{self, CsvRenderable, OutputFormat, Tableable};
use crate::providers::web::Moderation;
use crate::providers::xapi::XApi;
use reqwest::Method;
use serde::Serialize;
use serde_json::{json, Value};
use std::sync::Arc;

const BASE: &str = "https://api.x.com/2";

// OAuth1 signing and request execution now go through XApi::request()
// instead of local boilerplate. This eliminates one of the bypass sites
// catalogued in issue #16.

async fn signed_request(
    ctx: &Arc<AppContext>,
    method: Method,
    url: &str,
    body: Option<Value>,
) -> Result<(), XmasterError> {
    let _val = XApi::new(ctx.clone()).request(method, url, body).await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Display
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct ModerationResult {
    action: String,
    target: String,
    success: bool,
}

impl Tableable for ModerationResult {
    fn to_table(&self) -> comfy_table::Table {
        let mut table = comfy_table::Table::new();
        table.set_header(vec!["Action", "Target", "Status"]);
        table.add_row(vec![
            self.action.as_str(),
            self.target.as_str(),
            if self.success { "OK" } else { "Failed" },
        ]);
        table
    }
}

impl CsvRenderable for ModerationResult {}

fn render_success(format: OutputFormat, action: &str, target: String) {
    let display = ModerationResult {
        action: action.to_string(),
        target,
        success: true,
    };
    output::render(format, &display, None);
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

pub async fn hide_reply(
    ctx: Arc<AppContext>,
    format: OutputFormat,
    id: &str,
) -> Result<(), XmasterError> {
    let tweet_id = parse_tweet_id(id);
    signed_request(
        &ctx,
        Method::PUT,
        &format!("{BASE}/tweets/{tweet_id}/hidden"),
        Some(json!({ "hidden": true })),
    )
    .await?;
    render_success(format, "hide_reply", tweet_id);
    Ok(())
}

pub async fn unhide_reply(
    ctx: Arc<AppContext>,
    format: OutputFormat,
    id: &str,
) -> Result<(), XmasterError> {
    let tweet_id = parse_tweet_id(id);
    signed_request(
        &ctx,
        Method::PUT,
        &format!("{BASE}/tweets/{tweet_id}/hidden"),
        Some(json!({ "hidden": false })),
    )
    .await?;
    render_success(format, "unhide_reply", tweet_id);
    Ok(())
}

pub async fn block(
    ctx: Arc<AppContext>,
    format: OutputFormat,
    username: &str,
) -> Result<(), XmasterError> {
    XApi::new(ctx).moderate(Moderation::Block, username).await?;
    render_success(format, "block", format!("@{username}"));
    Ok(())
}

pub async fn unblock(
    ctx: Arc<AppContext>,
    format: OutputFormat,
    username: &str,
) -> Result<(), XmasterError> {
    XApi::new(ctx).moderate(Moderation::Unblock, username).await?;
    render_success(format, "unblock", format!("@{username}"));
    Ok(())
}

pub async fn mute(
    ctx: Arc<AppContext>,
    format: OutputFormat,
    username: &str,
) -> Result<(), XmasterError> {
    XApi::new(ctx).moderate(Moderation::Mute, username).await?;
    render_success(format, "mute", format!("@{username}"));
    Ok(())
}

pub async fn unmute(
    ctx: Arc<AppContext>,
    format: OutputFormat,
    username: &str,
) -> Result<(), XmasterError> {
    XApi::new(ctx).moderate(Moderation::Unmute, username).await?;
    render_success(format, "unmute", format!("@{username}"));
    Ok(())
}
