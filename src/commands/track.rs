use crate::context::AppContext;
use crate::errors::XmasterError;
use crate::intel::tracker::PostTracker;
use crate::output::{self, OutputFormat};
use std::sync::Arc;

/// Snapshot all recent posts (designed for cron). Default: last 48 hours.
pub async fn track_run(
    ctx: Arc<AppContext>,
    format: OutputFormat,
) -> Result<(), XmasterError> {
    let tracker = PostTracker::open()?;
    let summary = tracker.snapshot_all_recent(&ctx, 48).await?;
    output::render(format, &summary, None);
    Ok(())
}

/// Show which posts are being tracked and their latest snapshot age.
pub async fn track_status(
    _ctx: Arc<AppContext>,
    format: OutputFormat,
) -> Result<(), XmasterError> {
    let tracker = PostTracker::open()?;
    let status = tracker.tracking_status()?;

    if status.total == 0 {
        return Err(XmasterError::NotFound(
            "No posts are being tracked yet. Post something first with `xmaster post`, then run `xmaster track run`".into(),
        ));
    }

    output::render(format, &status, None);
    Ok(())
}
