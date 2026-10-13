use crate::context::AppContext;
use crate::errors::XmasterError;
use crate::intel::store::IntelStore;
use crate::output::{self, OutputFormat, Tableable};
use serde::Serialize;
use std::sync::Arc;

/// Pattern observations generated from the report data.
#[derive(Serialize)]
struct PatternObservation {
    pattern: String,
    lift: String,
}

#[derive(Serialize)]
struct ReplyOutcomeSummary {
    replies_sent: usize,
    top_targets: Vec<ReplyTargetSummary>,
    avg_reply_impressions: f64,
    avg_profile_clicks: f64,
    reply_back_rate: f64,
}

#[derive(Serialize)]
struct ReplyTargetSummary {
    username: String,
    sample_count: i64,
    avg_impressions: f64,
    score: f64,
}

#[derive(Serialize)]
struct EnhancedReport {
    #[serde(flatten)]
    report: crate::intel::tracker::PerformanceReport,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pattern_observations: Vec<PatternObservation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reply_outcomes: Option<ReplyOutcomeSummary>,
}

impl Tableable for EnhancedReport {
    fn to_table(&self) -> comfy_table::Table {
        let mut table = self.report.to_table();
        for obs in &self.pattern_observations {
            table.add_row(vec!["Pattern", &format!("{} ({})", obs.pattern, obs.lift)]);
        }
        if let Some(ref ro) = self.reply_outcomes {
            table.add_row(vec!["─── Reply Outcomes ───", "──────"]);
            table.add_row(vec!["Replies sent", &ro.replies_sent.to_string()]);
            table.add_row(vec!["Avg reply impressions", &format!("{:.0}", ro.avg_reply_impressions)]);
            table.add_row(vec!["Avg profile clicks", &format!("{:.1}", ro.avg_profile_clicks)]);
            table.add_row(vec!["Reply-back rate", &format!("{:.0}%", ro.reply_back_rate * 100.0)]);
            for (i, t) in ro.top_targets.iter().enumerate() {
                table.add_row(vec![
                    &format!("Top target #{}", i + 1),
                    &format!("@{} ({} replies, {:.0} avg imps, score {:.2})",
                             t.username, t.sample_count, t.avg_impressions, t.score),
                ]);
            }
        }
        table
    }
}

/// Derive pattern observations from the content breakdown.
fn derive_patterns(report: &crate::intel::tracker::PerformanceReport) -> Vec<PatternObservation> {
    let mut observations = Vec::new();

    if report.content_breakdown.len() < 2 || report.avg_engagement_rate == 0.0 {
        return observations;
    }

    // Find content types that significantly outperform the average
    for ct in &report.content_breakdown {
        if ct.count < 2 {
            continue;
        }
        let lift = ct.avg_engagement_rate / report.avg_engagement_rate;
        if lift > 1.2 {
            observations.push(PatternObservation {
                pattern: format!("'{}' posts outperform average", ct.content_type),
                lift: format!("{:.1}x engagement rate", lift),
            });
        } else if lift < 0.7 {
            observations.push(PatternObservation {
                pattern: format!("'{}' posts underperform average", ct.content_type),
                lift: format!("{:.1}x engagement rate", lift),
            });
        }
    }

    // Compare best vs worst post if both exist
    if let (Some(best), Some(worst)) = (&report.best_post, &report.worst_post) {
        if worst.engagement_rate > 0.0 {
            let ratio = best.engagement_rate / worst.engagement_rate;
            if ratio > 3.0 {
                observations.push(PatternObservation {
                    pattern: "Wide variance between best and worst posts".into(),
                    lift: format!("{:.0}x spread — experiment more with what works", ratio),
                });
            }
        }
    }

    observations
}

/// Build reply-outcome summary for the given period. Returns None on any
/// error or if no replies were sent in the window.
fn build_reply_outcomes(store: &IntelStore, days: i64) -> Option<ReplyOutcomeSummary> {
    // Use the same rank_hot_reply_targets method that powers `engage hot-targets`
    let ranked = store.rank_hot_reply_targets(days, 1, 0.0, 0.0).ok()?;
    if ranked.is_empty() {
        return None;
    }

    let total_replies: i64 = ranked.iter().map(|r| r.sample_count).sum();
    let total_imps: f64 = ranked.iter().map(|r| r.avg_impressions * r.sample_count as f64).sum::<f64>();
    let total_clicks: f64 = ranked.iter().map(|r| r.avg_profile_clicks * r.sample_count as f64).sum::<f64>();
    let total_reply_backs: f64 = ranked.iter().map(|r| r.reply_back_rate * r.sample_count as f64).sum::<f64>();

    let top_targets: Vec<ReplyTargetSummary> = ranked
        .iter()
        .take(3)
        .map(|r| ReplyTargetSummary {
            username: r.username.clone(),
            sample_count: r.sample_count,
            avg_impressions: r.avg_impressions,
            score: r.score,
        })
        .collect();

    Some(ReplyOutcomeSummary {
        replies_sent: total_replies as usize,
        top_targets,
        avg_reply_impressions: if total_replies > 0 { total_imps / total_replies as f64 } else { 0.0 },
        avg_profile_clicks: if total_replies > 0 { total_clicks / total_replies as f64 } else { 0.0 },
        reply_back_rate: if total_replies > 0 { total_reply_backs / total_replies as f64 } else { 0.0 },
    })
}

fn render_report(store: &IntelStore, period: &str, format: OutputFormat) -> Result<(), XmasterError> {
    let report = store.generate_report(period)?;

    if report.total_posts == 0 {
        let label = match period {
            "daily" => "24 hours",
            "weekly" => "7 days",
            "monthly" => "30 days",
            _ => "selected period",
        };
        return Err(XmasterError::NotFound(
            format!("No posts found in the last {label}. Post and track content first: `xmaster post \"...\"` then `xmaster track run`"),
        ));
    }

    let patterns = derive_patterns(&report);

    // Reply outcomes only for daily — weekly can be added later.
    let reply_outcomes = match period {
        "daily" => build_reply_outcomes(store, 1),
        _ => None,
    };

    let enhanced = EnhancedReport {
        report,
        pattern_observations: patterns,
        reply_outcomes,
    };
    output::render(format, &enhanced, None);
    Ok(())
}

/// Generate a daily performance report.
pub async fn daily(
    _ctx: Arc<AppContext>,
    format: OutputFormat,
) -> Result<(), XmasterError> {
    render_report(&IntelStore::open()?, "daily", format)
}

/// Generate a weekly performance report.
pub async fn weekly(
    _ctx: Arc<AppContext>,
    format: OutputFormat,
) -> Result<(), XmasterError> {
    render_report(&IntelStore::open()?, "weekly", format)
}
