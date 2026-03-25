use chrono::{Datelike, Timelike, Utc};
use rand::Rng;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::Arc;

use crate::config::config_dir;
use crate::context::AppContext;
use crate::errors::XmasterError;
use crate::intel::preflight;
use crate::providers::xapi::XApi;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct ScheduledPost {
    pub id: String,
    pub content: String,
    pub scheduled_at: i64,
    pub timezone: String,
    pub status: String,
    pub content_type: String,
    pub created_at: i64,
    pub fired_at: Option<i64>,
    pub tweet_id: Option<String>,
    pub retry_count: i32,
    pub last_error: Option<String>,
    pub preflight_score: Option<i32>,
    pub auto_scheduled: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct FireResult {
    pub fired: u32,
    pub failed: u32,
    pub missed: u32,
    pub posts: Vec<FiredPost>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FiredPost {
    pub id: String,
    pub content_preview: String,
    pub status: String,
    pub tweet_id: Option<String>,
    pub error: Option<String>,
}

// ---------------------------------------------------------------------------
// PostScheduler
// ---------------------------------------------------------------------------

pub struct PostScheduler {
    conn: Connection,
}

impl PostScheduler {
    /// Open (or create) the scheduler database at `~/.config/xmaster/xmaster.db`.
    pub fn open() -> Result<Self, XmasterError> {
        let dir = config_dir();
        std::fs::create_dir_all(&dir).ok();
        let db_path: PathBuf = dir.join("xmaster.db");
        let conn = Connection::open(db_path).map_err(|e| XmasterError::Config(e.to_string()))?;
        let sched = Self { conn };
        sched.init_tables()?;
        Ok(sched)
    }

    fn init_tables(&self) -> Result<(), XmasterError> {
        self.conn
            .execute_batch(
                "
            CREATE TABLE IF NOT EXISTS scheduled_posts (
                id              TEXT PRIMARY KEY,
                content         TEXT NOT NULL,
                scheduled_at    INTEGER NOT NULL,
                timezone        TEXT NOT NULL DEFAULT 'UTC',
                status          TEXT NOT NULL DEFAULT 'pending',
                content_type    TEXT NOT NULL DEFAULT 'text',
                reply_to_id     TEXT,
                quote_id        TEXT,
                media_paths     TEXT,
                created_at      INTEGER NOT NULL,
                fired_at        INTEGER,
                tweet_id        TEXT,
                retry_count     INTEGER NOT NULL DEFAULT 0,
                last_error      TEXT,
                preflight_score INTEGER,
                auto_scheduled  INTEGER NOT NULL DEFAULT 0
            );
            CREATE INDEX IF NOT EXISTS idx_scheduled_status
                ON scheduled_posts(status, scheduled_at);
            ",
            )
            .map_err(|e| XmasterError::Config(e.to_string()))?;
        Ok(())
    }

    // -- writes --------------------------------------------------------------

    /// Schedule a new post.
    #[allow(clippy::too_many_arguments)]
    pub fn add(
        &self,
        content: &str,
        scheduled_at_utc: i64,
        timezone: &str,
        content_type: &str,
        reply_to: Option<&str>,
        quote: Option<&str>,
        media: Option<&[String]>,
        auto_scheduled: bool,
    ) -> Result<ScheduledPost, XmasterError> {
        let now = Utc::now().timestamp();
        let id = generate_id(now);

        // Run preflight analysis
        let pf = preflight::analyze(content, None);
        let score = pf.score as i32;

        let media_json = media.map(|m| serde_json::to_string(m).unwrap_or_default());

        self.conn
            .execute(
                "INSERT INTO scheduled_posts
                    (id, content, scheduled_at, timezone, status, content_type,
                     reply_to_id, quote_id, media_paths, created_at,
                     preflight_score, auto_scheduled)
                 VALUES (?1,?2,?3,?4,'pending',?5,?6,?7,?8,?9,?10,?11)",
                params![
                    id,
                    content,
                    scheduled_at_utc,
                    timezone,
                    content_type,
                    reply_to,
                    quote,
                    media_json,
                    now,
                    score,
                    auto_scheduled as i32,
                ],
            )
            .map_err(|e| XmasterError::Config(e.to_string()))?;

        Ok(ScheduledPost {
            id,
            content: content.to_string(),
            scheduled_at: scheduled_at_utc,
            timezone: timezone.to_string(),
            status: "pending".to_string(),
            content_type: content_type.to_string(),
            created_at: now,
            fired_at: None,
            tweet_id: None,
            retry_count: 0,
            last_error: None,
            preflight_score: Some(score),
            auto_scheduled,
        })
    }

    // -- reads ---------------------------------------------------------------

    /// List scheduled posts, optionally filtered by status.
    pub fn list(&self, status_filter: Option<&str>) -> Result<Vec<ScheduledPost>, XmasterError> {
        let (sql, use_filter) = match status_filter {
            Some(_) => (
                "SELECT id, content, scheduled_at, timezone, status, content_type,
                        created_at, fired_at, tweet_id, retry_count, last_error,
                        preflight_score, auto_scheduled
                 FROM scheduled_posts WHERE status = ?1
                 ORDER BY scheduled_at ASC",
                true,
            ),
            None => (
                "SELECT id, content, scheduled_at, timezone, status, content_type,
                        created_at, fired_at, tweet_id, retry_count, last_error,
                        preflight_score, auto_scheduled
                 FROM scheduled_posts
                 ORDER BY scheduled_at ASC",
                false,
            ),
        };

        let mut stmt = self
            .conn
            .prepare(sql)
            .map_err(|e| XmasterError::Config(e.to_string()))?;

        let map_row = |row: &rusqlite::Row| -> rusqlite::Result<ScheduledPost> {
            let auto_int: i32 = row.get(12)?;
            Ok(ScheduledPost {
                id: row.get(0)?,
                content: row.get(1)?,
                scheduled_at: row.get(2)?,
                timezone: row.get(3)?,
                status: row.get(4)?,
                content_type: row.get(5)?,
                created_at: row.get(6)?,
                fired_at: row.get(7)?,
                tweet_id: row.get(8)?,
                retry_count: row.get(9)?,
                last_error: row.get(10)?,
                preflight_score: row.get(11)?,
                auto_scheduled: auto_int != 0,
            })
        };

        let results: Vec<ScheduledPost> = if use_filter {
            stmt.query_map(params![status_filter.unwrap()], map_row)
                .map_err(|e| XmasterError::Config(e.to_string()))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| XmasterError::Config(e.to_string()))?
        } else {
            stmt.query_map([], map_row)
                .map_err(|e| XmasterError::Config(e.to_string()))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| XmasterError::Config(e.to_string()))?
        };

        Ok(results)
    }

    // -- mutations ------------------------------------------------------------

    /// Cancel a pending scheduled post.
    pub fn cancel(&self, id: &str) -> Result<(), XmasterError> {
        let changed = self
            .conn
            .execute(
                "UPDATE scheduled_posts SET status = 'cancelled'
                 WHERE id = ?1 AND status = 'pending'",
                params![id],
            )
            .map_err(|e| XmasterError::Config(e.to_string()))?;

        if changed == 0 {
            return Err(XmasterError::NotFound(format!(
                "No pending post with id '{id}' (may already be sent or cancelled)"
            )));
        }
        Ok(())
    }

    /// Reschedule a pending or failed post to a new time.
    pub fn reschedule(&self, id: &str, new_time: i64) -> Result<(), XmasterError> {
        let changed = self
            .conn
            .execute(
                "UPDATE scheduled_posts SET scheduled_at = ?1
                 WHERE id = ?2 AND status IN ('pending', 'failed')",
                params![new_time, id],
            )
            .map_err(|e| XmasterError::Config(e.to_string()))?;

        if changed == 0 {
            return Err(XmasterError::NotFound(format!(
                "No pending/failed post with id '{id}'"
            )));
        }
        Ok(())
    }

    // -- fire engine ----------------------------------------------------------

    /// Fire all due posts. The core scheduling engine.
    ///
    /// - Posts past the grace window are marked as missed/failed.
    /// - Posts within the window are posted via XApi.
    /// - On post failure, retry_count is incremented; >= 3 retries marks as failed.
    pub async fn fire(
        &self,
        ctx: Arc<AppContext>,
        grace_minutes: i64,
    ) -> Result<FireResult, XmasterError> {
        let now = Utc::now().timestamp();
        let grace_cutoff = now - (grace_minutes * 60);

        // Phase 1: Atomically claim due posts (prevents duplicate posts from concurrent runs)
        self.conn
            .execute(
                "UPDATE scheduled_posts SET status = 'firing'
                 WHERE status = 'pending' AND scheduled_at <= ?1",
                params![now],
            )
            .map_err(|e| XmasterError::Config(e.to_string()))?;

        // Phase 2: Fetch claimed posts
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, content, scheduled_at, reply_to_id, quote_id, media_paths, retry_count
                 FROM scheduled_posts
                 WHERE status = 'firing'",
            )
            .map_err(|e| XmasterError::Config(e.to_string()))?;

        type DuePost = (String, String, i64, Option<String>, Option<String>, Option<String>, i32);
        let due_posts: Vec<DuePost> =
            stmt.query_map([], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                ))
            })
            .map_err(|e| XmasterError::Config(e.to_string()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| XmasterError::Config(e.to_string()))?;

        let api = XApi::new(ctx);
        let mut result = FireResult {
            fired: 0,
            failed: 0,
            missed: 0,
            posts: Vec::new(),
        };

        for (id, content, scheduled_at, reply_to, quote, media_json, retry_count) in &due_posts {
            let preview: String = content.chars().take(80).collect();

            // Missed the grace window
            if *scheduled_at < grace_cutoff {
                self.mark_failed(id, "Missed schedule window")?;
                result.missed += 1;
                result.posts.push(FiredPost {
                    id: id.clone(),
                    content_preview: preview,
                    status: "missed".to_string(),
                    tweet_id: None,
                    error: Some("Missed schedule window".to_string()),
                });
                continue;
            }

            // Upload media files and collect media IDs (stored as file paths, not IDs)
            let media_upload = if let Some(paths_json) = media_json {
                let paths: Vec<String> = serde_json::from_str(paths_json).unwrap_or_default();
                if !paths.is_empty() {
                    let mut ids = Vec::new();
                    let mut upload_err = None;
                    for path in &paths {
                        match api.upload_media(path).await {
                            Ok(mid) => ids.push(mid),
                            Err(e) => {
                                upload_err = Some(e);
                                break;
                            }
                        }
                    }
                    if let Some(e) = upload_err {
                        Err(e)
                    } else {
                        Ok(Some(ids))
                    }
                } else {
                    Ok(None)
                }
            } else {
                Ok(None)
            };

            let media_ids = match media_upload {
                Ok(ids) => ids,
                Err(e) => {
                    self.mark_failed(id, &format!("Media upload failed: {e}"))?;
                    result.failed += 1;
                    result.posts.push(FiredPost {
                        id: id.clone(),
                        content_preview: preview,
                        status: "failed".to_string(),
                        tweet_id: None,
                        error: Some(format!("Media upload failed: {e}")),
                    });
                    continue;
                }
            };

            // Attempt to post
            let post_result = api
                .create_tweet(
                    content,
                    reply_to.as_deref(),
                    quote.as_deref(),
                    media_ids.as_deref(),
                    None,
                    None,
                )
                .await;

            match post_result {
                Ok(tweet) => {
                    let fire_ts = Utc::now().timestamp();
                    self.conn
                        .execute(
                            "UPDATE scheduled_posts
                             SET status = 'sent', fired_at = ?1, tweet_id = ?2
                             WHERE id = ?3",
                            params![fire_ts, tweet.id, id],
                        )
                        .map_err(|e| XmasterError::Config(e.to_string()))?;

                    result.fired += 1;
                    result.posts.push(FiredPost {
                        id: id.clone(),
                        content_preview: preview,
                        status: "sent".to_string(),
                        tweet_id: Some(tweet.id),
                        error: None,
                    });
                }
                Err(e) => {
                    let new_retry = retry_count + 1;
                    let err_msg = e.to_string();

                    if new_retry >= 3 {
                        self.conn
                            .execute(
                                "UPDATE scheduled_posts
                                 SET status = 'failed', retry_count = ?1, last_error = ?2
                                 WHERE id = ?3",
                                params![new_retry, err_msg, id],
                            )
                            .map_err(|e| XmasterError::Config(e.to_string()))?;
                        result.failed += 1;
                    } else {
                        self.conn
                            .execute(
                                "UPDATE scheduled_posts
                                 SET retry_count = ?1, last_error = ?2
                                 WHERE id = ?3",
                                params![new_retry, err_msg, id],
                            )
                            .map_err(|e| XmasterError::Config(e.to_string()))?;
                        result.failed += 1;
                    }

                    result.posts.push(FiredPost {
                        id: id.clone(),
                        content_preview: preview,
                        status: if new_retry >= 3 {
                            "failed".to_string()
                        } else {
                            "retry".to_string()
                        },
                        tweet_id: None,
                        error: Some(err_msg),
                    });
                }
            }
        }

        Ok(result)
    }

    /// Find the best auto-schedule time based on historical engagement data.
    /// Queries timing_stats for the best performing hour and returns the next
    /// future occurrence of that hour as a Unix timestamp.
    pub fn get_best_auto_time(&self) -> Option<i64> {
        let row: Option<(i32, i32)> = self
            .conn
            .query_row(
                "SELECT day_of_week, hour_of_day
                 FROM timing_stats
                 WHERE content_type = 'all' AND sample_count >= 2
                 ORDER BY avg_engagement_rate DESC
                 LIMIT 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .ok()
            .flatten();

        let (best_dow, best_hour) = row?;

        // Find the next occurrence of this day/hour in UTC
        let now = Utc::now();
        let current_dow = now.weekday().num_days_from_monday() as i32; // 0=Mon
        let current_hour = now.hour() as i32;

        // Days until the target day-of-week
        let mut days_ahead = best_dow - current_dow;
        if days_ahead < 0 || (days_ahead == 0 && current_hour >= best_hour) {
            days_ahead += 7;
        }

        let target = now
            .date_naive()
            .and_hms_opt(best_hour as u32, 0, 0)?;
        let target_date = target + chrono::Duration::days(days_ahead as i64);
        Some(target_date.and_utc().timestamp())
    }

    // -- internal helpers -----------------------------------------------------

    fn mark_failed(&self, id: &str, error: &str) -> Result<(), XmasterError> {
        self.conn
            .execute(
                "UPDATE scheduled_posts SET status = 'failed', last_error = ?1 WHERE id = ?2",
                params![error, id],
            )
            .map_err(|e| XmasterError::Config(e.to_string()))?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn generate_id(timestamp: i64) -> String {
    let mut rng = rand::thread_rng();
    let chars: Vec<char> = "abcdefghijklmnopqrstuvwxyz0123456789".chars().collect();
    let suffix: String = (0..4).map(|_| chars[rng.gen_range(0..chars.len())]).collect();
    format!("sched_{timestamp}_{suffix}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn in_memory_scheduler() -> PostScheduler {
        let conn = Connection::open_in_memory().unwrap();
        let sched = PostScheduler { conn };
        sched.init_tables().unwrap();
        sched
    }

    #[test]
    fn add_and_list() {
        let sched = in_memory_scheduler();
        let future_ts = Utc::now().timestamp() + 3600;

        let post = sched
            .add("Hello world!", future_ts, "UTC", "text", None, None, None, false)
            .unwrap();

        assert!(post.id.starts_with("sched_"));
        assert_eq!(post.status, "pending");
        assert!(post.preflight_score.is_some());

        let all = sched.list(None).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].content, "Hello world!");

        let pending = sched.list(Some("pending")).unwrap();
        assert_eq!(pending.len(), 1);

        let sent = sched.list(Some("sent")).unwrap();
        assert!(sent.is_empty());
    }

    #[test]
    fn cancel_pending() {
        let sched = in_memory_scheduler();
        let future_ts = Utc::now().timestamp() + 3600;

        let post = sched
            .add("Cancel me", future_ts, "UTC", "text", None, None, None, false)
            .unwrap();

        sched.cancel(&post.id).unwrap();

        let pending = sched.list(Some("pending")).unwrap();
        assert!(pending.is_empty());

        let cancelled = sched.list(Some("cancelled")).unwrap();
        assert_eq!(cancelled.len(), 1);
    }

    #[test]
    fn cancel_nonexistent_fails() {
        let sched = in_memory_scheduler();
        let result = sched.cancel("sched_fake_id");
        assert!(result.is_err());
    }

    #[test]
    fn reschedule_updates_time() {
        let sched = in_memory_scheduler();
        let t1 = Utc::now().timestamp() + 3600;
        let t2 = t1 + 7200;

        let post = sched
            .add("Reschedule me", t1, "UTC", "text", None, None, None, false)
            .unwrap();

        sched.reschedule(&post.id, t2).unwrap();

        let all = sched.list(None).unwrap();
        assert_eq!(all[0].scheduled_at, t2);
    }

    #[test]
    fn id_format() {
        let id = generate_id(1700000000);
        assert!(id.starts_with("sched_1700000000_"));
        assert_eq!(id.len(), "sched_1700000000_".len() + 4);
    }
}
