use crate::context::AppContext;
use crate::errors::XmasterError;
use crate::intel::store::{IntelStore, REPLY_BACK_HOURS};
use crate::output::{self, OutputFormat};
use crate::providers::xapi::XApi;
use std::sync::Arc;

/// Snapshot all recent posts (designed for cron). Default: last 48 hours.
/// Also checks pending replies for reply-backs and auto-promotes hot reply
/// targets into the watchlist so they can be re-engaged.
pub async fn track_run(
    ctx: Arc<AppContext>,
    format: OutputFormat,
) -> Result<(), XmasterError> {
    let store = IntelStore::open()?;
    let summary = store.snapshot_all_recent(&ctx, 48).await?;

    // Check pending replies for reply-backs (silent, never fails the run)
    let reply_backs_checked = check_reply_backs(&ctx, &store).await;

    // Auto-promote high-performing reply targets into the watchlist
    // (silent on errors, never fails the run)
    let promoted = auto_promote_hot_reply_targets(&store);

    // Surface cross-post candidates — standalone posts that crossed 5k
    // impressions in the last 14d are worth repurposing on other platforms
    // (IG stills, LinkedIn). The clinstagram skill can take the screenshot.
    let cross_post_candidates = store.find_cross_post_candidates(5_000, 14, 5).unwrap_or_default();

    let mut meta = serde_json::json!({});
    if reply_backs_checked > 0 {
        meta["reply_backs_checked"] = reply_backs_checked.into();
    }
    if !promoted.is_empty() {
        meta["watchlist_auto_promoted_count"] = promoted.len().into();
        meta["watchlist_auto_promoted"] = promoted.into();
    }
    if !cross_post_candidates.is_empty() {
        meta["cross_post_candidates_count"] = cross_post_candidates.len().into();
        meta["cross_post_candidates"] = serde_json::to_value(&cross_post_candidates)
            .unwrap_or(serde_json::Value::Null);
        meta["cross_post_hint"] = serde_json::Value::String(
            "Standalone post(s) crossed 5k imps — consider screenshotting to IG via the clinstagram skill for a ~2x audience bounce".into()
        );
    }

    output::render(format, &summary, Some(meta));
    Ok(())
}

/// Run the store-layer hot-target selector and insert each winner into the
/// watchlist. Returns the list of usernames that were freshly promoted so the
/// caller can surface it in the track-run output metadata.
///
/// Thresholds (biased toward high-signal events at low sample volume):
///   impressions >= 100 OR profile_clicks >= 1 OR got_reply_back = 1
/// Guardrail: target_followers >= 1_000
/// Freshness: last 14 days
/// Already-watchlisted targets are excluded by the SQL join.
fn auto_promote_hot_reply_targets(store: &IntelStore) -> Vec<String> {
    let Ok(hot) = store.find_hot_reply_targets(100, 1, 1_000, 24 * 14) else {
        return Vec::new();
    };

    hot.into_iter()
        .filter_map(|row| {
            // Preserve existing topic if any; new auto-promoted rows get topic=NULL.
            store
                .add_watchlist(&row.username, row.user_id.as_deref(), None, row.target_followers)
                .ok()
                .map(|_| row.username)
        })
        .collect()
}

/// Check whether targets replied to our replies; returns how many were read.
/// A reply-back closes the row as `true`. No reply-back leaves it pending
/// until the reply is [`REPLY_BACK_HOURS`] old; only then is it closed as
/// `false`. Checks back off with age (see `get_pending_replies`), and replies
/// up to a day past the limit still get their final check.
async fn check_reply_backs(ctx: &Arc<AppContext>, store: &IntelStore) -> u32 {
    let Ok(due) = store.get_pending_replies(REPLY_BACK_HOURS + 24) else {
        return 0;
    };
    if due.is_empty() {
        return 0;
    }

    let api = XApi::new(ctx.clone());
    let now = chrono::Utc::now().timestamp();
    let mut checked = 0u32;

    for pr in &due {
        let target = pr.target_username.as_deref().unwrap_or("");
        let got_reply = match api.get_replies(&pr.reply_tweet_id, 10).await {
            Ok(replies) => replies.iter().any(|r| {
                r.author_username
                    .as_deref()
                    .is_some_and(|u| u.eq_ignore_ascii_case(target))
            }),
            // A failed read (e.g. the reply was deleted) counts as no reply-back.
            Err(_) => false,
        };
        let closed = now - pr.performed_at >= REPLY_BACK_HOURS * 3600;
        let verdict = (got_reply || closed).then_some(got_reply);
        if store.record_reply_check(pr.id, verdict).is_ok() {
            checked += 1;
        }
    }

    checked
}

/// Show which posts are being tracked and their latest snapshot age.
pub async fn track_status(
    _ctx: Arc<AppContext>,
    format: OutputFormat,
) -> Result<(), XmasterError> {
    let status = IntelStore::open()?.tracking_status()?;

    if status.total == 0 {
        return Err(XmasterError::NotFound(
            "No posts are being tracked yet. Post something first with `xmaster post`, then run `xmaster track run`".into(),
        ));
    }

    output::render(format, &status, None);
    Ok(())
}

/// Snapshot follower count and detect new/lost followers.
pub async fn track_followers(
    ctx: Arc<AppContext>,
    format: OutputFormat,
) -> Result<(), XmasterError> {
    let api = XApi::new(ctx.clone());
    let user_id = api.get_authenticated_user_id().await?;

    // Get current account stats
    let me = api.get_user_by_id(&user_id).await?;
    let metrics = me.public_metrics.as_ref();
    let followers_count = metrics.map(|m| m.followers_count as i64).unwrap_or(0);
    let following_count = metrics.map(|m| m.following_count as i64).unwrap_or(0);
    let tweet_count = metrics.map(|m| m.tweet_count as i64).unwrap_or(0);

    let tracker = IntelStore::open()?;

    // Snapshot account stats
    let snapshot = tracker.snapshot_account(followers_count, following_count, tweet_count)?;

    // Get full follower list for diffing
    let follower_data = api.get_user_followers(&user_id, 1000).await?;
    let follower_tuples: Vec<(String, String, i64)> = follower_data.iter().map(|u| {
        (
            u.id.clone(),
            u.username.clone(),
            u.public_metrics.as_ref().map(|m| m.followers_count as i64).unwrap_or(0),
        )
    }).collect();

    // Diff against previous
    let changes = tracker.diff_followers(&follower_tuples)?;

    // Store current list
    tracker.store_follower_list(&follower_tuples)?;

    // Combine output
    let output_data = FollowerTrackResult {
        account: snapshot,
        changes,
    };
    output::render(format, &output_data, None);
    Ok(())
}

/// Show follower growth history.
pub async fn follower_growth(
    _ctx: Arc<AppContext>,
    format: OutputFormat,
    days: i64,
) -> Result<(), XmasterError> {
    let history = IntelStore::open()?.follower_history(days)?;

    if history.is_empty() {
        return Err(XmasterError::NotFound(
            "No follower history yet. Run `xmaster track followers` first.".into(),
        ));
    }

    let output_data = FollowerGrowthResult { days, snapshots: history };
    output::render(format, &output_data, None);
    Ok(())
}

use serde::Serialize;
use crate::intel::tracker::{AccountSnapshot, FollowerChange};

#[derive(Serialize)]
struct FollowerTrackResult {
    account: AccountSnapshot,
    changes: FollowerChange,
}

impl crate::output::Tableable for FollowerTrackResult {
    fn to_table(&self) -> comfy_table::Table {
        let mut table = comfy_table::Table::new();
        table.set_header(vec!["Metric", "Value"]);
        table.add_row(vec!["Followers", &self.account.followers.to_string()]);
        table.add_row(vec!["Following", &self.account.following.to_string()]);
        table.add_row(vec!["Tweets", &self.account.tweets.to_string()]);
        let change_str = if self.account.followers_change >= 0 {
            format!("+{}", self.account.followers_change)
        } else {
            self.account.followers_change.to_string()
        };
        table.add_row(vec!["Followers Change", &change_str]);
        if !self.changes.new_followers.is_empty() {
            let names: Vec<String> = self.changes.new_followers.iter()
                .map(|f| format!("@{}", f.username))
                .collect();
            table.add_row(vec!["New Followers", &names.join(", ")]);
        }
        if !self.changes.lost_followers.is_empty() {
            let names: Vec<String> = self.changes.lost_followers.iter()
                .map(|f| format!("@{}", f.username))
                .collect();
            table.add_row(vec!["Lost Followers", &names.join(", ")]);
        }
        table
    }
}

#[derive(Serialize)]
struct FollowerGrowthResult {
    days: i64,
    snapshots: Vec<AccountSnapshot>,
}

impl crate::output::Tableable for FollowerGrowthResult {
    fn to_table(&self) -> comfy_table::Table {
        let mut table = comfy_table::Table::new();
        table.set_header(vec!["Date", "Followers", "Following", "Tweets"]);
        for s in &self.snapshots {
            table.add_row(vec![
                s.snapshot_at.chars().take(10).collect::<String>(),
                s.followers.to_string(),
                s.following.to_string(),
                s.tweets.to_string(),
            ]);
        }
        table
    }
}
