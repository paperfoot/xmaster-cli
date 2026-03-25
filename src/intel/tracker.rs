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

#[derive(Debug, Default)]
struct FetchedMetrics {
    likes: i64,
    retweets: i64,
    replies: i64,
    impressions: i64,
    bookmarks: i64,
    quotes: i64,
    profile_clicks: i64,
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
                profile_clicks INTEGER NOT NULL DEFAULT 0
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

        Ok(Self { conn })
    }

    // -- snapshot a single tweet via X API ------------------------------------

    /// Fetch current metrics for a single tweet and record a snapshot.
    /// Uses `reqwest_oauth1` directly, mirroring the pattern in commands/metrics.rs.
    pub async fn snapshot_tweet(
        &self,
        ctx: &crate::context::AppContext,
        tweet_id: &str,
        minutes_since_post: i64,
    ) -> Result<(), XmasterError> {
        let metrics = fetch_tweet_metrics(ctx, tweet_id).await?;

        let snapshot_at = Utc::now().timestamp();
        self.conn
            .execute(
                "INSERT INTO metric_snapshots
                    (tweet_id, snapshot_at, minutes_since_post, likes, retweets, replies,
                     impressions, bookmarks, quotes, profile_clicks)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
                params![
                    tweet_id,
                    snapshot_at,
                    minutes_since_post,
                    metrics.likes,
                    metrics.retweets,
                    metrics.replies,
                    metrics.impressions,
                    metrics.bookmarks,
                    metrics.quotes,
                    metrics.profile_clicks,
                ],
            )
            .map_err(|e| XmasterError::Config(format!("DB error: {e}")))?;
        Ok(())
    }

    // -- snapshot all recent posts --------------------------------------------

    pub async fn snapshot_all_recent(
        &self,
        ctx: &crate::context::AppContext,
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
            )
            .map_err(|e| XmasterError::Config(format!("DB error: {e}")))?;

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
            })
            .map_err(|e| XmasterError::Config(format!("DB error: {e}")))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| XmasterError::Config(format!("DB error: {e}")))?;

        let mut snapshotted = 0u32;
        let mut errors = 0u32;

        for (tweet_id, posted_at) in &rows {
            let posted = DateTime::from_timestamp(*posted_at, 0).unwrap_or(now);
            let minutes = (now - posted).num_minutes();

            match self.snapshot_tweet(ctx, tweet_id, minutes).await {
                Ok(()) => snapshotted += 1,
                Err(e) => {
                    tracing::warn!(tweet_id = %tweet_id, error = %e, "Failed to snapshot");
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
                "SELECT p.day_of_week, p.hour_of_day,
                        AVG(ms.impressions) AS avg_imp,
                        AVG(CASE WHEN ms.impressions > 0
                             THEN (ms.likes + ms.retweets + ms.replies + ms.bookmarks) * 1.0
                                  / ms.impressions ELSE 0 END) AS avg_er,
                        COUNT(DISTINCT p.tweet_id) AS cnt
                 FROM posts p
                 JOIN metric_snapshots ms ON ms.tweet_id = p.tweet_id
                 GROUP BY p.day_of_week, p.hour_of_day
                 ORDER BY avg_er DESC",
            )
            .map_err(|e| XmasterError::Config(format!("DB error: {e}")))?;

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
            })
            .map_err(|e| XmasterError::Config(format!("DB error: {e}")))?;

        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| XmasterError::Config(format!("DB error: {e}")))
    }

    // -- best posting time ----------------------------------------------------

    pub fn get_best_time(
        &self,
        content_type: Option<&str>,
    ) -> Result<Option<TimingSlot>, XmasterError> {
        let total: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM posts", [], |r| r.get(0))
            .map_err(|e| XmasterError::Config(format!("DB error: {e}")))?;

        if total < 10 {
            return Ok(None);
        }

        let slot = match content_type {
            Some(ct) => self
                .conn
                .query_row(
                    "SELECT p.day_of_week, p.hour_of_day,
                            AVG(ms.impressions),
                            AVG(CASE WHEN ms.impressions > 0
                                 THEN (ms.likes+ms.retweets+ms.replies+ms.bookmarks)*1.0
                                      / ms.impressions ELSE 0 END),
                            COUNT(DISTINCT p.tweet_id)
                     FROM posts p
                     JOIN metric_snapshots ms ON ms.tweet_id = p.tweet_id
                     WHERE p.content_type = ?1
                     GROUP BY p.day_of_week, p.hour_of_day
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
                .optional()
                .map_err(|e| XmasterError::Config(format!("DB error: {e}")))?,
            None => self
                .conn
                .query_row(
                    "SELECT p.day_of_week, p.hour_of_day,
                            AVG(ms.impressions),
                            AVG(CASE WHEN ms.impressions > 0
                                 THEN (ms.likes+ms.retweets+ms.replies+ms.bookmarks)*1.0
                                      / ms.impressions ELSE 0 END),
                            COUNT(DISTINCT p.tweet_id)
                     FROM posts p
                     JOIN metric_snapshots ms ON ms.tweet_id = p.tweet_id
                     GROUP BY p.day_of_week, p.hour_of_day
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
                .optional()
                .map_err(|e| XmasterError::Config(format!("DB error: {e}")))?,
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
            .optional()
            .map_err(|e| XmasterError::Config(format!("DB error: {e}")))?;

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
                "SELECT (likes + retweets + replies + bookmarks) * 1.0
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
                                   + COALESCE(ms.replies,0) + COALESCE(ms.bookmarks,0)) * 1.0
                                  / ms.impressions
                             ELSE 0 END AS er
                 FROM posts p
                 LEFT JOIN metric_snapshots ms ON ms.tweet_id = p.tweet_id
                   AND ms.id = (SELECT MAX(id) FROM metric_snapshots WHERE tweet_id = p.tweet_id)
                 WHERE p.posted_at > ?1
                 ORDER BY er DESC",
            )
            .map_err(|e| XmasterError::Config(format!("DB error: {e}")))?;

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
            })
            .map_err(|e| XmasterError::Config(format!("DB error: {e}")))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| XmasterError::Config(format!("DB error: {e}")))?;

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
                               +COALESCE(ms.replies,0)+COALESCE(ms.bookmarks,0))*1.0
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
            )
            .map_err(|e| XmasterError::Config(format!("DB error: {e}")))?
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

        let mut stmt = self
            .conn
            .prepare(
                "SELECT p.tweet_id,
                        SUBSTR(p.text, 1, 60) AS preview,
                        p.posted_at,
                        (SELECT COUNT(*) FROM metric_snapshots ms WHERE ms.tweet_id = p.tweet_id) AS snap_count,
                        (SELECT MAX(ms.snapshot_at) FROM metric_snapshots ms WHERE ms.tweet_id = p.tweet_id) AS last_snap,
                        (SELECT ms.impressions FROM metric_snapshots ms WHERE ms.tweet_id = p.tweet_id ORDER BY ms.id DESC LIMIT 1),
                        (SELECT CASE WHEN ms.impressions > 0
                                     THEN (ms.likes+ms.retweets+ms.replies+ms.bookmarks)*1.0 / ms.impressions
                                     ELSE 0 END
                         FROM metric_snapshots ms WHERE ms.tweet_id = p.tweet_id ORDER BY ms.id DESC LIMIT 1)
                 FROM posts p
                 ORDER BY p.posted_at DESC
                 LIMIT 50",
            )
            .map_err(|e| XmasterError::Config(format!("DB error: {e}")))?;

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
            })
            .map_err(|e| XmasterError::Config(format!("DB error: {e}")))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| XmasterError::Config(format!("DB error: {e}")))?;

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
            .execute_batch("DELETE FROM timing_stats")
            .map_err(|e| XmasterError::Config(format!("DB error: {e}")))?;
        self.conn
            .execute(
                "INSERT INTO timing_stats
                    (day_of_week, hour_of_day, content_type,
                     avg_impressions, avg_engagement_rate, sample_count, last_updated)
                 SELECT p.day_of_week, p.hour_of_day, 'all',
                        AVG(ms.impressions),
                        AVG(CASE WHEN ms.impressions > 0
                             THEN (ms.likes+ms.retweets+ms.replies+ms.bookmarks)*1.0
                                  / ms.impressions ELSE 0 END),
                        COUNT(DISTINCT p.tweet_id),
                        ?1
                 FROM posts p
                 JOIN metric_snapshots ms ON ms.tweet_id = p.tweet_id
                 GROUP BY p.day_of_week, p.hour_of_day",
                params![now],
            )
            .map_err(|e| XmasterError::Config(format!("DB error: {e}")))?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Fetch tweet metrics via X API (standalone, no XApi dependency)
// ---------------------------------------------------------------------------

use reqwest_oauth1::OAuthClientProvider;
use serde::Deserialize;

#[derive(Deserialize)]
struct MetricsEnvelope {
    data: Option<MetricsTweetData>,
}

#[derive(Deserialize)]
struct MetricsTweetData {
    #[serde(default)]
    public_metrics: Option<MetricsPublic>,
    #[serde(default)]
    non_public_metrics: Option<MetricsNonPublic>,
}

#[derive(Deserialize, Default)]
struct MetricsPublic {
    #[serde(default)]
    like_count: i64,
    #[serde(default)]
    retweet_count: i64,
    #[serde(default)]
    reply_count: i64,
    #[serde(default)]
    impression_count: i64,
    #[serde(default)]
    bookmark_count: i64,
    #[serde(default)]
    quote_count: i64,
}

#[derive(Deserialize, Default)]
struct MetricsNonPublic {
    #[serde(default)]
    user_profile_clicks: i64,
}

fn oauth_secrets(ctx: &crate::context::AppContext) -> reqwest_oauth1::Secrets<'_> {
    let k = &ctx.config.keys;
    reqwest_oauth1::Secrets::new(&k.api_key, &k.api_secret)
        .token(&k.access_token, &k.access_token_secret)
}

async fn fetch_tweet_metrics(
    ctx: &crate::context::AppContext,
    tweet_id: &str,
) -> Result<FetchedMetrics, XmasterError> {
    if !ctx.config.has_x_auth() {
        return Err(XmasterError::AuthMissing {
            provider: "x",
            message: "X API credentials not configured".into(),
        });
    }

    let url = format!(
        "https://api.x.com/2/tweets/{tweet_id}?tweet.fields=public_metrics,non_public_metrics"
    );

    let resp = ctx
        .client
        .clone()
        .oauth1(oauth_secrets(ctx))
        .get(&url)
        .send()
        .await?;

    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(XmasterError::Api {
            provider: "x",
            code: "api_error",
            message: format!("HTTP {status}: {text}"),
        });
    }

    let envelope: MetricsEnvelope = resp.json().await?;
    let tweet = envelope
        .data
        .ok_or_else(|| XmasterError::NotFound(format!("Tweet {tweet_id}")))?;

    let pub_m = tweet.public_metrics.unwrap_or_default();
    let non_pub = tweet.non_public_metrics.unwrap_or_default();

    Ok(FetchedMetrics {
        likes: pub_m.like_count,
        retweets: pub_m.retweet_count,
        replies: pub_m.reply_count,
        impressions: pub_m.impression_count,
        bookmarks: pub_m.bookmark_count,
        quotes: pub_m.quote_count,
        profile_clicks: non_pub.user_profile_clicks,
    })
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
