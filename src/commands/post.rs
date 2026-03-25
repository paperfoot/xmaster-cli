use crate::cli::parse_tweet_id;
use crate::context::AppContext;
use crate::errors::XmasterError;
use crate::intel::preflight;
use crate::intel::store::IntelStore;
use crate::output::{self, OutputFormat, Tableable};
use crate::providers::xapi::XApi;
use serde::Serialize;
use std::sync::Arc;

#[derive(Serialize)]
struct PostResult {
    id: String,
    text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    preflight_score: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    preflight_grade: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    warnings: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    suggested_next_commands: Vec<String>,
}

impl Tableable for PostResult {
    fn to_table(&self) -> comfy_table::Table {
        let mut table = comfy_table::Table::new();
        table.set_header(vec!["Field", "Value"]);
        table.add_row(vec!["Tweet ID", &self.id]);
        table.add_row(vec!["Text", &self.text]);
        if let Some(score) = self.preflight_score {
            let grade = self.preflight_grade.as_deref().unwrap_or("?");
            table.add_row(vec!["Quality", &format!("{score}/100 ({grade})")]);
        }
        for w in &self.warnings {
            table.add_row(vec!["Warning", w]);
        }
        table
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn execute(
    ctx: Arc<AppContext>,
    format: OutputFormat,
    text: &str,
    reply_to: Option<&str>,
    quote: Option<&str>,
    media: &[String],
    poll: Option<&str>,
    poll_duration: u64,
) -> Result<(), XmasterError> {
    let api = XApi::new(ctx.clone());

    // ── Pre-flight analysis (skip for replies — scoring is tuned for standalone posts) ──
    let is_reply = reply_to.is_some();
    let analysis = preflight::analyze(text, None);
    let mut warnings = Vec::new();

    if !is_reply {
        // Only flag issues on standalone posts; replies have different rules
        for issue in &analysis.issues {
            if issue.severity == preflight::Severity::Critical {
                warnings.push(format!("[CRITICAL] {}: {}", issue.code, issue.message));
            } else if issue.severity == preflight::Severity::Warning {
                warnings.push(format!("[WARN] {}", issue.message));
            }
        }

        // Show warnings in table mode (non-intrusively on stderr)
        if format == OutputFormat::Table && !warnings.is_empty() {
            eprintln!("--- Pre-flight ({}/100, {}) ---", analysis.score, analysis.grade);
            for w in &warnings {
                eprintln!("  {w}");
            }
            if !analysis.suggestions.is_empty() {
                eprintln!("  Tip: {}", analysis.suggestions[0]);
            }
            eprintln!("---");
        }
    }

    // ── Cannibalization check (is a recent post still gaining traction?) ──
    if let Ok(store) = IntelStore::open() {
        let velocity = store.get_recent_post_velocity();
        if let Ok(v) = velocity {
            if let Some(ref accel_id) = v.accelerating_post {
                if format == OutputFormat::Table {
                    eprintln!(
                        "Note: Your post {} is still gaining traction. Consider waiting.",
                        &accel_id[..accel_id.len().min(12)]
                    );
                }
            }
        }
    } else if format == OutputFormat::Table {
        eprintln!("Warning: Could not open intelligence store");
    }

    // ── Execute the post ──
    let reply_id = reply_to.map(parse_tweet_id);
    let quote_id = quote.map(parse_tweet_id);

    let media_ids = if !media.is_empty() {
        let mut ids = Vec::new();
        for path in media {
            let id = api.upload_media(path).await?;
            ids.push(id);
        }
        Some(ids)
    } else {
        None
    };

    let poll_options: Option<Vec<String>> = poll.map(|p| {
        p.split(',').map(|s| s.trim().to_string()).collect()
    });

    // Validate poll options: 2-4 choices, each max 25 chars
    if let Some(ref opts) = poll_options {
        if opts.len() < 2 || opts.len() > 4 || opts.iter().any(|o| o.len() > 25) {
            return Err(XmasterError::Config(
                "Poll must have 2-4 options, max 25 chars each".into(),
            ));
        }
    }

    let result = api
        .create_tweet(
            text,
            reply_id.as_deref(),
            quote_id.as_deref(),
            media_ids.as_deref(),
            poll_options.as_deref(),
            Some(poll_duration),
        )
        .await
        .map_err(|err| {
            // ReplyRestricted already has a clear message — pass through
            if matches!(&err, XmasterError::ReplyRestricted(_)) {
                return err;
            }
            if let XmasterError::AuthMissing { provider, ref message } = err {
                if message.contains("403") {
                    return XmasterError::Api {
                        provider,
                        code: "forbidden",
                        message: format!(
                            "{message}. Hint: Check your app permissions — ensure Read+Write is enabled"
                        ),
                    };
                }
            }
            err
        })?;

    // ── Log to intelligence store (silent, never fails the post) ──
    let content_type = if reply_to.is_some() {
        "reply"
    } else if quote.is_some() {
        "quote"
    } else if !media.is_empty() {
        "media"
    } else {
        "text"
    };

    if let Ok(store) = IntelStore::open() {
        let _ = store.log_post(
            &result.id,
            text,
            content_type,
            reply_id.as_deref(),
            quote_id.as_deref(),
            Some(analysis.score as f64),
        );
    } else if format == OutputFormat::Table {
        eprintln!("Warning: Could not open intelligence store");
    }

    // ── Build response with intelligence metadata ──
    let tweet_id = result.id.clone();
    let mut suggested_next = vec![
        format!("xmaster metrics {tweet_id}"),
    ];
    if !is_reply && analysis.score < 60 {
        suggested_next.push("Consider: xmaster analyze \"text\" before posting next time".into());
    }

    let display = PostResult {
        id: result.id,
        text: result.text,
        preflight_score: if is_reply { None } else { Some(analysis.score) },
        preflight_grade: if is_reply { None } else { Some(analysis.grade) },
        warnings: if format == OutputFormat::Json && !is_reply { warnings } else { vec![] },
        suggested_next_commands: suggested_next,
    };
    output::render(format, &display, None);

    // Undo hint
    if format == OutputFormat::Table {
        eprintln!("Delete: xmaster delete {tweet_id}");
    }
    Ok(())
}
