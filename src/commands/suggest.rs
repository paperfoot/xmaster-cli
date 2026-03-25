use crate::context::AppContext;
use crate::errors::XmasterError;
use crate::intel::tracker::{NextPostSuggestion, PostTracker};
use crate::output::{self, OutputFormat};
use std::sync::Arc;

/// Show the best posting times from the engagement heatmap.
pub async fn best_time(
    _ctx: Arc<AppContext>,
    format: OutputFormat,
) -> Result<(), XmasterError> {
    let tracker = PostTracker::open()?;
    let heatmap = tracker.compute_timing_heatmap()?;

    if heatmap.is_empty() {
        return Err(XmasterError::NotFound(
            "Not enough data to compute timing heatmap. Track more posts first: `xmaster track run`".into(),
        ));
    }

    // Show top 10 slots
    let top: Vec<_> = heatmap.into_iter().take(10).collect();
    output::render(format, &top, None);
    Ok(())
}

/// Check cannibalization risk and suggest whether it's safe to post now.
pub async fn next_post(
    _ctx: Arc<AppContext>,
    format: OutputFormat,
) -> Result<(), XmasterError> {
    let tracker = PostTracker::open()?;

    let cannibalization = tracker.check_cannibalization()?;
    let best_time = tracker.get_best_time(None)?;

    let safe_to_post = cannibalization.is_none();
    let recommendation = if safe_to_post {
        match &best_time {
            Some(bt) => format!(
                "Safe to post now. Your best historical slot is {} {:02}:00 ({:.2}% avg ER).",
                bt.day_name,
                bt.hour_of_day,
                bt.avg_engagement_rate * 100.0,
            ),
            None => "Safe to post now. Not enough data yet to suggest an optimal time (need 10+ tracked posts).".into(),
        }
    } else {
        let w = cannibalization.as_ref().unwrap();
        format!(
            "Hold off — your post from {} min ago is still gaining traction ({:.1} eng/hr). {}",
            w.posted_minutes_ago, w.current_velocity, w.suggestion,
        )
    };

    let suggestion = NextPostSuggestion {
        safe_to_post,
        cannibalization,
        best_time,
        recommendation,
    };

    output::render(format, &suggestion, None);
    Ok(())
}
