use crate::commands::{post_url, reclassify_403};
use crate::context::AppContext;
use crate::errors::XmasterError;
use crate::intel::judge::DraftFlag;
use crate::intel::preflight::{self, AnalyzeContext, MediaKind, PostMode};
use crate::intel::store::IntelStore;
use crate::output::{self, OutputFormat, Tableable};
use crate::providers::xapi::XApi;
use crate::utils::weighted_len;
use serde::Serialize;
use std::sync::Arc;

/// X API pay-per-use prices (devcommunity post 263025, 2026-04-20).
const API_POST_USD: f64 = 0.015;
const API_POST_WITH_URL_USD: f64 = 0.20;

/// One post to publish: `post`, `reply` and `quote` all build one.
pub struct Draft<'a> {
    pub text: &'a str,
    pub reply_to: Option<&'a str>,
    pub quote: Option<&'a str>,
    pub media: &'a [String],
    pub poll: Option<&'a str>,
    pub poll_duration: u64,
}

#[derive(Serialize)]
struct PostResult {
    id: String,
    url: String,
    text: String,
    /// Removes the post again.
    undo: String,
    /// "web" (browser session, free) or "api" (paid), when the path was fixed.
    #[serde(skip_serializing_if = "Option::is_none")]
    via: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    preflight_score: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    preflight_grade: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    warnings: Vec<String>,
    /// JEV draft flags (generic, slop, off_topic, bait). Advisory: the post
    /// went out; use them to rewrite the next one.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    jev_flags: Vec<DraftFlag>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    suggested_next_commands: Vec<String>,
}

impl Tableable for PostResult {
    fn to_table(&self) -> comfy_table::Table {
        let mut table = comfy_table::Table::new();
        table.set_header(vec!["Field", "Value"]);
        table.add_row(vec!["Post", &self.url]);
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

/// `--dry-run`: what would be posted, by which path, at what cost, and what
/// the draft checks say. Nothing is uploaded or posted.
#[derive(Serialize)]
struct DryRun<'a> {
    dry_run: bool,
    /// No critical pre-flight issue: posting it now should go through.
    ready: bool,
    kind: &'static str,
    text: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    reply_to: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    quote: Option<&'a str>,
    #[serde(skip_serializing_if = "<[String]>::is_empty")]
    media: &'a [String],
    #[serde(skip_serializing_if = "Option::is_none")]
    poll: Option<Vec<String>>,
    /// X's weighted length (URLs 23, CJK and emoji 2) against the limit.
    length: usize,
    max_length: usize,
    /// "web" (browser session) or "api" (paid X API).
    route: &'static str,
    est_cost_usd: f64,
    preflight_score: u32,
    preflight_grade: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    warnings: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    jev_flags: Vec<DraftFlag>,
}

impl Tableable for DryRun<'_> {
    fn to_table(&self) -> comfy_table::Table {
        let mut table = comfy_table::Table::new();
        table.set_header(vec!["Dry run", "Value"]);
        table.add_row(vec!["Kind", self.kind]);
        table.add_row(vec!["Text", self.text]);
        table.add_row(vec!["Length", &format!("{}/{}", self.length, self.max_length)]);
        table.add_row(vec!["Route", &format!("{} (~${:.3})", self.route, self.est_cost_usd)]);
        table.add_row(vec!["Quality", &format!("{}/100 ({})", self.preflight_score, self.preflight_grade)]);
        table.add_row(vec!["Ready", if self.ready { "yes" } else { "no" }]);
        for w in &self.warnings {
            table.add_row(vec!["Warning", w]);
        }
        for f in &self.jev_flags {
            table.add_row(vec![f.code, f.fix]);
        }
        table
    }
}

/// Does the text carry a link X will wrap in t.co (and bill as a URL post)?
fn has_link(text: &str) -> bool {
    text.split_whitespace().any(|w| {
        let w = w.trim_end_matches(['.', ',', ';', ':', '!', '?', ')', '"', '\'', ']']);
        w.contains('.') && weighted_len(w) == 23
    })
}

/// The path `XApi::create_tweet` takes for this draft, and whether that's
/// certain (`false` when a web failure could still fall back to the API).
/// Mirrors xapi.rs: polls are API-only; replies, quotes and long Premium
/// posts go web-first; plain posts follow settings.web_writes.
fn route(ctx: &AppContext, d: &Draft) -> (&'static str, bool) {
    let long_premium = weighted_len(d.text) > 280 && ctx.config.account.premium;
    if d.poll.is_some() {
        return ("api", true);
    }
    let blocked_on_api = d.reply_to.is_some() || d.quote.is_some() || long_premium;
    if !ctx.config.web_first(blocked_on_api) {
        return ("api", true);
    }
    ("web", d.quote.is_some() || long_premium)
}

fn kind(d: &Draft) -> &'static str {
    match (d.reply_to, d.quote, d.poll) {
        (Some(_), _, _) => "reply",
        (_, Some(_), _) => "quote",
        (_, _, Some(_)) => "poll",
        _ => "post",
    }
}

pub async fn execute(
    ctx: Arc<AppContext>,
    format: OutputFormat,
    draft: Draft<'_>,
    dry_run: bool,
) -> Result<(), XmasterError> {
    let Draft { text, reply_to, quote, media, poll, poll_duration } = draft;
    let api = XApi::new(ctx.clone());

    // Poll options: 2-4 choices, each max 25 characters (not bytes). Checked
    // before anything else so bad input never reaches X.
    let poll_options: Option<Vec<String>> = poll.map(|p| p.split(',').map(|s| s.trim().to_string()).collect());
    if let Some(ref opts) = poll_options {
        if opts.len() < 2 || opts.len() > 4 || opts.iter().any(|o| o.is_empty() || o.chars().count() > 25) {
            return Err(XmasterError::InvalidInput("--poll needs 2-4 comma-separated options, max 25 characters each".into()));
        }
    }
    if let Some(missing) = media.iter().find(|m| !std::path::Path::new(m.as_str()).is_file()) {
        return Err(XmasterError::InvalidInput(format!("media file not found: {missing}")));
    }

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

    if dry_run {
        let draft = Draft { text, reply_to, quote, media, poll, poll_duration };
        let (route, _) = route(&ctx, &draft);
        let review = crate::commands::analyze::jev_review(&ctx, text, reply_to.or(quote), false).await;
        let est_cost_usd = match route {
            "web" => 0.0,
            _ if has_link(text) => API_POST_WITH_URL_USD,
            _ => API_POST_USD,
        };
        let mut ready = !analysis.issues.iter().any(|i| i.severity == preflight::Severity::Critical);
        let mut warnings = warnings;
        if route == "api" && quote.is_some() {
            ready = false;
            warnings.push("[CRITICAL] quote_needs_web: X removed quote posts from the API (2026-04-20); run `xmaster config web-login`".into());
        } else if route == "api" && reply_to.is_some() {
            warnings.push("[WARN] The API only accepts replies to people who @mentioned you (Feb 2026); `xmaster config web-login` lets you reply anywhere".into());
        }
        let report = DryRun {
            dry_run: true,
            ready,
            kind: kind(&draft),
            text,
            reply_to,
            quote,
            media,
            poll: poll_options,
            length: weighted_len(text),
            max_length: if ctx.config.account.premium { 25_000 } else { 280 },
            route,
            est_cost_usd,
            preflight_score: analysis.score,
            preflight_grade: analysis.grade.clone(),
            warnings,
            jev_flags: review.map(|r| r.flags).unwrap_or_default(),
        };
        output::render(format, &report, None);
        return Ok(());
    }

    // ── Execute the post ──
    let (via, via_certain) = route(&ctx, &Draft { text, reply_to, quote, media, poll, poll_duration });
    let reply_id = reply_to.map(str::to_string);
    let quote_id = quote.map(str::to_string);

    let media_ids = if !media.is_empty() {
        Some(api.upload_media_many(media).await?)
    } else {
        None
    };

    // The JEV draft guard runs alongside the write, so it adds no latency;
    // it is advisory and never holds the post.
    let (posted, review) = tokio::join!(
        api.create_tweet(
            text,
            reply_id.as_deref(),
            quote_id.as_deref(),
            media_ids.as_deref(),
            poll_options.as_deref(),
            Some(poll_duration),
        ),
        crate::commands::analyze::jev_review(&ctx, text, reply_id.as_deref().or(quote_id.as_deref()), false),
    );
    let jev_flags = review.map(|r| r.flags).unwrap_or_default();
    let result = posted.map_err(reclassify_403)?;

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
    let mut suggested_next = vec![format!("xmaster metrics {tweet_id}")];
    if !is_reply && analysis.score < 60 {
        suggested_next.push("xmaster post \"<text>\" --dry-run (check the next draft first)".into());
    }

    if format == OutputFormat::Table {
        for flag in &jev_flags {
            eprintln!("JEV [{}]: {}", flag.code, flag.fix);
        }
        eprintln!("Delete: xmaster delete {tweet_id}");
    }
    let display = PostResult {
        url: post_url(&result.id),
        undo: format!("xmaster delete {}", result.id),
        via: via_certain.then_some(via),
        id: result.id,
        text: result.text,
        preflight_score: if is_reply { None } else { Some(analysis.score) },
        preflight_grade: if is_reply { None } else { Some(analysis.grade) },
        warnings: if format == OutputFormat::Json && !is_reply { warnings } else { vec![] },
        jev_flags,
        suggested_next_commands: suggested_next,
    };
    output::render(format, &display, None);
    Ok(())
}
