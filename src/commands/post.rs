use crate::cli::parse_tweet_id;
use crate::context::AppContext;
use crate::errors::XmasterError;
use crate::intel::preflight::{self, AnalyzeContext, MediaKind, PostMode};
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

    // ── Pre-flight analysis ──
    let is_reply = reply_to.is_some();
    let is_quote = quote.is_some();
    let mode = if is_reply {
        PostMode::Reply
    } else if is_quote {
        PostMode::Quote
    } else {
        PostMode::Standalone
    };
    let media_kind = if !media.is_empty() {
        let first = media[0].to_lowercase();
        if first.ends_with(".mp4") || first.ends_with(".mov") || first.ends_with(".webm") {
            Some(MediaKind::Video)
        } else if first.ends_with(".gif") {
            Some(MediaKind::Gif)
        } else {
            Some(MediaKind::Image)
        }
    } else {
        None
    };
    let analyze_ctx = AnalyzeContext {
        goal: None,
        mode: Some(mode),
        has_media: !media.is_empty(),
        media_kind,
        has_poll: poll.is_some(),
        target_text: None,
        author_voice: if ctx.config.style.voice.is_empty() { None } else { Some(ctx.config.style.voice.clone()) },
        premium: ctx.config.account.premium,
    };
    let analysis = preflight::analyze(text, &analyze_ctx);
    let mut warnings = Vec::new();

    // Always show critical issues (over_limit, empty); standalone posts get
    // all warnings; replies suppress most standalone-only warnings but KEEP
    // reply-quality warnings (too_short, generic, emoji_only) since those are
    // the whole point of analyzing a reply.
    for issue in &analysis.issues {
        let is_reply_quality = matches!(
            issue.code.as_str(),
            "reply_too_short" | "reply_generic" | "reply_emoji_only"
        );
        if issue.severity == preflight::Severity::Critical {
            warnings.push(format!("[CRITICAL] {}: {}", issue.code, issue.message));
        } else if issue.severity == preflight::Severity::Warning
            && (!is_reply || is_reply_quality)
        {
            warnings.push(format!("[WARN] {}", issue.message));
        }
    }

    // Show warnings in table mode (non-intrusively on stderr)
    if format == OutputFormat::Table && !warnings.is_empty() {
        eprintln!("--- Pre-flight ({}/100, {}) ---", analysis.score, analysis.grade);
        for w in &warnings {
            eprintln!("  {w}");
        }
        if !is_reply && !analysis.suggestions.is_empty() {
            eprintln!("  Tip: {}", analysis.suggestions[0]);
        }
        eprintln!("---");
    }

    // ── Cannibalization check (is a recent post still gaining traction?) ──
    // The algorithm's author_diversity_scorer applies exponential decay to
    // repeated authors in a feed session. Posting while a previous post is
    // in its 30-60 min traction window actively hurts both posts.
    // Standalone post-velocity (posts_6h / standalone_24h) is warned by
    // preflight above, so we only need to add the accelerating-post warning
    // here — that one isn't computed by preflight.
    if !is_reply {
        if let Ok(store) = IntelStore::open() {
            if let Ok(v) = store.get_recent_post_velocity() {
                if let Some(ref accel_id) = v.accelerating_post {
                    let warning = format!(
                        "Your post {} is still gaining traction — posting now splits the 30-60 min distribution window",
                        &accel_id[..accel_id.len().min(12)]
                    );
                    warnings.push(format!("[WARN] {warning}"));
                    if format == OutputFormat::Table {
                        eprintln!("Warning: {warning}");
                    }
                }
            }
        }
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
        let analysis_json_str = serde_json::to_string(&analysis).ok();
        let _ = store.record_published_post(
            &result.id,
            text,
            content_type,
            reply_id.as_deref(),
            quote_id.as_deref(),
            Some(analysis.score as f64),
            analysis_json_str.as_deref(),
            None,
        );

        // Log reply for reciprocity tracking with style classification.
        // ALWAYS log the reply even if we can't fetch the target tweet
        // (deleted tweet, rate limit, network blip). We lose follower count
        // on the failure path but preserve the correlation so the reply
        // participates in hot-targets, reply-back tracking, and engage
        // recommend scoring. Previously a get_tweet failure silently
        // dropped the entire log_reply call.
        if let Some(ref target_id) = reply_id {
            let style = IntelStore::classify_reply_style(text);
            let (target_uid, target_uname, target_followers) =
                match api.get_tweet(target_id).await {
                    Ok(t) => (
                        t.author_id.clone(),
                        t.author_username.clone(),
                        t.author_followers.map(|f| f as i64),
                    ),
                    Err(_) => (None, None, None),
                };
            let _ = store.log_reply(
                target_id,
                target_uid.as_deref(),
                target_uname.as_deref(),
                target_followers,
                &result.id,
                Some(&style),
            );
        }
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
