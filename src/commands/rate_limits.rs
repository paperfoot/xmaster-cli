use crate::context::AppContext;
use crate::errors::XmasterError;
use crate::output::{self, CsvRenderable, OutputFormat, Tableable};
use crate::providers::oauth1::{self, OAuthClientProvider};
use serde::Serialize;
use std::sync::Arc;

#[derive(Serialize)]
struct RateLimitInfo {
    endpoint: String,
    limit: String,
    remaining: String,
    reset: String,
}

#[derive(Serialize)]
struct RateLimitsDisplay {
    entries: Vec<RateLimitInfo>,
}

impl Tableable for RateLimitsDisplay {
    fn to_table(&self) -> comfy_table::Table {
        let mut table = comfy_table::Table::new();
        table.set_header(vec!["Endpoint", "Limit", "Remaining", "Resets At"]);
        for e in &self.entries {
            table.add_row(vec![
                e.endpoint.as_str(),
                e.limit.as_str(),
                e.remaining.as_str(),
                e.reset.as_str(),
            ]);
        }
        if self.entries.is_empty() {
            table.add_row(vec![
                "users/me",
                "\u{2014}",
                "\u{2014}",
                "Headers not available",
            ]);
        }
        table
    }
}

impl CsvRenderable for RateLimitsDisplay {
    fn csv_headers() -> Vec<&'static str> {
        vec!["endpoint", "limit", "remaining", "reset"]
    }
    fn csv_rows(&self) -> Vec<Vec<String>> {
        self.entries
            .iter()
            .map(|e| {
                vec![
                    e.endpoint.clone(),
                    e.limit.clone(),
                    e.remaining.clone(),
                    e.reset.clone(),
                ]
            })
            .collect()
    }
}

fn oauth_secrets(ctx: &AppContext) -> oauth1::Secrets<'_> {
    let k = &ctx.config.keys;
    oauth1::Secrets::new(&k.api_key, &k.api_secret)
        .token(&k.access_token, &k.access_token_secret)
}

pub async fn execute(
    ctx: Arc<AppContext>,
    format: OutputFormat,
) -> Result<(), XmasterError> {
    if !ctx.config.has_x_auth() {
        return Err(XmasterError::AuthMissing {
            provider: "x",
            message: "X API credentials not configured".into(),
        });
    }

    // Make a lightweight call and capture rate limit headers
    let resp = ctx
        .client
        .clone()
        .oauth1(oauth_secrets(&ctx))
        .get("https://api.x.com/2/users/me?user.fields=id")
        .send()
        .await?;

    let headers = resp.headers().clone();

    // Consume response body (required before dropping)
    let status = resp.status();
    let _ = resp.text().await;

    if status == 401 || status == 403 {
        return Err(XmasterError::AuthMissing {
            provider: "x",
            message: format!("HTTP {status}: authentication failed"),
        });
    }

    let limit = headers
        .get("x-rate-limit-limit")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("\u{2014}")
        .to_string();
    let remaining = headers
        .get("x-rate-limit-remaining")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("\u{2014}")
        .to_string();
    let reset_epoch = headers
        .get("x-rate-limit-reset")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("\u{2014}")
        .to_string();

    let reset_display = if let Ok(epoch) = reset_epoch.parse::<i64>() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let diff = epoch - now;
        if diff > 0 {
            format!("{reset_epoch} ({diff}s from now)")
        } else {
            format!("{reset_epoch} (reset)")
        }
    } else {
        reset_epoch.clone()
    };

    let display = RateLimitsDisplay {
        entries: vec![RateLimitInfo {
            endpoint: "GET /2/users/me".into(),
            limit,
            remaining,
            reset: reset_display,
        }],
    };
    output::render(format, &display, None);
    Ok(())
}
