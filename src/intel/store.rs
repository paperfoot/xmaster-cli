use chrono::{Datelike, Timelike, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use std::path::PathBuf;

use crate::config::config_dir;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct PostRecord {
    pub tweet_id: String,
    pub text: String,
    pub content_type: String,
    pub posted_at: i64,
    pub preflight_score: Option<f64>,
    pub latest_metrics: Option<SnapshotRecord>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SnapshotRecord {
    pub likes: i64,
    pub retweets: i64,
    pub replies: i64,
    pub impressions: i64,
    pub bookmarks: i64,
    pub engagement_rate: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct TimingSlot {
    pub day_of_week: i32,
    pub hour_of_day: i32,
    pub avg_impressions: f64,
    pub avg_engagement_rate: f64,
    pub sample_count: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct RecentVelocity {
    pub posts_1h: i64,
    pub posts_6h: i64,
    pub posts_24h: i64,
    pub accelerating_post: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReciprocityInfo {
    pub total_engagements: i64,
    pub replies_received: i64,
    pub reply_rate: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReciprocatorInfo {
    pub username: String,
    pub total_engagements: i64,
    pub replies_received: i64,
    pub reply_rate: f64,
    pub avg_followers: i64,
}

// ---------------------------------------------------------------------------
// IntelStore
// ---------------------------------------------------------------------------

pub struct IntelStore {
    conn: Connection,
}

impl IntelStore {
    /// Open (or create) the learning database at `~/.config/xmaster/xmaster.db`.
    pub fn open() -> Result<Self, rusqlite::Error> {
        let dir = config_dir();
        std::fs::create_dir_all(&dir).ok();
        let db_path: PathBuf = dir.join("xmaster.db");
        let conn = Connection::open(db_path)?;
        let store = Self { conn };
        store.init_tables()?;
        Ok(store)
    }

    // -- schema --------------------------------------------------------------

    fn init_tables(&self) -> Result<(), rusqlite::Error> {
        self.conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS posts (
                id              INTEGER PRIMARY KEY AUTOINCREMENT,
                tweet_id        TEXT UNIQUE NOT NULL,
                text            TEXT NOT NULL,
                content_type    TEXT NOT NULL DEFAULT 'text',
                char_count      INTEGER NOT NULL,
                has_link        BOOLEAN NOT NULL DEFAULT 0,
                has_media       BOOLEAN NOT NULL DEFAULT 0,
                has_poll        BOOLEAN NOT NULL DEFAULT 0,
                hashtag_count   INTEGER NOT NULL DEFAULT 0,
                hook_text       TEXT,
                posted_at       INTEGER NOT NULL,
                day_of_week     INTEGER NOT NULL,
                hour_of_day     INTEGER NOT NULL,
                reply_to_id     TEXT,
                quote_of_id     TEXT,
                preflight_score REAL
            );

            CREATE TABLE IF NOT EXISTS metric_snapshots (
                id                 INTEGER PRIMARY KEY AUTOINCREMENT,
                tweet_id           TEXT NOT NULL REFERENCES posts(tweet_id),
                snapshot_at        INTEGER NOT NULL,
                minutes_since_post INTEGER NOT NULL,
                likes              INTEGER NOT NULL DEFAULT 0,
                retweets           INTEGER NOT NULL DEFAULT 0,
                replies            INTEGER NOT NULL DEFAULT 0,
                impressions        INTEGER NOT NULL DEFAULT 0,
                bookmarks          INTEGER NOT NULL DEFAULT 0,
                quotes             INTEGER NOT NULL DEFAULT 0,
                profile_clicks     INTEGER NOT NULL DEFAULT 0
            );

            CREATE TABLE IF NOT EXISTS engagement_actions (
                id               INTEGER PRIMARY KEY AUTOINCREMENT,
                action_type      TEXT NOT NULL,
                target_tweet_id  TEXT,
                target_user_id   TEXT,
                target_username  TEXT,
                performed_at     INTEGER NOT NULL,
                got_reply_back   BOOLEAN DEFAULT NULL,
                target_followers INTEGER
            );

            CREATE TABLE IF NOT EXISTS timing_stats (
                id                  INTEGER PRIMARY KEY AUTOINCREMENT,
                day_of_week         INTEGER NOT NULL,
                hour_of_day         INTEGER NOT NULL,
                content_type        TEXT NOT NULL DEFAULT 'all',
                avg_impressions     REAL,
                avg_engagement_rate REAL,
                sample_count        INTEGER NOT NULL DEFAULT 0,
                last_updated        INTEGER NOT NULL
            );
            ",
        )?;
        Ok(())
    }

    // -- writes --------------------------------------------------------------

    /// Log a published post, extracting features automatically.
    pub fn log_post(
        &self,
        tweet_id: &str,
        text: &str,
        content_type: &str,
        reply_to: Option<&str>,
        quote_of: Option<&str>,
        preflight_score: Option<f64>,
    ) -> Result<(), rusqlite::Error> {
        let now = Utc::now();
        let posted_at = now.timestamp();
        let day_of_week = now.weekday().num_days_from_monday() as i32; // 0=Mon
        let hour_of_day = now.hour() as i32;
        let char_count = text.len() as i32;
        let has_link = text.contains("http://") || text.contains("https://");
        let hashtag_count = text.matches('#').count() as i32;
        let hook_text: String = text.chars().take(140).collect();

        self.conn.execute(
            "INSERT OR IGNORE INTO posts
                (tweet_id, text, content_type, char_count, has_link, has_media, has_poll,
                 hashtag_count, hook_text, posted_at, day_of_week, hour_of_day,
                 reply_to_id, quote_of_id, preflight_score)
             VALUES (?1,?2,?3,?4,?5,0,0,?6,?7,?8,?9,?10,?11,?12,?13)",
            params![
                tweet_id,
                text,
                content_type,
                char_count,
                has_link,
                hashtag_count,
                hook_text,
                posted_at,
                day_of_week,
                hour_of_day,
                reply_to,
                quote_of,
                preflight_score,
            ],
        )?;
        Ok(())
    }

    /// Record a metric snapshot for a tweet.
    #[allow(clippy::too_many_arguments)]
    pub fn log_metric_snapshot(
        &self,
        tweet_id: &str,
        likes: i64,
        retweets: i64,
        replies: i64,
        impressions: i64,
        bookmarks: i64,
        quotes: i64,
        profile_clicks: i64,
        minutes_since_post: i64,
    ) -> Result<(), rusqlite::Error> {
        let snapshot_at = Utc::now().timestamp();
        self.conn.execute(
            "INSERT INTO metric_snapshots
                (tweet_id, snapshot_at, minutes_since_post, likes, retweets, replies,
                 impressions, bookmarks, quotes, profile_clicks)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            params![
                tweet_id,
                snapshot_at,
                minutes_since_post,
                likes,
                retweets,
                replies,
                impressions,
                bookmarks,
                quotes,
                profile_clicks,
            ],
        )?;
        Ok(())
    }

    /// Log an engagement action (like, reply, retweet, etc.).
    pub fn log_engagement(
        &self,
        action_type: &str,
        target_tweet_id: Option<&str>,
        target_user_id: Option<&str>,
        target_username: Option<&str>,
        target_followers: Option<i64>,
    ) -> Result<(), rusqlite::Error> {
        let performed_at = Utc::now().timestamp();
        self.conn.execute(
            "INSERT INTO engagement_actions
                (action_type, target_tweet_id, target_user_id, target_username,
                 performed_at, target_followers)
             VALUES (?1,?2,?3,?4,?5,?6)",
            params![
                action_type,
                target_tweet_id,
                target_user_id,
                target_username,
                performed_at,
                target_followers,
            ],
        )?;
        Ok(())
    }

    // -- reads ---------------------------------------------------------------

    /// Recent posts with their latest metric snapshot.
    pub fn get_post_history(&self, limit: i64) -> Result<Vec<PostRecord>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(
            "SELECT tweet_id, text, content_type, posted_at, preflight_score
             FROM posts ORDER BY posted_at DESC LIMIT ?1",
        )?;

        let rows = stmt.query_map(params![limit], |row| {
            Ok(PostRecord {
                tweet_id: row.get(0)?,
                text: row.get(1)?,
                content_type: row.get(2)?,
                posted_at: row.get(3)?,
                preflight_score: row.get(4)?,
                latest_metrics: None, // filled below
            })
        })?;

        let mut posts: Vec<PostRecord> = rows.collect::<Result<Vec<_>, _>>()?;

        for post in &mut posts {
            post.latest_metrics = self.latest_snapshot(&post.tweet_id)?;
        }

        Ok(posts)
    }

    fn latest_snapshot(&self, tweet_id: &str) -> Result<Option<SnapshotRecord>, rusqlite::Error> {
        self.conn
            .query_row(
                "SELECT likes, retweets, replies, impressions, bookmarks
                 FROM metric_snapshots
                 WHERE tweet_id = ?1
                 ORDER BY snapshot_at DESC LIMIT 1",
                params![tweet_id],
                |row| {
                    let likes: i64 = row.get(0)?;
                    let retweets: i64 = row.get(1)?;
                    let replies: i64 = row.get(2)?;
                    let impressions: i64 = row.get(3)?;
                    let bookmarks: i64 = row.get(4)?;
                    let engagement_rate = if impressions > 0 {
                        (likes + retweets + replies + bookmarks) as f64 / impressions as f64
                    } else {
                        0.0
                    };
                    Ok(SnapshotRecord {
                        likes,
                        retweets,
                        replies,
                        impressions,
                        bookmarks,
                        engagement_rate,
                    })
                },
            )
            .optional()
    }

    /// Aggregated timing heatmap: avg engagement by day-of-week / hour-of-day.
    pub fn get_timing_heatmap(&self) -> Result<Vec<TimingSlot>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(
            "SELECT p.day_of_week, p.hour_of_day,
                    AVG(ms.impressions)                                         AS avg_imp,
                    AVG(CASE WHEN ms.impressions > 0
                         THEN (ms.likes + ms.retweets + ms.replies + ms.bookmarks) * 1.0
                              / ms.impressions ELSE 0 END)                      AS avg_er,
                    COUNT(DISTINCT p.tweet_id)                                  AS cnt
             FROM posts p
             JOIN metric_snapshots ms ON ms.tweet_id = p.tweet_id
             GROUP BY p.day_of_week, p.hour_of_day
             ORDER BY avg_er DESC",
        )?;

        let rows = stmt.query_map([], |row| {
            Ok(TimingSlot {
                day_of_week: row.get(0)?,
                hour_of_day: row.get(1)?,
                avg_impressions: row.get(2)?,
                avg_engagement_rate: row.get(3)?,
                sample_count: row.get(4)?,
            })
        })?;

        rows.collect()
    }

    /// Top N best posting time slots, optionally filtered by content type.
    pub fn get_best_posting_times(
        &self,
        content_type: Option<&str>,
        top_n: i64,
    ) -> Result<Vec<TimingSlot>, rusqlite::Error> {
        let (sql, use_filter) = match content_type {
            Some(_) => (
                "SELECT p.day_of_week, p.hour_of_day,
                        AVG(ms.impressions) AS avg_imp,
                        AVG(CASE WHEN ms.impressions > 0
                             THEN (ms.likes+ms.retweets+ms.replies+ms.bookmarks)*1.0
                                  / ms.impressions ELSE 0 END) AS avg_er,
                        COUNT(DISTINCT p.tweet_id) AS cnt
                 FROM posts p
                 JOIN metric_snapshots ms ON ms.tweet_id = p.tweet_id
                 WHERE p.content_type = ?1
                 GROUP BY p.day_of_week, p.hour_of_day
                 HAVING cnt >= 2
                 ORDER BY avg_er DESC
                 LIMIT ?2",
                true,
            ),
            None => (
                "SELECT p.day_of_week, p.hour_of_day,
                        AVG(ms.impressions) AS avg_imp,
                        AVG(CASE WHEN ms.impressions > 0
                             THEN (ms.likes+ms.retweets+ms.replies+ms.bookmarks)*1.0
                                  / ms.impressions ELSE 0 END) AS avg_er,
                        COUNT(DISTINCT p.tweet_id) AS cnt
                 FROM posts p
                 JOIN metric_snapshots ms ON ms.tweet_id = p.tweet_id
                 GROUP BY p.day_of_week, p.hour_of_day
                 HAVING cnt >= 2
                 ORDER BY avg_er DESC
                 LIMIT ?1",
                false,
            ),
        };

        let mut stmt = self.conn.prepare(sql)?;

        let map_row = |row: &rusqlite::Row| -> rusqlite::Result<TimingSlot> {
            Ok(TimingSlot {
                day_of_week: row.get(0)?,
                hour_of_day: row.get(1)?,
                avg_impressions: row.get(2)?,
                avg_engagement_rate: row.get(3)?,
                sample_count: row.get(4)?,
            })
        };

        let results: Vec<TimingSlot> = if use_filter {
            stmt.query_map(params![content_type.unwrap(), top_n], map_row)?
                .collect::<Result<Vec<_>, _>>()?
        } else {
            stmt.query_map(params![top_n], map_row)?
                .collect::<Result<Vec<_>, _>>()?
        };

        Ok(results)
    }

    /// Posts in the last 1h, 6h, 24h and whether any recent post is accelerating.
    pub fn get_recent_post_velocity(&self) -> Result<RecentVelocity, rusqlite::Error> {
        let now = Utc::now().timestamp();

        let posts_1h: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM posts
             WHERE posted_at > ?1 - 3600",
            params![now],
            |r| r.get(0),
        )?;
        let posts_6h: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM posts
             WHERE posted_at > ?1 - 21600",
            params![now],
            |r| r.get(0),
        )?;
        let posts_24h: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM posts
             WHERE posted_at > ?1 - 86400",
            params![now],
            |r| r.get(0),
        )?;

        // A post is "accelerating" if its latest two snapshots show increasing impressions
        let accelerating_post: Option<String> = self
            .conn
            .query_row(
                "SELECT s1.tweet_id
                 FROM metric_snapshots s1
                 JOIN metric_snapshots s2 ON s1.tweet_id = s2.tweet_id
                   AND s2.id = (SELECT MAX(id) FROM metric_snapshots
                                WHERE tweet_id = s1.tweet_id AND id < s1.id)
                 JOIN posts p ON p.tweet_id = s1.tweet_id
                 WHERE s1.id = (SELECT MAX(id) FROM metric_snapshots
                                WHERE tweet_id = s1.tweet_id)
                   AND p.posted_at > ?1 - 86400
                   AND s1.impressions > s2.impressions * 1.5
                 ORDER BY (s1.impressions - s2.impressions) DESC
                 LIMIT 1",
                params![now],
                |r| r.get(0),
            )
            .optional()?;

        Ok(RecentVelocity {
            posts_1h,
            posts_6h,
            posts_24h,
            accelerating_post,
        })
    }

    /// Recalculate the `timing_stats` table from raw posts + snapshots.
    pub fn update_timing_stats(&self) -> Result<(), rusqlite::Error> {
        let now = Utc::now().timestamp();
        let tx = self.conn.unchecked_transaction()?;
        tx.execute_batch("DELETE FROM timing_stats")?;
        tx.execute(
            "INSERT INTO timing_stats
                (day_of_week, hour_of_day, content_type,
                 avg_impressions, avg_engagement_rate, sample_count, last_updated)
             SELECT p.day_of_week, p.hour_of_day, p.content_type,
                    AVG(ms.impressions),
                    AVG(CASE WHEN ms.impressions > 0
                         THEN (ms.likes+ms.retweets+ms.replies+ms.bookmarks)*1.0
                              / ms.impressions ELSE 0 END),
                    COUNT(DISTINCT p.tweet_id),
                    ?1
             FROM posts p
             JOIN metric_snapshots ms ON ms.tweet_id = p.tweet_id
             GROUP BY p.day_of_week, p.hour_of_day, p.content_type",
            params![now],
        )?;
        // Also insert an 'all' row per slot
        tx.execute(
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
        )?;
        tx.commit()?;
        Ok(())
    }

    /// How often a user replies back after we engage with them.
    pub fn get_engagement_reciprocity(
        &self,
        username: &str,
    ) -> Result<Option<ReciprocityInfo>, rusqlite::Error> {
        self.conn
            .query_row(
                "SELECT COUNT(*) AS total,
                        SUM(CASE WHEN got_reply_back = 1 THEN 1 ELSE 0 END) AS replies_back
                 FROM engagement_actions
                 WHERE target_username = ?1",
                params![username],
                |row| {
                    let total: i64 = row.get(0)?;
                    if total == 0 {
                        return Ok(None);
                    }
                    let replies_received: i64 = row.get(1)?;
                    Ok(Some(ReciprocityInfo {
                        total_engagements: total,
                        replies_received,
                        reply_rate: if total > 0 {
                            replies_received as f64 / total as f64
                        } else {
                            0.0
                        },
                    }))
                },
            )
    }

    /// Top reciprocators: accounts that reply back most often, filtered by min followers.
    pub fn get_top_reciprocators(
        &self,
        min_followers: i64,
        limit: i64,
    ) -> Result<Vec<ReciprocatorInfo>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(
            "SELECT target_username,
                    COUNT(*) AS total,
                    SUM(CASE WHEN got_reply_back = 1 THEN 1 ELSE 0 END) AS replies_back,
                    AVG(target_followers) AS avg_fol
             FROM engagement_actions
             WHERE target_username IS NOT NULL
               AND target_followers >= ?1
             GROUP BY target_username
             HAVING replies_back > 0
             ORDER BY (CAST(replies_back AS REAL) / total) DESC
             LIMIT ?2",
        )?;

        let rows = stmt.query_map(params![min_followers, limit], |row| {
            let total: i64 = row.get(1)?;
            let replies_received: i64 = row.get(2)?;
            let avg_fol: f64 = row.get(3)?;
            Ok(ReciprocatorInfo {
                username: row.get(0)?,
                total_engagements: total,
                replies_received,
                reply_rate: if total > 0 {
                    replies_received as f64 / total as f64
                } else {
                    0.0
                },
                avg_followers: avg_fol as i64,
            })
        })?;

        rows.collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn test_store() -> IntelStore {
        let dir = tempdir().unwrap();
        std::env::set_var("XMASTER_CONFIG_DIR", dir.path());
        let store = IntelStore::open().unwrap();
        // Keep tempdir alive by leaking it (tests are short-lived)
        std::mem::forget(dir);
        store
    }

    #[test]
    fn log_and_retrieve_post() {
        let store = test_store();
        store
            .log_post("tweet_001", "Hello world!", "opinion", None, None, Some(85.0))
            .unwrap();

        let posts = store.get_post_history(10).unwrap();
        assert_eq!(posts.len(), 1);
        assert_eq!(posts[0].tweet_id, "tweet_001");
        assert_eq!(posts[0].text, "Hello world!");
        assert_eq!(posts[0].content_type, "opinion");
        assert_eq!(posts[0].preflight_score, Some(85.0));
        assert!(posts[0].latest_metrics.is_none());
    }

    #[test]
    fn duplicate_tweet_id_ignored() {
        let store = test_store();
        store
            .log_post("tweet_dup", "First", "opinion", None, None, None)
            .unwrap();
        // INSERT OR IGNORE — second insert should not fail or create duplicate
        store
            .log_post("tweet_dup", "Second", "opinion", None, None, None)
            .unwrap();

        let posts = store.get_post_history(10).unwrap();
        assert_eq!(posts.len(), 1);
        assert_eq!(posts[0].text, "First"); // original text preserved
    }

    #[test]
    fn engagement_logging() {
        let store = test_store();
        store
            .log_engagement("like", Some("t_100"), None, Some("testuser"), Some(5000))
            .unwrap();
        store
            .log_engagement("reply", Some("t_101"), None, Some("testuser"), Some(5000))
            .unwrap();

        let info = store.get_engagement_reciprocity("testuser").unwrap();
        assert!(info.is_some());
        let info = info.unwrap();
        assert_eq!(info.total_engagements, 2);
    }

    #[test]
    fn timing_heatmap_empty_db() {
        let store = test_store();
        let heatmap = store.get_timing_heatmap().unwrap();
        assert!(heatmap.is_empty());
    }

    #[test]
    fn recent_velocity_empty_db() {
        let store = test_store();
        let velocity = store.get_recent_post_velocity().unwrap();
        assert_eq!(velocity.posts_1h, 0);
        assert_eq!(velocity.posts_6h, 0);
        assert_eq!(velocity.posts_24h, 0);
        assert!(velocity.accelerating_post.is_none());
    }

    #[test]
    fn metric_snapshot_and_retrieval() {
        let store = test_store();
        store
            .log_post("tweet_metrics", "Test post", "opinion", None, None, None)
            .unwrap();
        store
            .log_metric_snapshot("tweet_metrics", 10, 5, 3, 1000, 2, 1, 50, 60)
            .unwrap();

        let posts = store.get_post_history(10).unwrap();
        assert_eq!(posts.len(), 1);
        let metrics = posts[0].latest_metrics.as_ref().unwrap();
        assert_eq!(metrics.likes, 10);
        assert_eq!(metrics.retweets, 5);
        assert_eq!(metrics.replies, 3);
        assert_eq!(metrics.impressions, 1000);
        assert_eq!(metrics.bookmarks, 2);
        assert!(metrics.engagement_rate > 0.0);
    }

    #[test]
    fn engagement_reciprocity_unknown_user() {
        let store = test_store();
        let info = store.get_engagement_reciprocity("nobody").unwrap();
        assert!(info.is_none());
    }

    #[test]
    fn update_timing_stats_empty_db() {
        let store = test_store();
        // Should not fail on empty database
        store.update_timing_stats().unwrap();
    }
}
