use crate::context::AppContext;
use crate::errors::XmasterError;
use crate::intel::scheduler::PostScheduler;
use crate::output::{self, OutputFormat, Tableable};
use chrono::{Local, NaiveDateTime, TimeZone, Utc};
use serde::Serialize;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Result types
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct ScheduleAddResult {
    id: String,
    content_preview: String,
    scheduled_at: String,
    scheduled_at_unix: i64,
    timezone: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    preflight_score: Option<i32>,
    auto_scheduled: bool,
}

impl Tableable for ScheduleAddResult {
    fn to_table(&self) -> comfy_table::Table {
        let mut table = comfy_table::Table::new();
        table.set_header(vec!["Field", "Value"]);
        table.add_row(vec!["ID", &self.id]);
        table.add_row(vec!["Content", &self.content_preview]);
        table.add_row(vec!["Scheduled At", &self.scheduled_at]);
        table.add_row(vec!["Timezone", &self.timezone]);
        if self.auto_scheduled {
            table.add_row(vec!["Auto-Scheduled", "yes (best engagement time)"]);
        }
        if let Some(score) = self.preflight_score {
            table.add_row(vec!["Quality Score", &format!("{score}/100")]);
        }
        table
    }
}

#[derive(Serialize)]
struct ScheduleListResult {
    items: Vec<ScheduleItem>,
    total: usize,
}

#[derive(Serialize)]
struct ScheduleItem {
    id: String,
    content_preview: String,
    scheduled_at: String,
    scheduled_at_unix: i64,
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    preflight_score: Option<i32>,
}

impl Tableable for ScheduleListResult {
    fn to_table(&self) -> comfy_table::Table {
        let mut table = comfy_table::Table::new();
        table.set_header(vec!["ID", "Content", "Scheduled At", "Status", "Score"]);
        for item in &self.items {
            let preview = truncate(&item.content_preview, 60);
            let score = item
                .preflight_score
                .map(|s| format!("{s}"))
                .unwrap_or_else(|| "-".into());
            table.add_row(vec![
                &item.id,
                &preview,
                &item.scheduled_at,
                &item.status,
                &score,
            ]);
        }
        table
    }
}

#[derive(Serialize)]
struct ScheduleActionResult {
    action: String,
    id: String,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    new_time: Option<String>,
}

impl Tableable for ScheduleActionResult {
    fn to_table(&self) -> comfy_table::Table {
        let mut table = comfy_table::Table::new();
        table.set_header(vec!["Field", "Value"]);
        table.add_row(vec!["Action", &self.action]);
        table.add_row(vec!["ID", &self.id]);
        table.add_row(vec!["Status", &self.message]);
        if let Some(ref t) = self.new_time {
            table.add_row(vec!["New Time", t]);
        }
        table
    }
}

#[derive(Serialize)]
struct FireDisplayResult {
    fired: u32,
    failed: u32,
    missed: u32,
    details: Vec<FireDetail>,
}

#[derive(Serialize)]
struct FireDetail {
    id: String,
    content_preview: String,
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    tweet_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

impl Tableable for FireDisplayResult {
    fn to_table(&self) -> comfy_table::Table {
        let mut table = comfy_table::Table::new();
        table.set_header(vec!["ID", "Content", "Status", "Tweet ID"]);
        for d in &self.details {
            let preview = truncate(&d.content_preview, 50);
            let tweet = d.tweet_id.as_deref().unwrap_or("-");
            let status_str = if let Some(ref err) = d.error {
                format!("{} ({})", d.status, err)
            } else {
                d.status.clone()
            };
            table.add_row(vec![&d.id, &preview, &status_str, tweet]);
        }
        table
    }
}

#[derive(Serialize)]
struct SetupResult {
    plist_path: String,
    loaded: bool,
    message: String,
}

impl Tableable for SetupResult {
    fn to_table(&self) -> comfy_table::Table {
        let mut table = comfy_table::Table::new();
        table.set_header(vec!["Field", "Value"]);
        table.add_row(vec!["Plist", &self.plist_path]);
        table.add_row(vec!["Loaded", &self.loaded.to_string()]);
        table.add_row(vec!["Status", &self.message]);
        table
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn truncate(s: &str, max: usize) -> String {
    if s.len() > max {
        format!("{}...", &s[..max.saturating_sub(3)])
    } else {
        s.to_string()
    }
}

/// Parse the `--at` argument into a UTC unix timestamp.
/// Accepts "auto" or ISO-like datetime strings interpreted as local time.
fn parse_scheduled_time(at: &str) -> Result<(i64, bool), XmasterError> {
    if at.eq_ignore_ascii_case("auto") {
        return Ok((0, true)); // caller handles auto
    }

    let formats = [
        "%Y-%m-%d %H:%M",
        "%Y-%m-%dT%H:%M",
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%dT%H:%M:%S",
    ];

    for fmt in &formats {
        if let Ok(naive) = NaiveDateTime::parse_from_str(at, fmt) {
            // Interpret as local time, convert to UTC
            let local_dt = Local
                .from_local_datetime(&naive)
                .single()
                .ok_or_else(|| {
                    XmasterError::Config(format!(
                        "Ambiguous local datetime '{at}' (DST transition?)"
                    ))
                })?;
            return Ok((local_dt.with_timezone(&Utc).timestamp(), false));
        }
    }

    Err(XmasterError::Config(format!(
        "Invalid datetime '{at}'. Use ISO format like '2026-03-24 09:00' or 'auto'"
    )))
}

fn format_timestamp(ts: i64) -> String {
    chrono::DateTime::from_timestamp(ts, 0)
        .map(|dt| dt.with_timezone(&Local).format("%Y-%m-%d %H:%M %Z").to_string())
        .unwrap_or_else(|| ts.to_string())
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

pub async fn add(
    _ctx: Arc<AppContext>,
    format: OutputFormat,
    content: &str,
    at: &str,
    reply_to: Option<&str>,
    quote: Option<&str>,
    media: &[String],
) -> Result<(), XmasterError> {
    let scheduler = PostScheduler::open()?;
    let tz = Local::now().offset().to_string();
    let (parsed_ts, is_auto) = parse_scheduled_time(at)?;

    let scheduled_ts = if is_auto {
        scheduler.get_best_auto_time().ok_or_else(|| {
            XmasterError::Config(
                "Not enough engagement data for auto-scheduling. Use an explicit time instead, or track more posts first.".into()
            )
        })?
    } else {
        parsed_ts
    };

    // Determine content type
    let content_type = if reply_to.is_some() {
        "reply"
    } else if quote.is_some() {
        "quote"
    } else if !media.is_empty() {
        "media"
    } else {
        "text"
    };

    let media_slice = if media.is_empty() { None } else { Some(media) };

    let entry = scheduler.add(
        content,
        scheduled_ts,
        &tz,
        content_type,
        reply_to,
        quote,
        media_slice,
        is_auto,
    )?;

    let result = ScheduleAddResult {
        id: entry.id.clone(),
        content_preview: truncate(content, 80),
        scheduled_at: format_timestamp(entry.scheduled_at),
        scheduled_at_unix: entry.scheduled_at,
        timezone: tz,
        preflight_score: entry.preflight_score,
        auto_scheduled: is_auto,
    };
    output::render(format, &result, None);

    if format == OutputFormat::Table {
        eprintln!("Cancel: xmaster schedule cancel {}", entry.id);
        eprintln!(
            "Reschedule: xmaster schedule reschedule {} --at \"TIME\"",
            entry.id
        );
    }
    Ok(())
}

pub async fn list(
    format: OutputFormat,
    status_filter: Option<&str>,
) -> Result<(), XmasterError> {
    let scheduler = PostScheduler::open()?;
    let entries = scheduler.list(status_filter)?;

    if entries.is_empty() {
        output::render_error(
            format,
            "no_scheduled_posts",
            "No scheduled posts found",
            "Schedule a post: xmaster schedule add \"text\" --at \"2026-03-24 09:00\"",
        );
        return Ok(());
    }

    let total = entries.len();
    let items: Vec<ScheduleItem> = entries
        .into_iter()
        .map(|e| ScheduleItem {
            id: e.id,
            content_preview: e.content,
            scheduled_at: format_timestamp(e.scheduled_at),
            scheduled_at_unix: e.scheduled_at,
            status: e.status,
            preflight_score: e.preflight_score,
        })
        .collect();

    let result = ScheduleListResult { items, total };
    output::render(format, &result, None);
    Ok(())
}

pub async fn cancel(format: OutputFormat, id: &str) -> Result<(), XmasterError> {
    let scheduler = PostScheduler::open()?;
    scheduler.cancel(id)?;

    let result = ScheduleActionResult {
        action: "cancel".into(),
        id: id.to_string(),
        message: "Scheduled post cancelled".into(),
        new_time: None,
    };
    output::render(format, &result, None);
    Ok(())
}

pub async fn reschedule(
    format: OutputFormat,
    id: &str,
    at: &str,
) -> Result<(), XmasterError> {
    let scheduler = PostScheduler::open()?;
    let (parsed_ts, is_auto) = parse_scheduled_time(at)?;

    let new_ts = if is_auto {
        scheduler.get_best_auto_time().ok_or_else(|| {
            XmasterError::Config(
                "Not enough engagement data for auto-scheduling. Use an explicit time.".into(),
            )
        })?
    } else {
        parsed_ts
    };

    scheduler.reschedule(id, new_ts)?;

    let result = ScheduleActionResult {
        action: "reschedule".into(),
        id: id.to_string(),
        message: "Post rescheduled".into(),
        new_time: Some(format_timestamp(new_ts)),
    };
    output::render(format, &result, None);
    Ok(())
}

pub async fn fire(
    ctx: Arc<AppContext>,
    format: OutputFormat,
) -> Result<(), XmasterError> {
    let scheduler = PostScheduler::open()?;
    let fire_result = scheduler.fire(ctx, 5).await?;

    let details: Vec<FireDetail> = fire_result
        .posts
        .into_iter()
        .map(|p| FireDetail {
            id: p.id,
            content_preview: p.content_preview,
            status: p.status,
            tweet_id: p.tweet_id,
            error: p.error,
        })
        .collect();

    let result = FireDisplayResult {
        fired: fire_result.fired,
        failed: fire_result.failed,
        missed: fire_result.missed,
        details,
    };
    output::render(format, &result, None);

    if format == OutputFormat::Table {
        eprintln!(
            "Fired {} posts, {} failed, {} missed window",
            result.fired, result.failed, result.missed
        );
    }
    Ok(())
}

pub async fn setup(format: OutputFormat) -> Result<(), XmasterError> {
    let home = std::env::var("HOME")
        .map_err(|_| XmasterError::Config("HOME environment variable not set".into()))?;

    let xmaster_bin = std::env::current_exe()
        .unwrap_or_else(|_| std::path::PathBuf::from("/usr/local/bin/xmaster"));

    let plist_dir = format!("{home}/Library/LaunchAgents");
    let plist_path = format!("{plist_dir}/com.xmaster.schedule.plist");

    let plist_content = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.xmaster.schedule</string>
    <key>ProgramArguments</key>
    <array>
        <string>{}</string>
        <string>schedule</string>
        <string>fire</string>
    </array>
    <key>StartInterval</key>
    <integer>60</integer>
    <key>StandardOutPath</key>
    <string>{home}/Library/Logs/xmaster-schedule.log</string>
    <key>StandardErrorPath</key>
    <string>{home}/Library/Logs/xmaster-schedule.err</string>
    <key>RunAtLoad</key>
    <true/>
</dict>
</plist>
"#,
        xmaster_bin.display()
    );

    std::fs::create_dir_all(&plist_dir)?;
    std::fs::write(&plist_path, &plist_content)?;

    let load_result = std::process::Command::new("launchctl")
        .args(["load", &plist_path])
        .output();

    let loaded = match load_result {
        Ok(out) => out.status.success(),
        Err(_) => false,
    };

    let message = if loaded {
        "Scheduler installed and running. Posts will fire every 60 seconds.".into()
    } else {
        format!(
            "Plist written but launchctl load failed. Run: launchctl load \"{}\"",
            plist_path
        )
    };

    let result = SetupResult {
        plist_path: plist_path.clone(),
        loaded,
        message,
    };
    output::render(format, &result, None);

    if format == OutputFormat::Table {
        eprintln!(
            "Uninstall: launchctl unload \"{}\" && rm \"{}\"",
            plist_path, plist_path
        );
    }
    Ok(())
}
