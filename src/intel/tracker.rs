use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use std::path::PathBuf;

use crate::config::config_dir;
use crate::errors::XmasterError;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct TimingSlot {
    pub day_of_week: u32,
    pub hour_of_day: u32,
    pub day_name: String,
    pub avg_impressions: f64,
    pub avg_engagement_rate: f64,
    pub sample_count: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct CannibalizationWarning {
    #[serde(rename = "id")]
    pub tweet_id: String,
    pub text_preview: String,
    pub posted_minutes_ago: u32,
    pub current_velocity: f64,
    pub suggestion: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct PerformanceReport {
    pub period: String,
    pub total_posts: u32,
    pub total_impressions: u64,
    pub avg_engagement_rate: f64,
    pub best_post: Option<PostSummary>,
    pub worst_post: Option<PostSummary>,
    pub best_time: Option<TimingSlot>,
    pub content_breakdown: Vec<ContentTypeStats>,
    pub trend: String,
    pub suggested_next_commands: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PostSummary {
    #[serde(rename = "id")]
    pub tweet_id: String,
    pub text_preview: String,
    pub engagement_rate: f64,
    pub impressions: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ContentTypeStats {
    pub content_type: String,
    pub count: u32,
    pub avg_engagement_rate: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct SnapshotSummary {
    pub tweets_snapshotted: u32,
    pub errors: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct TrackedPost {
    #[serde(rename = "id")]
    pub tweet_id: String,
    pub text_preview: String,
    #[serde(rename = "date")]
    pub posted_at: String,
    pub snapshots: u32,
    pub last_snapshot_age_mins: Option<i64>,
    pub latest_impressions: Option<i64>,
    pub latest_engagement_rate: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TrackStatus {
    pub tracked_posts: Vec<TrackedPost>,
    pub total: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct NextPostSuggestion {
    pub safe_to_post: bool,
    pub cannibalization: Option<CannibalizationWarning>,
    pub best_time: Option<TimingSlot>,
    pub recommendation: String,
}

// ---------------------------------------------------------------------------
// Metric snapshot data fetched from X API (used internally)
// ---------------------------------------------------------------------------

/// Full tweet metrics with a canonical engagement_rate() method.
/// Kept public because downstream callers may build one of these up from
/// arbitrary sources to compare / combine metrics consistently.
#[derive(Debug, Clone, Default, Serialize)]
pub struct TweetMetricsFull {
    pub likes: i64,
    pub retweets: i64,
    pub replies: i64,
    pub quotes: i64,
    pub impressions: i64,
    pub bookmarks: i64,
    pub profile_clicks: Option<i64>,
    pub url_clicks: Option<i64>,
}

impl TweetMetricsFull {
    /// Canonical engagement rate: (likes + retweets + replies + quotes) / impressions.
    /// Bookmarks excluded — not a 2026 algorithm signal.
    pub fn engagement_rate(&self) -> f64 {
        if self.impressions <= 0 { return 0.0; }
        (self.likes + self.retweets + self.replies + self.quotes) as f64 / self.impressions as f64
    }
}

// ---------------------------------------------------------------------------
// PostTracker — uses its own Connection to avoid needing store.rs edits
// ---------------------------------------------------------------------------

pub struct PostTracker {
    conn: Connection,
}

impl PostTracker {
    pub fn open() -> Result<Self, XmasterError> {
        let dir = config_dir();
        std::fs::create_dir_all(&dir).ok();
        let db_path: PathBuf = dir.join("xmaster.db");
        let conn = Connection::open(db_path)
            .map_err(|e| XmasterError::Config(format!("DB open error: {e}")))?;
        conn.pragma_update(None, "journal_mode", "wal")
            .map_err(|e| XmasterError::Config(format!("DB pragma error: {e}")))?;
        conn.pragma_update(None, "busy_timeout", 5000)
            .map_err(|e| XmasterError::Config(format!("DB pragma error: {e}")))?;
        conn.pragma_update(None, "synchronous", "NORMAL")
            .map_err(|e| XmasterError::Config(format!("DB pragma error: {e}")))?;

        // Ensure required tables exist (safe on fresh install)
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS posts (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                tweet_id TEXT UNIQUE NOT NULL,
                text TEXT NOT NULL,
                content_type TEXT NOT NULL DEFAULT 'text',
                char_count INTEGER NOT NULL DEFAULT 0,
                has_link INTEGER NOT NULL DEFAULT 0,
                has_media INTEGER NOT NULL DEFAULT 0,
                has_poll INTEGER NOT NULL DEFAULT 0,
                hashtag_count INTEGER NOT NULL DEFAULT 0,
                hook_text TEXT,
                posted_at INTEGER NOT NULL,
                day_of_week INTEGER NOT NULL DEFAULT 0,
                hour_of_day INTEGER NOT NULL DEFAULT 0,
                reply_to_id TEXT,
                quote_of_id TEXT,
                preflight_score REAL
            );
            CREATE TABLE IF NOT EXISTS metric_snapshots (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                tweet_id TEXT NOT NULL,
                snapshot_at INTEGER NOT NULL,
                minutes_since_post INTEGER NOT NULL DEFAULT 0,
                likes INTEGER NOT NULL DEFAULT 0,
                retweets INTEGER NOT NULL DEFAULT 0,
                replies INTEGER NOT NULL DEFAULT 0,
                impressions INTEGER NOT NULL DEFAULT 0,
                bookmarks INTEGER NOT NULL DEFAULT 0,
                quotes INTEGER NOT NULL DEFAULT 0,
                profile_clicks INTEGER NOT NULL DEFAULT 0,
                url_clicks INTEGER
            );
            CREATE TABLE IF NOT EXISTS account_snapshots (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                snapshot_at INTEGER NOT NULL,
                followers INTEGER NOT NULL DEFAULT 0,
                following INTEGER NOT NULL DEFAULT 0,
                tweets INTEGER NOT NULL DEFAULT 0
            );
            CREATE TABLE IF NOT EXISTS follower_list (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                snapshot_at INTEGER NOT NULL,
                user_id TEXT NOT NULL,
                username TEXT NOT NULL,
                followers INTEGER NOT NULL DEFAULT 0
            );
            CREATE TABLE IF NOT EXISTS timing_stats (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                day_of_week INTEGER NOT NULL,
                hour_of_day INTEGER NOT NULL,
                content_type TEXT NOT NULL DEFAULT 'all',
                avg_impressions REAL,
                avg_engagement_rate REAL,
                sample_count INTEGER NOT NULL DEFAULT 0,
                last_updated INTEGER NOT NULL
            );",
        )
        .map_err(|e| XmasterError::Config(format!("DB init error: {e}")))?;

        // Safe migrations: add local-time columns if not present
        let post_cols: Vec<String> = conn
            .prepare("PRAGMA table_info(posts)")
            .and_then(|mut s| s.query_map([], |row| row.get::<_, String>(1))?.collect::<Result<Vec<_>, _>>())
            .unwrap_or_default();
        if !post_cols.iter().any(|c| c == "local_day_of_week") {
            conn.execute_batch("ALTER TABLE posts ADD COLUMN local_day_of_week INTEGER;").ok();
        }
        if !post_cols.iter().any(|c| c == "local_hour_of_day") {
            conn.execute_batch("ALTER TABLE posts ADD COLUMN local_hour_of_day INTEGER;").ok();
        }
        if !post_cols.iter().any(|c| c == "tz_offset_minutes") {
            conn.execute_batch("ALTER TABLE posts ADD COLUMN tz_offset_minutes INTEGER;").ok();
        }
        // Add url_clicks to metric_snapshots if not present
        let ms_cols: Vec<String> = conn
            .prepare("PRAGMA table_info(metric_snapshots)")
            .and_then(|mut s| s.query_map([], |row| row.get::<_, String>(1))?.collect::<Result<Vec<_>, _>>())
            .unwrap_or_default();
        if !ms_cols.iter().any(|c| c == "url_clicks") {
            conn.execute_batch("ALTER TABLE metric_snapshots ADD COLUMN url_clicks INTEGER;").ok();
        }

        Ok(Self { conn })
    }

    // -- snapshot all recent posts --------------------------------------------

    /// Fetch current metrics for every post within the last `hours` window in
    /// a single batched HTTP pass via `XApi::get_posts_by_ids`, then insert a
    /// snapshot row for each matched tweet.
    ///
    /// Previously this looped `snapshot_tweet` per post = O(n) HTTP calls. The
    /// batch path uses one call per 100 tweets. Results are joined back to
    /// posts by tweet_id (HashMap lookup) — never by index — because X may
    /// omit deleted tweets or reorder the response.
    pub async fn snapshot_all_recent(
        &self,
        ctx: &std::sync::Arc<crate::context::AppContext>,
        hours: u32,
    ) -> Result<SnapshotSummary, XmasterError> {
        let now = Utc::now();
        let now_ts = now.timestamp();
        let cutoff = now_ts - (hours as i64 * 3600);

        let mut stmt = self
            .conn
            .prepare(
                "SELECT tweet_id, posted_at FROM posts
                 WHERE posted_at > ?1
                 ORDER BY posted_at DESC",
            )?;

        let rows: Vec<(String, i64)> = stmt
            .query_map(params![cutoff], |row| {
                let tweet_id: String = row.get(0)?;
                // Handle both INTEGER (current) and TEXT (old DB) for posted_at
                let posted_at = row.get::<_, i64>(1).unwrap_or_else(|_| {
                    row.get::<_, String>(1)
                        .ok()
                        .and_then(|s| s.parse().ok())
                        .unwrap_or(0)
                });
                Ok((tweet_id, posted_at))
            })?
            .collect::<Result<Vec<_>, _>>()?;

        if rows.is_empty() {
            self.update_timing_stats()?;
            return Ok(SnapshotSummary {
                tweets_snapshotted: 0,
                errors: 0,
            });
        }

        let tweet_ids: Vec<String> = rows.iter().map(|(id, _)| id.clone()).collect();

        // Single batched fetch via the shared XApi helper. XApi chunks into
        // groups of 100 internally and handles the 403 public-only fallback.
        let api = crate::providers::xapi::XApi::new(ctx.clone());
        let lookups = match api.get_posts_by_ids(&tweet_ids).await {
            Ok(l) => l,
            Err(e) => {
                tracing::warn!(error = %e, "batch metrics fetch failed in snapshot_all_recent");
                return Ok(SnapshotSummary {
                    tweets_snapshotted: 0,
                    errors: rows.len() as u32,
                });
            }
        };

        // Index by tweet_id. NEVER zip by position — X may omit deleted tweets
        // or return them in an arbitrary order, and a positional mismatch would
        // write the wrong metrics to the wrong post.
        let by_id: std::collections::HashMap<String, crate::providers::xapi::TweetLookup> =
            lookups.into_iter().map(|t| (t.id.clone(), t)).collect();

        let mut snapshotted = 0u32;
        let mut errors = 0u32;
        let snapshot_at = now_ts;

        for (tweet_id, posted_at) in &rows {
            let Some(lookup) = by_id.get(tweet_id) else {
                tracing::warn!(
                    tweet_id = %tweet_id,
                    "tweet not returned by /2/tweets batch — likely deleted or hidden"
                );
                errors += 1;
                continue;
            };

            let public = lookup.public_metrics.clone().unwrap_or_default();
            // Store Some(0) when non_public_metrics was present (real zero),
            // None only when absent (403 fallback). Distinguishing "no data"
            // from "0 clicks" is the whole point of making this Optional.
            let url_clicks = lookup
                .non_public_metrics
                .as_ref()
                .map(|np| np.url_link_clicks as i64);
            let non_public = lookup.non_public_metrics.clone().unwrap_or_default();
            let posted = DateTime::from_timestamp(*posted_at, 0).unwrap_or(now);
            let minutes = (now - posted).num_minutes();

            match self.conn.execute(
                "INSERT INTO metric_snapshots
                    (tweet_id, snapshot_at, minutes_since_post, likes, retweets, replies,
                     impressions, bookmarks, quotes, profile_clicks, url_clicks)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
                params![
                    tweet_id,
                    snapshot_at,
                    minutes,
                    public.like_count as i64,
                    public.retweet_count as i64,
                    public.reply_count as i64,
                    public.impression_count as i64,
                    public.bookmark_count as i64,
                    public.quote_count as i64,
                    non_public.user_profile_clicks as i64,
                    url_clicks,
                ],
            ) {
                Ok(_) => snapshotted += 1,
                Err(e) => {
                    tracing::warn!(tweet_id = %tweet_id, error = %e, "insert failed");
                    errors += 1;
                }
            }
        }

        // Refresh timing_stats after batch snapshot
        self.update_timing_stats()?;

        Ok(SnapshotSummary {
            tweets_snapshotted: snapshotted,
            errors,
        })
    }

    // -- timing heatmap -------------------------------------------------------

    pub fn compute_timing_heatmap(&self) -> Result<Vec<TimingSlot>, XmasterError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT COALESCE(p.local_day_of_week, p.day_of_week), COALESCE(p.local_hour_of_day, p.hour_of_day),
                        AVG(ms.impressions) AS avg_imp,
                        AVG(CASE WHEN ms.impressions > 0
                             THEN (ms.likes + ms.retweets + ms.replies + ms.quotes) * 1.0
                                  / ms.impressions ELSE 0 END) AS avg_er,
                        COUNT(DISTINCT p.tweet_id) AS cnt
                 FROM posts p
                 JOIN metric_snapshots ms ON ms.tweet_id = p.tweet_id
                 GROUP BY COALESCE(p.local_day_of_week, p.day_of_week), COALESCE(p.local_hour_of_day, p.hour_of_day)
                 ORDER BY avg_er DESC",
            )?;

        let rows = stmt
            .query_map([], |row| {
                let dow: i32 = row.get(0)?;
                let hod: i32 = row.get(1)?;
                Ok(TimingSlot {
                    day_of_week: dow as u32,
                    hour_of_day: hod as u32,
                    day_name: day_name(dow as u32),
                    avg_impressions: row.get(2)?,
                    avg_engagement_rate: row.get(3)?,
                    sample_count: row.get::<_, i64>(4)? as u32,
                })
            })?;

        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    // -- best posting time ----------------------------------------------------

    pub fn get_best_time(
        &self,
        content_type: Option<&str>,
    ) -> Result<Option<TimingSlot>, XmasterError> {
        let total: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM posts", [], |r| r.get(0))?;

        if total < 10 {
            return Ok(None);
        }

        let slot = match content_type {
            Some(ct) => self
                .conn
                .query_row(
                    "SELECT COALESCE(p.local_day_of_week, p.day_of_week), COALESCE(p.local_hour_of_day, p.hour_of_day),
                            AVG(ms.impressions),
                            AVG(CASE WHEN ms.impressions > 0
                                 THEN (ms.likes+ms.retweets+ms.replies+ms.quotes)*1.0
                                      / ms.impressions ELSE 0 END),
                            COUNT(DISTINCT p.tweet_id)
                     FROM posts p
                     JOIN metric_snapshots ms ON ms.tweet_id = p.tweet_id
                     WHERE p.content_type = ?1
                     GROUP BY COALESCE(p.local_day_of_week, p.day_of_week), COALESCE(p.local_hour_of_day, p.hour_of_day)
                     HAVING COUNT(DISTINCT p.tweet_id) >= 2
                     ORDER BY 4 DESC LIMIT 1",
                    params![ct],
                    |row| {
                        let dow: i32 = row.get(0)?;
                        let hod: i32 = row.get(1)?;
                        Ok(TimingSlot {
                            day_of_week: dow as u32,
                            hour_of_day: hod as u32,
                            day_name: day_name(dow as u32),
                            avg_impressions: row.get(2)?,
                            avg_engagement_rate: row.get(3)?,
                            sample_count: row.get::<_, i64>(4)? as u32,
                        })
                    },
                )
                .optional()?,
            None => self
                .conn
                .query_row(
                    "SELECT COALESCE(p.local_day_of_week, p.day_of_week), COALESCE(p.local_hour_of_day, p.hour_of_day),
                            AVG(ms.impressions),
                            AVG(CASE WHEN ms.impressions > 0
                                 THEN (ms.likes+ms.retweets+ms.replies+ms.quotes)*1.0
                                      / ms.impressions ELSE 0 END),
                            COUNT(DISTINCT p.tweet_id)
                     FROM posts p
                     JOIN metric_snapshots ms ON ms.tweet_id = p.tweet_id
                     GROUP BY COALESCE(p.local_day_of_week, p.day_of_week), COALESCE(p.local_hour_of_day, p.hour_of_day)
                     HAVING COUNT(DISTINCT p.tweet_id) >= 2
                     ORDER BY 4 DESC LIMIT 1",
                    [],
                    |row| {
                        let dow: i32 = row.get(0)?;
                        let hod: i32 = row.get(1)?;
                        Ok(TimingSlot {
                            day_of_week: dow as u32,
                            hour_of_day: hod as u32,
                            day_name: day_name(dow as u32),
                            avg_impressions: row.get(2)?,
                            avg_engagement_rate: row.get(3)?,
                            sample_count: row.get::<_, i64>(4)? as u32,
                        })
                    },
                )
                .optional()?,
        };

        Ok(slot)
    }

    // -- cannibalization check ------------------------------------------------

    pub fn check_cannibalization(&self) -> Result<Option<CannibalizationWarning>, XmasterError> {
        let now = Utc::now();
        let now_ts = now.timestamp();
        let cutoff = now_ts - 21600; // 6 hours

        // Find a post from the last 6 hours whose latest two snapshots show accelerating impressions
        let result: Option<(String, String, i64)> = self
            .conn
            .query_row(
                "SELECT s1.tweet_id, p.text, p.posted_at
                 FROM metric_snapshots s1
                 JOIN metric_snapshots s2 ON s1.tweet_id = s2.tweet_id
                   AND s2.id = (SELECT MAX(id) FROM metric_snapshots
                                WHERE tweet_id = s1.tweet_id AND id < s1.id)
                 JOIN posts p ON p.tweet_id = s1.tweet_id
                 WHERE s1.id = (SELECT MAX(id) FROM metric_snapshots
                                WHERE tweet_id = s1.tweet_id)
                   AND p.posted_at > ?1
                   AND s1.impressions > s2.impressions * 1.5
                 ORDER BY (s1.impressions - s2.impressions) DESC
                 LIMIT 1",
                params![cutoff],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;

        let (tweet_id, text, posted_at) = match result {
            Some(r) => r,
            None => return Ok(None),
        };

        let posted = DateTime::from_timestamp(posted_at, 0).unwrap_or(now);
        let minutes_ago = (now - posted).num_minutes().max(0) as u32;

        // Compute current velocity from the latest snapshot
        let current_velocity: f64 = self
            .conn
            .query_row(
                "SELECT (likes + retweets + replies + quotes) * 1.0
                 FROM metric_snapshots
                 WHERE tweet_id = ?1
                 ORDER BY id DESC LIMIT 1",
                params![tweet_id],
                |row| row.get::<_, f64>(0),
            )
            .unwrap_or(0.0)
            / (minutes_ago as f64 / 60.0).max(0.1);

        let text_preview: String = text.chars().take(80).collect();
        let wait_hours = if minutes_ago < 120 {
            (120 - minutes_ago) / 60 + 1
        } else {
            1
        };

        Ok(Some(CannibalizationWarning {
            tweet_id,
            text_preview,
            posted_minutes_ago: minutes_ago,
            current_velocity,
            suggestion: format!(
                "Wait ~{wait_hours} hour(s) for your current post to settle before posting again"
            ),
        }))
    }

    // -- performance report ---------------------------------------------------

    pub fn generate_report(&self, period: &str) -> Result<PerformanceReport, XmasterError> {
        let hours: i64 = match period {
            "daily" => 24,
            "weekly" => 168,
            "monthly" => 720,
            _ => 168,
        };

        let now_ts = Utc::now().timestamp();
        let cutoff = now_ts - (hours * 3600);

        // Current period posts with latest metrics
        let mut stmt = self
            .conn
            .prepare(
                "SELECT p.tweet_id, p.text, p.content_type,
                        COALESCE(ms.impressions, 0),
                        CASE WHEN COALESCE(ms.impressions, 0) > 0
                             THEN (COALESCE(ms.likes,0) + COALESCE(ms.retweets,0)
                                   + COALESCE(ms.replies,0) + COALESCE(ms.quotes,0)) * 1.0
                                  / ms.impressions
                             ELSE 0 END AS er
                 FROM posts p
                 LEFT JOIN metric_snapshots ms ON ms.tweet_id = p.tweet_id
                   AND ms.id = (SELECT MAX(id) FROM metric_snapshots WHERE tweet_id = p.tweet_id)
                 WHERE p.posted_at > ?1
                 ORDER BY er DESC",
            )?;

        struct PostRow {
            tweet_id: String,
            text: String,
            content_type: String,
            impressions: i64,
            engagement_rate: f64,
        }

        let posts: Vec<PostRow> = stmt
            .query_map(params![cutoff], |row| {
                Ok(PostRow {
                    tweet_id: row.get(0)?,
                    text: row.get(1)?,
                    content_type: row.get(2)?,
                    impressions: row.get(3)?,
                    engagement_rate: row.get(4)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        let total_posts = posts.len() as u32;
        let total_impressions: u64 = posts.iter().map(|p| p.impressions as u64).sum();
        let avg_engagement_rate = if total_posts > 0 {
            posts.iter().map(|p| p.engagement_rate).sum::<f64>() / total_posts as f64
        } else {
            0.0
        };

        let best_post = posts.first().map(|p| PostSummary {
            tweet_id: p.tweet_id.clone(),
            text_preview: p.text.chars().take(80).collect(),
            engagement_rate: p.engagement_rate,
            impressions: p.impressions as u64,
        });

        let worst_post = if posts.len() > 1 {
            posts.last().map(|p| PostSummary {
                tweet_id: p.tweet_id.clone(),
                text_preview: p.text.chars().take(80).collect(),
                engagement_rate: p.engagement_rate,
                impressions: p.impressions as u64,
            })
        } else {
            None
        };

        // Content breakdown
        let mut content_map: std::collections::HashMap<String, (u32, f64)> =
            std::collections::HashMap::new();
        for p in &posts {
            let entry = content_map.entry(p.content_type.clone()).or_insert((0, 0.0));
            entry.0 += 1;
            entry.1 += p.engagement_rate;
        }
        let content_breakdown: Vec<ContentTypeStats> = content_map
            .into_iter()
            .map(|(ct, (count, er_sum))| ContentTypeStats {
                content_type: ct,
                count,
                avg_engagement_rate: if count > 0 {
                    er_sum / count as f64
                } else {
                    0.0
                },
            })
            .collect();

        let best_time = self.get_best_time(None)?;

        // Trend: compare to previous period
        let prev_cutoff = now_ts - (hours * 2 * 3600);
        let prev_avg_er: f64 = self
            .conn
            .query_row(
                "SELECT AVG(
                    CASE WHEN COALESCE(ms.impressions, 0) > 0
                         THEN (COALESCE(ms.likes,0)+COALESCE(ms.retweets,0)
                               +COALESCE(ms.replies,0)+COALESCE(ms.quotes,0))*1.0
                              / ms.impressions
                         ELSE 0 END
                 )
                 FROM posts p
                 LEFT JOIN metric_snapshots ms ON ms.tweet_id = p.tweet_id
                   AND ms.id = (SELECT MAX(id) FROM metric_snapshots WHERE tweet_id = p.tweet_id)
                 WHERE p.posted_at > ?1
                   AND p.posted_at <= ?2",
                params![prev_cutoff, cutoff],
                |row| row.get::<_, Option<f64>>(0),
            )?
            .unwrap_or(0.0);

        let trend = if prev_avg_er == 0.0 || total_posts == 0 {
            "insufficient_data".to_string()
        } else if avg_engagement_rate > prev_avg_er * 1.1 {
            "improving".to_string()
        } else if avg_engagement_rate < prev_avg_er * 0.9 {
            "declining".to_string()
        } else {
            "stable".to_string()
        };

        Ok(PerformanceReport {
            period: period.to_string(),
            total_posts,
            total_impressions,
            avg_engagement_rate,
            best_post,
            worst_post,
            best_time,
            content_breakdown,
            trend,
            suggested_next_commands: vec![
                "xmaster suggest best-time".into(),
                "xmaster suggest next-post".into(),
                "xmaster track run".into(),
            ],
        })
    }

    // -- tracking status ------------------------------------------------------

    pub fn tracking_status(&self) -> Result<TrackStatus, XmasterError> {
        let now_ts = Utc::now().timestamp();

        // CAST snapshot_at to INTEGER: older DBs (created by tracker.rs's
        // original CREATE TABLE) may store snapshot_at as TEXT despite the
        // current schema declaring INTEGER. See note in store.rs:405-409.
        let mut stmt = self
            .conn
            .prepare(
                "SELECT p.tweet_id,
                        SUBSTR(p.text, 1, 60) AS preview,
                        p.posted_at,
                        (SELECT COUNT(*) FROM metric_snapshots ms WHERE ms.tweet_id = p.tweet_id) AS snap_count,
                        (SELECT CAST(MAX(ms.snapshot_at) AS INTEGER) FROM metric_snapshots ms WHERE ms.tweet_id = p.tweet_id) AS last_snap,
                        (SELECT ms.impressions FROM metric_snapshots ms WHERE ms.tweet_id = p.tweet_id ORDER BY ms.id DESC LIMIT 1),
                        (SELECT CASE WHEN ms.impressions > 0
                                     THEN (ms.likes+ms.retweets+ms.replies+ms.quotes)*1.0 / ms.impressions
                                     ELSE 0 END
                         FROM metric_snapshots ms WHERE ms.tweet_id = p.tweet_id ORDER BY ms.id DESC LIMIT 1)
                 FROM posts p
                 ORDER BY p.posted_at DESC
                 LIMIT 50",
            )?;

        let posts: Vec<TrackedPost> = stmt
            .query_map([], |row| {
                let last_snap: Option<i64> = row.get(4)?;
                let age_mins = last_snap.map(|ts| (now_ts - ts) / 60);
                // Handle both INTEGER (current schema) and TEXT (old DBs) for posted_at
                let posted_str = match row.get::<_, i64>(2) {
                    Ok(ts) => DateTime::from_timestamp(ts, 0)
                        .map(|dt| dt.to_rfc3339())
                        .unwrap_or_else(|| ts.to_string()),
                    Err(_) => row.get::<_, String>(2).unwrap_or_default(),
                };
                Ok(TrackedPost {
                    tweet_id: row.get(0)?,
                    text_preview: row.get(1)?,
                    posted_at: posted_str,
                    snapshots: row.get::<_, i64>(3)? as u32,
                    last_snapshot_age_mins: age_mins,
                    latest_impressions: row.get(5)?,
                    latest_engagement_rate: row.get(6)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        let total = posts.len() as u32;
        Ok(TrackStatus {
            tracked_posts: posts,
            total,
        })
    }

    // -- internal: refresh timing_stats table ---------------------------------

    fn update_timing_stats(&self) -> Result<(), XmasterError> {
        let now = Utc::now().timestamp();
        self.conn
            .execute_batch("DELETE FROM timing_stats")?;
        self.conn
            .execute(
                "INSERT INTO timing_stats
                    (day_of_week, hour_of_day, content_type,
                     avg_impressions, avg_engagement_rate, sample_count, last_updated)
                 SELECT COALESCE(p.local_day_of_week, p.day_of_week), COALESCE(p.local_hour_of_day, p.hour_of_day), 'all',
                        AVG(ms.impressions),
                        AVG(CASE WHEN ms.impressions > 0
                             THEN (ms.likes+ms.retweets+ms.replies+ms.quotes)*1.0
                                  / ms.impressions ELSE 0 END),
                        COUNT(DISTINCT p.tweet_id),
                        ?1
                 FROM posts p
                 JOIN metric_snapshots ms ON ms.tweet_id = p.tweet_id
                 GROUP BY COALESCE(p.local_day_of_week, p.day_of_week), COALESCE(p.local_hour_of_day, p.hour_of_day)",
                params![now],
            )?;
        Ok(())
    }
}

// Batched metrics fetch now lives in providers::xapi::get_posts_by_ids.
// snapshot_all_recent above calls that helper once per 100-tweet chunk.

// ---------------------------------------------------------------------------
// Follower tracking
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct AccountSnapshot {
    pub followers: i64,
    pub following: i64,
    pub tweets: i64,
    pub followers_change: i64,
    pub following_change: i64,
    pub snapshot_at: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct FollowerChange {
    pub new_followers: Vec<FollowerInfo>,
    pub lost_followers: Vec<FollowerInfo>,
    pub current_total: i64,
    pub previous_total: i64,
    pub net_change: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct FollowerInfo {
    pub username: String,
    pub followers: i64,
}

impl PostTracker {
    /// Snapshot the current account stats (followers, following, tweets).
    pub fn snapshot_account(&self, followers: i64, following: i64, tweets: i64) -> Result<AccountSnapshot, XmasterError> {
        let now = Utc::now();
        let now_ts = now.timestamp();

        self.conn.execute(
            "INSERT INTO account_snapshots (snapshot_at, followers, following, tweets) VALUES (?1, ?2, ?3, ?4)",
            params![now_ts, followers, following, tweets],
        )?;

        // Get previous snapshot for change calculation
        let prev: Option<(i64, i64)> = self.conn.query_row(
            "SELECT followers, following FROM account_snapshots WHERE snapshot_at < ?1 ORDER BY snapshot_at DESC LIMIT 1",
            params![now_ts],
            |row| Ok((row.get(0)?, row.get(1)?)),
        ).optional()?;

        let (prev_followers, prev_following) = prev.unwrap_or((followers, following));

        Ok(AccountSnapshot {
            followers,
            following,
            tweets,
            followers_change: followers - prev_followers,
            following_change: following - prev_following,
            snapshot_at: now.to_rfc3339(),
        })
    }

    /// Store the current follower list for diffing.
    pub fn store_follower_list(&self, followers: &[(String, String, i64)]) -> Result<(), XmasterError> {
        let now_ts = Utc::now().timestamp();
        let tx = self.conn.unchecked_transaction()?;
        for (user_id, username, follower_count) in followers {
            tx.execute(
                "INSERT INTO follower_list (snapshot_at, user_id, username, followers) VALUES (?1, ?2, ?3, ?4)",
                params![now_ts, user_id, username, follower_count],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Diff the current follower list against the previous snapshot.
    pub fn diff_followers(&self, current: &[(String, String, i64)]) -> Result<FollowerChange, XmasterError> {
        // Get the most recent snapshot timestamp
        let latest_ts: Option<i64> = self.conn.query_row(
            "SELECT MAX(snapshot_at) FROM follower_list",
            [],
            |row| row.get(0),
        ).optional()?.flatten();

        let current_set: std::collections::HashMap<&str, (&str, i64)> = current.iter()
            .map(|(uid, uname, fc)| (uid.as_str(), (uname.as_str(), *fc)))
            .collect();

        let mut new_followers = Vec::new();
        let mut lost_followers = Vec::new();

        if let Some(ts) = latest_ts {
            // Get previous follower user_ids
            let mut stmt = self.conn.prepare(
                "SELECT user_id, username, followers FROM follower_list WHERE snapshot_at = ?1"
            )?;

            let prev_rows: Vec<(String, String, i64)> = stmt.query_map(params![ts], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;

            let prev_set: std::collections::HashSet<String> = prev_rows.iter().map(|(uid, _, _)| uid.clone()).collect();
            let curr_ids: std::collections::HashSet<&str> = current_set.keys().copied().collect();

            // New: in current but not in previous
            for (uid, (uname, fc)) in &current_set {
                if !prev_set.contains(*uid) {
                    new_followers.push(FollowerInfo { username: uname.to_string(), followers: *fc });
                }
            }

            // Lost: in previous but not in current
            for (uid, uname, fc) in &prev_rows {
                if !curr_ids.contains(uid.as_str()) {
                    lost_followers.push(FollowerInfo { username: uname.clone(), followers: *fc });
                }
            }
        }

        let current_total = current.len() as i64;
        let previous_total = current_total - new_followers.len() as i64 + lost_followers.len() as i64;

        Ok(FollowerChange {
            new_followers,
            lost_followers,
            current_total,
            previous_total,
            net_change: current_total - previous_total,
        })
    }

    /// Get follower growth history.
    pub fn follower_history(&self, days: i64) -> Result<Vec<AccountSnapshot>, XmasterError> {
        let cutoff = Utc::now().timestamp() - (days * 86400);
        let mut stmt = self.conn.prepare(
            "SELECT snapshot_at, followers, following, tweets FROM account_snapshots
             WHERE snapshot_at > ?1 ORDER BY snapshot_at ASC"
        )?;

        let rows = stmt.query_map(params![cutoff], |row| {
            let ts: i64 = row.get(0)?;
            Ok(AccountSnapshot {
                followers: row.get(1)?,
                following: row.get(2)?,
                tweets: row.get(3)?,
                followers_change: 0,
                following_change: 0,
                snapshot_at: DateTime::from_timestamp(ts, 0)
                    .map(|d| d.to_rfc3339())
                    .unwrap_or_default(),
            })
        })?;

        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn day_name(day: u32) -> String {
    match day {
        0 => "Monday",
        1 => "Tuesday",
        2 => "Wednesday",
        3 => "Thursday",
        4 => "Friday",
        5 => "Saturday",
        6 => "Sunday",
        _ => "Unknown",
    }
    .to_string()
}

// ---------------------------------------------------------------------------
// Tableable impls
// ---------------------------------------------------------------------------

use crate::output::Tableable;

impl Tableable for SnapshotSummary {
    fn to_table(&self) -> comfy_table::Table {
        let mut table = comfy_table::Table::new();
        table.set_header(vec!["Metric", "Value"]);
        table.add_row(vec![
            "Tweets Snapshotted",
            &self.tweets_snapshotted.to_string(),
        ]);
        table.add_row(vec!["Errors", &self.errors.to_string()]);
        table
    }
}

impl Tableable for TrackStatus {
    fn to_table(&self) -> comfy_table::Table {
        let mut table = comfy_table::Table::new();
        table.set_header(vec![
            "Tweet ID",
            "Preview",
            "Posted",
            "Snapshots",
            "Last Snap (min)",
            "Impressions",
            "Eng. Rate",
        ]);
        for p in &self.tracked_posts {
            table.add_row(vec![
                &p.tweet_id,
                &p.text_preview,
                &p.posted_at,
                &p.snapshots.to_string(),
                &p.last_snapshot_age_mins
                    .map(|m| m.to_string())
                    .unwrap_or_else(|| "—".into()),
                &p.latest_impressions
                    .map(|i| i.to_string())
                    .unwrap_or_else(|| "—".into()),
                &p.latest_engagement_rate
                    .map(|r| format!("{:.2}%", r * 100.0))
                    .unwrap_or_else(|| "—".into()),
            ]);
        }
        table
    }
}

impl Tableable for Vec<TimingSlot> {
    fn to_table(&self) -> comfy_table::Table {
        let mut table = comfy_table::Table::new();
        table.set_header(vec![
            "Day",
            "Hour",
            "Avg Impressions",
            "Avg Eng. Rate",
            "Samples",
        ]);
        for slot in self {
            table.add_row(vec![
                &slot.day_name,
                &format!("{:02}:00", slot.hour_of_day),
                &format!("{:.0}", slot.avg_impressions),
                &format!("{:.2}%", slot.avg_engagement_rate * 100.0),
                &slot.sample_count.to_string(),
            ]);
        }
        table
    }
}

impl Tableable for CannibalizationWarning {
    fn to_table(&self) -> comfy_table::Table {
        let mut table = comfy_table::Table::new();
        table.set_header(vec!["Field", "Value"]);
        table.add_row(vec!["Active Tweet", &self.tweet_id]);
        table.add_row(vec!["Preview", &self.text_preview]);
        table.add_row(vec![
            "Posted",
            &format!("{} minutes ago", self.posted_minutes_ago),
        ]);
        table.add_row(vec![
            "Velocity",
            &format!("{:.1} engagements/hour", self.current_velocity),
        ]);
        table.add_row(vec!["Suggestion", &self.suggestion]);
        table
    }
}

impl Tableable for PerformanceReport {
    fn to_table(&self) -> comfy_table::Table {
        let mut table = comfy_table::Table::new();
        table.set_header(vec!["Metric", "Value"]);
        table.add_row(vec!["Period", &self.period]);
        table.add_row(vec!["Total Posts", &self.total_posts.to_string()]);
        table.add_row(vec![
            "Total Impressions",
            &self.total_impressions.to_string(),
        ]);
        table.add_row(vec![
            "Avg Engagement Rate",
            &format!("{:.2}%", self.avg_engagement_rate * 100.0),
        ]);
        table.add_row(vec!["Trend", &self.trend]);

        if let Some(ref bp) = self.best_post {
            table.add_row(vec![
                "Best Post",
                &format!(
                    "{} — {:.2}% ER, {} imp",
                    bp.text_preview,
                    bp.engagement_rate * 100.0,
                    bp.impressions
                ),
            ]);
        }
        if let Some(ref wp) = self.worst_post {
            table.add_row(vec![
                "Worst Post",
                &format!(
                    "{} — {:.2}% ER, {} imp",
                    wp.text_preview,
                    wp.engagement_rate * 100.0,
                    wp.impressions
                ),
            ]);
        }
        if let Some(ref bt) = self.best_time {
            table.add_row(vec![
                "Best Time",
                &format!(
                    "{} {:02}:00 ({:.2}% ER)",
                    bt.day_name,
                    bt.hour_of_day,
                    bt.avg_engagement_rate * 100.0
                ),
            ]);
        }

        if !self.content_breakdown.is_empty() {
            let breakdown: String = self
                .content_breakdown
                .iter()
                .map(|c| {
                    format!(
                        "{}: {} posts ({:.2}% ER)",
                        c.content_type,
                        c.count,
                        c.avg_engagement_rate * 100.0
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            table.add_row(vec!["Content Types", &breakdown]);
        }

        if !self.suggested_next_commands.is_empty() {
            table.add_row(vec![
                "Next Steps",
                &self.suggested_next_commands.join(", "),
            ]);
        }

        table
    }
}

impl Tableable for NextPostSuggestion {
    fn to_table(&self) -> comfy_table::Table {
        let mut table = comfy_table::Table::new();
        table.set_header(vec!["Field", "Value"]);
        table.add_row(vec![
            "Safe to Post Now?",
            if self.safe_to_post { "Yes" } else { "No" },
        ]);
        table.add_row(vec!["Recommendation", &self.recommendation]);

        if let Some(ref w) = self.cannibalization {
            table.add_row(vec![
                "Active Post",
                &format!(
                    "{} ({} min ago, {:.1} eng/hr)",
                    w.text_preview, w.posted_minutes_ago, w.current_velocity
                ),
            ]);
        }
        if let Some(ref bt) = self.best_time {
            table.add_row(vec![
                "Optimal Time",
                &format!(
                    "{} {:02}:00 ({:.2}% ER)",
                    bt.day_name,
                    bt.hour_of_day,
                    bt.avg_engagement_rate * 100.0
                ),
            ]);
        }
        table
    }
}
