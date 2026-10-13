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
    let mut recommendation = match (&cannibalization, &best_time) {
        (None, Some(bt)) => format!(
            "Safe to post now. Your best historical slot is {} {:02}:00 ({:.2}% avg ER).",
            bt.day_name,
            bt.hour_of_day,
            bt.avg_engagement_rate * 100.0,
        ),
        (None, None) => "Safe to post now. Not enough data yet to suggest an optimal time (need 10+ tracked posts).".into(),
        (Some(w), _) => format!(
            "Hold off: your post from {} min ago is still gaining traction ({:.1} eng/hr). A second unseen post from you in a viewer's feed scores ×0.625, a third ×0.44 (author diversity). {}",
            w.posted_minutes_ago, w.current_velocity, w.suggestion,
        ),
    };
    // Cold start (param.rs:489-562): one bandit slot per feed request for
    // originals under 2 h / 200 home impressions by authors with ≤50k followers.
    if crate::intel::preflight::own_follower_count().is_none_or(|f| f <= 50_000) {
        recommendation.push_str(
            " Cold start: for its first 2 h (under 200 home impressions) an original can win one exploration slot, picked by likes per impression, so post when your followers are online and keep originals 2 h apart.",
        );
    }

    let suggestion = NextPostSuggestion {
        safe_to_post,
        cannibalization,
        best_time,
        recommendation,
    };

    output::render(format, &suggestion, None);
    Ok(())
}
