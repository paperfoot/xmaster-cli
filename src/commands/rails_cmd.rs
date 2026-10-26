//! `xmaster rails ...`: inspect and steer the pacing rails (write budget,
//! cooldowns, pause). See `intel::rails`.

use crate::context::AppContext;
use crate::errors::XmasterError;
use crate::intel::rails::{self, CapWrite, RailsStatus};
use crate::output::{self, OutputFormat, Tableable};
use serde::Serialize;
use std::sync::Arc;

impl Tableable for RailsStatus {
    fn to_table(&self) -> comfy_table::Table {
        let mut table = comfy_table::Table::new();
        table.set_header(vec!["Action", "Used 24h", "Cap 24h", "Used 1h", "Next allowed", "Why"]);
        let opt = |v: Option<u32>| v.map(|n| n.to_string()).unwrap_or_else(|| "-".into());
        for a in &self.actions {
            let next = if a.wait_secs == 0 { "now".to_string() } else { format!("in {}s ({})", a.wait_secs, a.next_allowed_at) };
            let used_1h = match (a.used_1h, a.cap_1h) {
                (Some(u), Some(c)) => format!("{u}/{c}"),
                _ => "-".into(),
            };
            table.add_row(vec![
                a.action.to_string(),
                a.used_24h.to_string(),
                opt(a.cap_24h),
                used_1h,
                next,
                a.reason.clone(),
            ]);
        }
        let mut notes = Vec::new();
        if !self.enabled {
            notes.push("Rails OFF (XMASTER_RAILS=off)".to_string());
        }
        if let Some(t) = &self.paused_until {
            notes.push(format!("Paused until {t}"));
        }
        if let Some(t) = &self.cooldown_until {
            let why = self.cooldown_reason.as_deref().unwrap_or("X challenge");
            notes.push(format!("COOLDOWN until {t} ({why}): clear it on x.com, then `xmaster rails reset-cooldown --yes`"));
        }
        if let Some(t) = &self.half_caps_until {
            notes.push(format!("Caps halved until {t}"));
        }
        for id in &self.possible_silent_fails {
            notes.push(format!("Possible silent fail: https://x.com/i/status/{id}"));
        }
        // The writes each cap counts, newest five per action
        for a in self.actions.iter().filter(|a| !a.writes_24h.is_empty()) {
            let earlier = a.writes_24h.len().saturating_sub(5);
            let mut list: Vec<String> = a.writes_24h[earlier..].iter().map(write_label).collect();
            if earlier > 0 {
                list.insert(0, format!("+{earlier} earlier"));
            }
            notes.push(format!("{} in 24h: {}", a.action, list.join(", ")));
        }
        for n in notes {
            table.add_row(vec![n]);
        }
        table
    }
}

/// `14:02Z @alice 1234 (pending)`: UTC time, then whatever the ledger kept.
fn write_label(w: &CapWrite) -> String {
    let mut label = w.at.get(11..16).map(|t| format!("{t}Z")).unwrap_or_else(|| w.at.clone());
    for part in [w.author.as_ref().map(|a| format!("@{a}")), w.target_id.clone(), w.status.map(|s| format!("({s})"))]
        .into_iter()
        .flatten()
    {
        label.push(' ');
        label.push_str(&part);
    }
    label
}

#[derive(Serialize)]
struct Changed {
    action: &'static str,
    changed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    until: Option<String>,
    message: String,
}

impl Tableable for Changed {
    fn to_table(&self) -> comfy_table::Table {
        let mut table = comfy_table::Table::new();
        table.set_header(vec!["Rails", "Result"]);
        table.add_row(vec![self.action, self.message.as_str()]);
        table
    }
}

pub async fn status(ctx: Arc<AppContext>, format: OutputFormat) -> Result<(), XmasterError> {
    let st = rails::status(&ctx)?;
    output::render(format, &st, None);
    Ok(())
}

pub async fn pause(format: OutputFormat, hours: Option<f64>) -> Result<(), XmasterError> {
    let until = rails::pause(hours)?;
    let message = match &until {
        Some(t) => format!("All writes paused until {t}"),
        None => "All writes paused until `xmaster rails resume`".into(),
    };
    output::render(format, &Changed { action: "pause", changed: true, until, message }, None);
    Ok(())
}

pub async fn resume(format: OutputFormat) -> Result<(), XmasterError> {
    let changed = rails::resume()?;
    let message = if changed { "Writes resumed" } else { "Writes weren't paused" }.to_string();
    output::render(format, &Changed { action: "resume", changed, until: None, message }, None);
    Ok(())
}

pub async fn reset_cooldown(ctx: Arc<AppContext>, format: OutputFormat, yes: bool) -> Result<(), XmasterError> {
    if !yes {
        return Err(XmasterError::Config(
            "reset-cooldown re-enables writes after X challenged the account. First open x.com in \
             your browser and clear any lock, captcha or verification, then rerun with --yes"
                .into(),
        ));
    }
    let changed = rails::reset_cooldown(&ctx)?;
    let message = if changed {
        "Cooldown cleared; caps stay halved for the next days (see `xmaster rails status`)"
    } else {
        "No cooldown was active; cleared any parked action types"
    }
    .to_string();
    output::render(format, &Changed { action: "reset-cooldown", changed, until: None, message }, None);
    Ok(())
}
