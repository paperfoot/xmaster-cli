use chrono::{Datelike, Timelike, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::config::config_dir;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct DiscoveredPostRow {
    pub tweet_id: String,
    pub author_username: String,
    pub text: String,
    pub like_count: i64,
    pub impression_count: i64,
    pub last_source: String,
    pub first_discovered_at: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct WatchlistEntry {
    pub username: String,
    pub user_id: Option<String>,
    pub topic: Option<String>,
    pub followers: i64,
    pub added_at: i64,
}

/// A reply target that earned a spot in the watchlist based on reply outcomes.
/// Returned by `find_hot_reply_targets` for automatic promotion.
#[derive(Debug, Clone, Serialize)]
pub struct HotReplyTarget {
    pub username: String,
    pub user_id: Option<String>,
    pub target_followers: i64,
}

/// Aggregated stats for a reply target across all replies in a time window.
/// Returned by `rank_hot_reply_targets` for the `engage hot-targets` command.
#[derive(Debug, Clone, Serialize)]
pub struct HotTargetStats {
    pub username: String,
    pub sample_count: i64,
    pub avg_impressions: f64,
    pub avg_profile_clicks: f64,
    pub reply_back_rate: f64,
    pub last_reply_at: i64,
    pub score: f64,
}

#[derive(Debug, Clone)]
pub struct PendingReply {
    pub id: i64,
    pub reply_tweet_id: String,
    pub target_username: Option<String>,
    pub performed_at: i64,
}

/// Full metric snapshot row — used by the metrics command to compute deltas.
/// Unlike `SnapshotRecord` (which is a summary), this includes snapshot_at + all counters.
#[derive(Debug, Clone)]
pub struct FullSnapshot {
    pub snapshot_at: i64,
    pub minutes_since_post: i64,
    pub likes: i64,
    pub retweets: i64,
    pub replies: i64,
    pub impressions: i64,
    pub bookmarks: i64,
    pub quotes: i64,
    pub profile_clicks: i64,
}

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
    /// Original posts (non-replies) in the last 24h. The 2026 algorithm treats
    /// >4 standalone posts/day as a spam-flag risk, but heavy replying is fine.
    pub standalone_24h: i64,
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
// ReplyStyle classification
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ReplyStyle {
    Question,
    DataPoint,
    Counterpoint,
    Anecdote,
    Humor,
    Agreement,
}

impl ReplyStyle {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Question => "question",
            Self::DataPoint => "data_point",
            Self::Counterpoint => "counterpoint",
            Self::Anecdote => "anecdote",
            Self::Humor => "humor",
            Self::Agreement => "agreement",
        }
    }
}

impl std::fmt::Display for ReplyStyle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Classify a reply's rhetorical style using simple heuristics.
pub fn classify_reply(text: &str) -> ReplyStyle {
    let lower = text.to_lowercase();

    if text.contains('?') {
        return ReplyStyle::Question;
    }

    // DataPoint: numbers, percentages, stats
    if lower.contains('%')
        || lower.chars().any(|c| c.is_ascii_digit())
            && (lower.contains("study") || lower.contains("data") || lower.contains("stat"))
    {
        return ReplyStyle::DataPoint;
    }

    // Counterpoint: disagreement markers
    let counterpoint_markers = ["but ", "however", "actually", "disagree", "on the other hand"];
    if counterpoint_markers.iter().any(|m| lower.contains(m)) {
        return ReplyStyle::Counterpoint;
    }

    // Anecdote: personal experience markers
    let anecdote_markers = [
        "i've ", "i tested", "in my experience", "i found", "i noticed",
        "i tried", "personally", "my own",
    ];
    if anecdote_markers.iter().any(|m| lower.contains(m)) {
        return ReplyStyle::Anecdote;
    }

    // Humor: casual tone markers
    let humor_markers = ["lol", "lmao", "haha", "rofl", "😂", "🤣", "💀"];
    if humor_markers.iter().any(|m| lower.contains(m)) {
        return ReplyStyle::Humor;
    }

    ReplyStyle::Agreement
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
        Self::open_at(&db_path)
    }

    /// Open the database at an explicit path. Used by tests to avoid the
    /// process-wide `XMASTER_CONFIG_DIR` env var that causes race conditions
    /// when `cargo test` runs in parallel.
    pub fn open_at(path: &std::path::Path) -> Result<Self, rusqlite::Error> {
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "wal")?;
        conn.pragma_update(None, "busy_timeout", 5000)?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
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
                preflight_score REAL,
                analysis_json   TEXT,
                analysis_version INTEGER DEFAULT 1,
                scheduled_post_id TEXT,
                local_day_of_week INTEGER,
                local_hour_of_day INTEGER,
                tz_offset_minutes INTEGER
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
                profile_clicks     INTEGER NOT NULL DEFAULT 0,
                url_clicks         INTEGER
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

            CREATE TABLE IF NOT EXISTS watchlist_accounts (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                username    TEXT UNIQUE NOT NULL,
                user_id     TEXT,
                topic       TEXT,
                followers   INTEGER NOT NULL DEFAULT 0,
                added_at    INTEGER NOT NULL
            );

            -- Add reply_tweet_id if not exists (safe for existing DBs)
            -- SQLite doesn't support IF NOT EXISTS for ALTER, so we use a pragma check
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

            CREATE TABLE IF NOT EXISTS discovered_posts (
                id                     INTEGER PRIMARY KEY AUTOINCREMENT,
                tweet_id               TEXT UNIQUE NOT NULL,
                text                   TEXT NOT NULL,
                author_id              TEXT,
                author_username        TEXT,
                created_at             TEXT,
                conversation_id        TEXT,
                referenced_tweets_json TEXT NOT NULL DEFAULT '[]',
                like_count             INTEGER,
                retweet_count          INTEGER,
                reply_count            INTEGER,
                impression_count       INTEGER,
                bookmark_count         INTEGER,
                author_followers       INTEGER,
                media_urls_json        TEXT NOT NULL DEFAULT '[]',
                first_source           TEXT NOT NULL,
                last_source            TEXT NOT NULL,
                first_discovered_at    INTEGER NOT NULL,
                last_seen_at           INTEGER NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_discovered_author
                ON discovered_posts(author_username);
            CREATE INDEX IF NOT EXISTS idx_discovered_last_seen
                ON discovered_posts(last_seen_at DESC);
            CREATE INDEX IF NOT EXISTS idx_discovered_impressions
                ON discovered_posts(impression_count DESC);
            ",
        )?;

        // Safe migrations: add columns if not present (transaction for atomicity)
        self.conn.execute_batch("BEGIN IMMEDIATE;")?;
        let migrate_result = (|| -> Result<(), rusqlite::Error> {
            let cols: Vec<String> = self.conn
                .prepare("PRAGMA table_info(engagement_actions)")?
                .query_map([], |row| row.get::<_, String>(1))?
                .collect::<Result<Vec<_>, _>>()?;
            if !cols.iter().any(|c| c == "reply_tweet_id") {
                self.conn.execute_batch("ALTER TABLE engagement_actions ADD COLUMN reply_tweet_id TEXT;")?;
            }
            if !cols.iter().any(|c| c == "reply_style") {
                self.conn.execute_batch("ALTER TABLE engagement_actions ADD COLUMN reply_style TEXT;")?;
            }

            let post_cols: Vec<String> = self.conn
                .prepare("PRAGMA table_info(posts)")?
                .query_map([], |row| row.get::<_, String>(1))?
                .collect::<Result<Vec<_>, _>>()?;
            if !post_cols.iter().any(|c| c == "analysis_json") {
                self.conn.execute_batch("ALTER TABLE posts ADD COLUMN analysis_json TEXT;")?;
            }
            if !post_cols.iter().any(|c| c == "analysis_version") {
                self.conn.execute_batch("ALTER TABLE posts ADD COLUMN analysis_version INTEGER DEFAULT 1;")?;
            }
            if !post_cols.iter().any(|c| c == "scheduled_post_id") {
                self.conn.execute_batch("ALTER TABLE posts ADD COLUMN scheduled_post_id TEXT;")?;
            }

            // Add url_clicks to metric_snapshots if missing (legacy DBs predating the field)
            let ms_cols: Vec<String> = self.conn
                .prepare("PRAGMA table_info(metric_snapshots)")?
                .query_map([], |row| row.get::<_, String>(1))?
                .collect::<Result<Vec<_>, _>>()?;
            if !ms_cols.iter().any(|c| c == "url_clicks") {
                self.conn.execute_batch("ALTER TABLE metric_snapshots ADD COLUMN url_clicks INTEGER;")?;
            }

            // NOTE: engagement_actions.performed_at has TEXT column affinity
            // (declared TEXT in tracker.rs original CREATE TABLE). SQLite's
            // UPDATE ... SET col = CAST(col AS INTEGER) does NOT change the
            // stored type when the declared column affinity is TEXT. The only
            // real fix is DROP + CREATE TABLE with INTEGER affinity + INSERT
            // from old. That's tracked in issue #3 and too risky for an
            // additive migration. Queries on performed_at MUST use CAST
            // guards until the table rebuild lands.
            //
            // metric_snapshots.snapshot_at was declared INTEGER in store.rs
            // (correct affinity) so the IntelStore path stores INTEGER values.
            // However the tracker.rs CREATE TABLE declared it differently in
            // older versions, so existing rows may be text. The CAST in
            // latest_snapshot_full handles this case.

            Ok(())
        })();
        match migrate_result {
            Ok(()) => self.conn.execute_batch("COMMIT;")?,
            Err(e) => { self.conn.execute_batch("ROLLBACK;").ok(); return Err(e); }
        }

        // reply_outcomes view v2: extended with profile_clicks, quotes,
        // url_clicks, snapshot_at, minutes_since_post, and target_user_id.
        // DROP + CREATE because views hold no data. Safe on every open.
        self.conn.execute_batch("DROP VIEW IF EXISTS reply_outcomes;")?;
        self.conn.execute_batch(
            "CREATE VIEW reply_outcomes AS
             SELECT ea.id AS action_id,
                    ea.target_tweet_id,
                    ea.target_user_id,
                    ea.target_username,
                    ea.target_followers,
                    ea.reply_tweet_id,
                    ea.reply_style,
                    ea.performed_at,
                    ea.got_reply_back,
                    ms.likes,
                    ms.retweets,
                    ms.replies,
                    ms.impressions,
                    ms.bookmarks,
                    ms.quotes,
                    ms.profile_clicks,
                    ms.url_clicks,
                    CAST(ms.snapshot_at AS INTEGER) AS snapshot_at,
                    ms.minutes_since_post
             FROM engagement_actions ea
             LEFT JOIN metric_snapshots ms
               ON ms.tweet_id = ea.reply_tweet_id
               AND ms.id = (
                   SELECT MAX(ms2.id) FROM metric_snapshots ms2
                   WHERE ms2.tweet_id = ea.reply_tweet_id
               )
             WHERE ea.action_type = 'reply'
               AND ea.reply_tweet_id IS NOT NULL;"
        )?;

        Ok(())
    }

    /// Static helper: classify a reply's rhetorical style.
    pub fn classify_reply_style(text: &str) -> ReplyStyle {
        classify_reply(text)
    }

    // -- watchlist ------------------------------------------------------------

    pub fn add_watchlist(&self, username: &str, user_id: Option<&str>, topic: Option<&str>, followers: i64) -> Result<(), rusqlite::Error> {
        let now = Utc::now().timestamp();
        self.conn.execute(
            "INSERT OR REPLACE INTO watchlist_accounts (username, user_id, topic, followers, added_at)
             VALUES (LOWER(?1), ?2, ?3, ?4, ?5)",
            params![username.to_lowercase(), user_id, topic, followers, now],
        )?;
        Ok(())
    }

    pub fn remove_watchlist(&self, username: &str) -> Result<bool, rusqlite::Error> {
        let changed = self.conn.execute(
            "DELETE FROM watchlist_accounts WHERE username = LOWER(?1)",
            params![username.to_lowercase()],
        )?;
        Ok(changed > 0)
    }

    pub fn list_watchlist(&self) -> Result<Vec<WatchlistEntry>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(
            "SELECT username, user_id, topic, followers, added_at FROM watchlist_accounts ORDER BY added_at DESC"
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(WatchlistEntry {
                username: row.get(0)?,
                user_id: row.get(1)?,
                topic: row.get(2)?,
                followers: row.get(3)?,
                added_at: row.get(4)?,
            })
        })?;
        rows.collect()
    }

    /// Log a reply with the reply tweet ID and style for tracking reply-backs.
    pub fn log_reply(
        &self,
        target_tweet_id: &str,
        target_user_id: Option<&str>,
        target_username: Option<&str>,
        target_followers: Option<i64>,
        reply_tweet_id: &str,
        reply_style: Option<&ReplyStyle>,
    ) -> Result<(), rusqlite::Error> {
        let performed_at = Utc::now().timestamp();
        self.conn.execute(
            "INSERT INTO engagement_actions
                (action_type, target_tweet_id, target_user_id, target_username,
                 performed_at, target_followers, reply_tweet_id, reply_style)
             VALUES ('reply', ?1, ?2, LOWER(?3), ?4, ?5, ?6, ?7)",
            params![
                target_tweet_id,
                target_user_id,
                target_username.map(|u| u.to_lowercase()),
                performed_at,
                target_followers,
                reply_tweet_id,
                reply_style.map(|s| s.as_str()),
            ],
        )?;
        Ok(())
    }

    /// Get pending replies that haven't been checked for reply-back yet.
    pub fn get_pending_replies(&self, max_age_hours: i64) -> Result<Vec<PendingReply>, rusqlite::Error> {
        let cutoff = Utc::now().timestamp() - (max_age_hours * 3600);
        let mut stmt = self.conn.prepare(
            "SELECT id, reply_tweet_id, target_username, performed_at
             FROM engagement_actions
             WHERE action_type = 'reply' AND got_reply_back IS NULL
               AND reply_tweet_id IS NOT NULL AND performed_at > ?1
             ORDER BY performed_at DESC"
        )?;
        let rows = stmt.query_map(params![cutoff], |row| {
            Ok(PendingReply {
                id: row.get(0)?,
                reply_tweet_id: row.get(1)?,
                target_username: row.get(2)?,
                performed_at: row.get(3)?,
            })
        })?;
        rows.collect()
    }

    /// Mark a reply as having received (or not) a reply-back.
    pub fn set_reply_back(&self, action_id: i64, got_reply: bool) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "UPDATE engagement_actions SET got_reply_back = ?1 WHERE id = ?2",
            params![got_reply as i32, action_id],
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
        analysis_json: Option<&str>,
        scheduled_post_id: Option<&str>,
    ) -> Result<(), rusqlite::Error> {
        let now = Utc::now();
        let local_now = chrono::Local::now();
        let posted_at = now.timestamp();
        let day_of_week = now.weekday().num_days_from_monday() as i32; // 0=Mon (UTC)
        let hour_of_day = now.hour() as i32; // UTC
        let local_day = local_now.weekday().num_days_from_monday() as i32;
        let local_hour = local_now.hour() as i32;
        let tz_offset = local_now.offset().local_minus_utc() / 60;
        let char_count = text.chars().count() as i32;
        let has_link = text.contains("http://") || text.contains("https://");
        let hashtag_count = text.matches('#').count() as i32;
        let hook_text: String = text.chars().take(140).collect();

        self.conn.execute(
            "INSERT OR IGNORE INTO posts
                (tweet_id, text, content_type, char_count, has_link, has_media, has_poll,
                 hashtag_count, hook_text, posted_at, day_of_week, hour_of_day,
                 reply_to_id, quote_of_id, preflight_score,
                 analysis_json, analysis_version, scheduled_post_id,
                 local_day_of_week, local_hour_of_day, tz_offset_minutes)
             VALUES (?1,?2,?3,?4,?5,0,0,?6,?7,?8,?9,?10,?11,?12,?13,?14,1,?15,?16,?17,?18)",
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
                analysis_json,
                scheduled_post_id,
                local_day,
                local_hour,
                tz_offset,
            ],
        )?;
        Ok(())
    }

    /// Convenience function to record a published post with full metadata.
    /// Centralizes post recording logic: writes the post row, attaches analysis,
    /// links to scheduled_posts when applicable, and logs reply metadata for replies.
    #[allow(clippy::too_many_arguments)]
    pub fn record_published_post(
        &self,
        tweet_id: &str,
        text: &str,
        content_type: &str,
        reply_to: Option<&str>,
        quote_of: Option<&str>,
        preflight_score: Option<f64>,
        analysis_json: Option<&str>,
        scheduled_post_id: Option<&str>,
    ) -> Result<(), rusqlite::Error> {
        self.log_post(
            tweet_id,
            text,
            content_type,
            reply_to,
            quote_of,
            preflight_score,
            analysis_json,
            scheduled_post_id,
        )
    }

    // -- discovered posts library ---------------------------------------------

    /// Cache external posts encountered during search/timeline/read commands.
    /// UPSERT: first encounter preserves source/timestamp, re-encounters update metrics.
    pub fn record_discovered_posts(
        &self,
        source: &str,
        tweets: &[crate::providers::xapi::TweetData],
    ) -> Result<(), rusqlite::Error> {
        if tweets.is_empty() {
            return Ok(());
        }
        let now = Utc::now().timestamp();
        let tx = self.conn.unchecked_transaction()?;
        {
            let mut stmt = tx.prepare_cached(
                "INSERT INTO discovered_posts (
                    tweet_id, text, author_id, author_username, created_at,
                    conversation_id, referenced_tweets_json,
                    like_count, retweet_count, reply_count, impression_count,
                    bookmark_count, author_followers, media_urls_json,
                    first_source, last_source, first_discovered_at, last_seen_at
                ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?15,?16,?16)
                ON CONFLICT(tweet_id) DO UPDATE SET
                    text = excluded.text,
                    author_id = COALESCE(excluded.author_id, discovered_posts.author_id),
                    author_username = COALESCE(excluded.author_username, discovered_posts.author_username),
                    created_at = COALESCE(excluded.created_at, discovered_posts.created_at),
                    conversation_id = COALESCE(excluded.conversation_id, discovered_posts.conversation_id),
                    referenced_tweets_json = CASE
                        WHEN excluded.referenced_tweets_json <> '[]' THEN excluded.referenced_tweets_json
                        ELSE discovered_posts.referenced_tweets_json END,
                    like_count = COALESCE(excluded.like_count, discovered_posts.like_count),
                    retweet_count = COALESCE(excluded.retweet_count, discovered_posts.retweet_count),
                    reply_count = COALESCE(excluded.reply_count, discovered_posts.reply_count),
                    impression_count = COALESCE(excluded.impression_count, discovered_posts.impression_count),
                    bookmark_count = COALESCE(excluded.bookmark_count, discovered_posts.bookmark_count),
                    author_followers = COALESCE(excluded.author_followers, discovered_posts.author_followers),
                    media_urls_json = CASE
                        WHEN excluded.media_urls_json <> '[]' THEN excluded.media_urls_json
                        ELSE discovered_posts.media_urls_json END,
                    last_source = excluded.last_source,
                    last_seen_at = excluded.last_seen_at",
            )?;
            for t in tweets {
                let m = t.public_metrics.as_ref();
                let refs_json = serde_json::to_string(
                    &t.referenced_tweets.as_deref().unwrap_or(&[])
                ).unwrap_or_else(|_| "[]".into());
                let media_json = serde_json::to_string(&t.media_urls)
                    .unwrap_or_else(|_| "[]".into());
                stmt.execute(params![
                    t.id,
                    t.text,
                    t.author_id,
                    t.author_username,
                    t.created_at,
                    t.conversation_id,
                    refs_json,
                    m.map(|m| m.like_count as i64),
                    m.map(|m| m.retweet_count as i64),
                    m.map(|m| m.reply_count as i64),
                    m.map(|m| m.impression_count as i64),
                    m.map(|m| m.bookmark_count as i64),
                    t.author_followers.map(|f| f as i64),
                    media_json,
                    source,
                    now,
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Cache a single discovered post.
    pub fn record_discovered_post(
        &self,
        source: &str,
        tweet: &crate::providers::xapi::TweetData,
    ) -> Result<(), rusqlite::Error> {
        self.record_discovered_posts(source, std::slice::from_ref(tweet))
    }

    /// Query the discovered posts library with optional filters.
    pub fn query_discovered_posts(
        &self,
        topic: Option<&str>,
        author: Option<&str>,
        min_likes: Option<i64>,
        limit: usize,
    ) -> Result<Vec<DiscoveredPostRow>, rusqlite::Error> {
        let mut sql = String::from(
            "SELECT tweet_id, COALESCE(author_username,''), text,
                    COALESCE(like_count,0), COALESCE(impression_count,0),
                    last_source, first_discovered_at
             FROM discovered_posts WHERE 1=1"
        );
        // Build dynamic WHERE clauses — params are positional
        let mut param_idx = 1usize;
        let topic_idx = if topic.is_some() {
            sql.push_str(&format!(" AND text LIKE '%' || ?{param_idx} || '%'"));
            let idx = param_idx; param_idx += 1; Some(idx)
        } else { None };
        let author_idx = if author.is_some() {
            sql.push_str(&format!(" AND author_username LIKE '%' || ?{param_idx} || '%'"));
            let idx = param_idx; param_idx += 1; Some(idx)
        } else { None };
        let likes_idx = if min_likes.is_some() {
            sql.push_str(&format!(" AND like_count >= ?{param_idx}"));
            let idx = param_idx; param_idx += 1; Some(idx)
        } else { None };
        sql.push_str(&format!(" ORDER BY COALESCE(impression_count,0) DESC LIMIT ?{param_idx}"));

        let mut stmt = self.conn.prepare(&sql)?;
        let mut bind_idx = 1usize;
        if let Some(_) = topic_idx { stmt.raw_bind_parameter(bind_idx, topic.unwrap())?; bind_idx += 1; }
        if let Some(_) = author_idx { stmt.raw_bind_parameter(bind_idx, author.unwrap())?; bind_idx += 1; }
        if let Some(_) = likes_idx { stmt.raw_bind_parameter(bind_idx, min_likes.unwrap())?; bind_idx += 1; }
        stmt.raw_bind_parameter(bind_idx, limit as i64)?;

        let mut rows = Vec::new();
        let mut raw = stmt.raw_query();
        while let Some(row) = raw.next()? {
            rows.push(DiscoveredPostRow {
                tweet_id: row.get(0)?,
                author_username: row.get(1)?,
                text: row.get(2)?,
                like_count: row.get(3)?,
                impression_count: row.get(4)?,
                last_source: row.get(5)?,
                first_discovered_at: row.get(6)?,
            });
        }
        Ok(rows)
    }

    /// Count total posts in the discovered library.
    pub fn discovered_posts_count(&self) -> Result<i64, rusqlite::Error> {
        self.conn.query_row("SELECT COUNT(*) FROM discovered_posts", [], |r| r.get(0))
    }

    // -- metric snapshots -----------------------------------------------------

    /// Record a metric snapshot for a tweet. `url_clicks` is `Option<i64>` because
    /// the value comes from `non_public_metrics` which is only visible for posts you own;
    /// callers pass `None` when the data is unavailable.
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
        url_clicks: Option<i64>,
    ) -> Result<(), rusqlite::Error> {
        let snapshot_at = Utc::now().timestamp();
        self.conn.execute(
            "INSERT INTO metric_snapshots
                (tweet_id, snapshot_at, minutes_since_post, likes, retweets, replies,
                 impressions, bookmarks, quotes, profile_clicks, url_clicks)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
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
                url_clicks,
            ],
        )?;
        Ok(())
    }

    /// Fetch the most recent metric snapshot for a tweet with all counters + timestamps.
    /// Returns None if no snapshot exists yet for this tweet_id.
    ///
    /// Fetch the most recent metric snapshot for a tweet with all counters + timestamps.
    /// Returns None if no snapshot exists yet for this tweet_id.
    pub fn latest_snapshot_full(
        &self,
        tweet_id: &str,
    ) -> Result<Option<FullSnapshot>, rusqlite::Error> {
        self.conn
            .query_row(
                "SELECT snapshot_at, minutes_since_post,
                        likes, retweets, replies,
                        impressions, bookmarks, quotes, profile_clicks
                 FROM metric_snapshots
                 WHERE tweet_id = ?1
                 ORDER BY snapshot_at DESC
                 LIMIT 1",
                params![tweet_id],
                |row| {
                    Ok(FullSnapshot {
                        snapshot_at: row.get(0)?,
                        minutes_since_post: row.get(1)?,
                        likes: row.get(2)?,
                        retweets: row.get(3)?,
                        replies: row.get(4)?,
                        impressions: row.get(5)?,
                        bookmarks: row.get(6)?,
                        quotes: row.get(7)?,
                        profile_clicks: row.get(8)?,
                    })
                },
            )
            .optional()
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
                        (likes + retweets + replies) as f64 / impressions as f64
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
                         THEN (ms.likes + ms.retweets + ms.replies + ms.quotes) * 1.0
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
                             THEN (ms.likes+ms.retweets+ms.replies+ms.quotes)*1.0
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
                             THEN (ms.likes+ms.retweets+ms.replies+ms.quotes)*1.0
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
        // Standalone = everything except replies. Threads count (thread_hook
        // and thread_reply are both original-authored feed items). Replies to
        // other people's posts are excluded — the 2026 "2-4/day" cap applies
        // only to new impressions in the main feed.
        let standalone_24h: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM posts
             WHERE posted_at > ?1 - 86400
               AND content_type != 'reply'",
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
            standalone_24h,
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
                         THEN (ms.likes+ms.retweets+ms.replies+ms.quotes)*1.0
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
                         THEN (ms.likes+ms.retweets+ms.replies+ms.quotes)*1.0
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

    /// Find reply targets that deserve promotion to watchlist based on recent reply outcomes.
    ///
    /// Selection criteria (per-row OR): impressions ≥ min, profile_clicks ≥ min, or got_reply_back = 1.
    /// Guardrail: target_followers ≥ min_target_followers.
    /// Freshness: only considers replies in the last `max_age_hours`.
    /// Excludes targets already on the watchlist.
    ///
    /// Find reply targets that deserve promotion to watchlist based on recent reply outcomes.
    ///
    /// Same TEXT-affinity caveat as `rank_hot_reply_targets` — CAST guards
    /// on `performed_at` remain until a full table rebuild migration.
    pub fn find_hot_reply_targets(
        &self,
        min_impressions: i64,
        min_profile_clicks: i64,
        min_target_followers: i64,
        max_age_hours: i64,
    ) -> Result<Vec<HotReplyTarget>, rusqlite::Error> {
        let cutoff = Utc::now().timestamp() - max_age_hours * 3600;
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT LOWER(ea.target_username),
                    ea.target_user_id,
                    COALESCE(ea.target_followers, 0)
             FROM engagement_actions ea
             LEFT JOIN metric_snapshots ms
               ON ms.tweet_id = ea.reply_tweet_id
              AND ms.id = (
                  SELECT MAX(ms2.id) FROM metric_snapshots ms2
                  WHERE ms2.tweet_id = ea.reply_tweet_id
              )
             LEFT JOIN watchlist_accounts wa
               ON wa.username = LOWER(ea.target_username)
             WHERE ea.action_type = 'reply'
               AND ea.target_username IS NOT NULL
               AND ea.reply_tweet_id IS NOT NULL
               AND CAST(ea.performed_at AS INTEGER) >= ?1
               AND COALESCE(ea.target_followers, 0) >= ?2
               AND wa.username IS NULL
               AND (COALESCE(ms.impressions, 0) >= ?3
                 OR COALESCE(ms.profile_clicks, 0) >= ?4
                 OR COALESCE(ea.got_reply_back, 0) = 1)
             ORDER BY COALESCE(ea.target_followers, 0) DESC"
        )?;
        let rows = stmt.query_map(
            params![cutoff, min_target_followers, min_impressions, min_profile_clicks],
            |row| {
                Ok(HotReplyTarget {
                    username: row.get(0)?,
                    user_id: row.get(1)?,
                    target_followers: row.get(2)?,
                })
            },
        )?;
        rows.collect()
    }

    /// Aggregate reply-outcome stats per target username over the last `days`.
    ///
    /// Filters by min_samples, min_avg_impressions, min_avg_profile_clicks (HAVING).
    /// Returns rows sorted by composite score descending. Caller can re-sort.
    ///
    /// Aggregate reply-outcome stats per target username over the last `days`.
    ///
    /// NOTE: `performed_at` still has TEXT affinity in production DBs (the column
    /// was declared TEXT in the original tracker.rs CREATE TABLE and SQLite's
    /// CAST-in-UPDATE trick doesn't fix cell types when the declared affinity
    /// disagrees). We must keep CAST in comparisons and MAX() until a full table
    /// rebuild migration lands.
    pub fn rank_hot_reply_targets(
        &self,
        days: i64,
        min_samples: i64,
        min_avg_impressions: f64,
        min_avg_profile_clicks: f64,
    ) -> Result<Vec<HotTargetStats>, rusqlite::Error> {
        let cutoff = Utc::now().timestamp() - days * 24 * 3600;
        let mut stmt = self.conn.prepare(
            "SELECT LOWER(ea.target_username) AS username,
                    COUNT(*) AS sample_count,
                    AVG(COALESCE(ms.impressions, 0)) AS avg_imps,
                    AVG(COALESCE(ms.profile_clicks, 0)) AS avg_clicks,
                    AVG(CASE WHEN ea.got_reply_back = 1 THEN 1.0 ELSE 0.0 END) AS reply_back_rate,
                    CAST(MAX(CAST(ea.performed_at AS INTEGER)) AS INTEGER) AS last_reply_at
             FROM engagement_actions ea
             LEFT JOIN metric_snapshots ms
               ON ms.tweet_id = ea.reply_tweet_id
              AND ms.id = (
                  SELECT MAX(ms2.id) FROM metric_snapshots ms2
                  WHERE ms2.tweet_id = ea.reply_tweet_id
              )
             WHERE ea.action_type = 'reply'
               AND ea.target_username IS NOT NULL
               AND ea.reply_tweet_id IS NOT NULL
               AND CAST(ea.performed_at AS INTEGER) >= ?1
             GROUP BY LOWER(ea.target_username)
             HAVING sample_count >= ?2
                AND avg_imps >= ?3
                AND avg_clicks >= ?4"
        )?;
        let mut rows: Vec<HotTargetStats> = stmt
            .query_map(
                params![cutoff, min_samples, min_avg_impressions, min_avg_profile_clicks],
                |row| {
                    let avg_imps: f64 = row.get(2)?;
                    let avg_clicks: f64 = row.get(3)?;
                    let reply_back_rate: f64 = row.get(4)?;
                    // Composite score: normalized blend of reach + profile clicks + reciprocity.
                    // Normalization caps avoid one huge outlier dominating.
                    let imps_norm = (avg_imps / 1000.0).min(1.0);
                    let clicks_norm = (avg_clicks / 10.0).min(1.0);
                    let score = 0.55 * imps_norm + 0.25 * clicks_norm + 0.20 * reply_back_rate;
                    Ok(HotTargetStats {
                        username: row.get(0)?,
                        sample_count: row.get(1)?,
                        avg_impressions: avg_imps,
                        avg_profile_clicks: avg_clicks,
                        reply_back_rate,
                        last_reply_at: row.get(5)?,
                        score,
                    })
                },
            )?
            .collect::<Result<Vec<_>, _>>()?;
        rows.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        Ok(rows)
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
        let db_path = dir.path().join("test.db");
        let store = IntelStore::open_at(&db_path).unwrap();
        // Keep tempdir alive by leaking it (tests are short-lived)
        std::mem::forget(dir);
        store
    }

    #[test]
    fn log_and_retrieve_post() {
        let store = test_store();
        store
            .log_post("tweet_001", "Hello world!", "opinion", None, None, Some(85.0), None, None)
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
            .log_post("tweet_dup", "First", "opinion", None, None, None, None, None)
            .unwrap();
        // INSERT OR IGNORE — second insert should not fail or create duplicate
        store
            .log_post("tweet_dup", "Second", "opinion", None, None, None, None, None)
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
        assert_eq!(velocity.standalone_24h, 0);
        assert!(velocity.accelerating_post.is_none());
    }

    #[test]
    fn standalone_24h_excludes_replies() {
        let store = test_store();
        store
            .log_post("p_1", "a standalone post", "text", None, None, None, None, None)
            .unwrap();
        store
            .log_post("p_2", "reply to something", "reply", Some("other_id"), None, None, None, None)
            .unwrap();
        store
            .log_post("p_3", "thread opener", "thread_hook", None, None, None, None, None)
            .unwrap();
        let v = store.get_recent_post_velocity().unwrap();
        assert_eq!(v.posts_24h, 3, "all three rows count toward posts_24h");
        assert_eq!(v.standalone_24h, 2, "reply must be excluded from standalone_24h");
    }

    #[test]
    fn metric_snapshot_persists_url_clicks() {
        let store = test_store();
        store
            .log_post("tweet_url", "Check this link", "text", None, None, None, None, None)
            .unwrap();
        store
            .log_metric_snapshot("tweet_url", 5, 1, 0, 200, 0, 0, 3, 10, Some(42))
            .unwrap();
        let clicks: Option<i64> = store
            .conn
            .query_row(
                "SELECT url_clicks FROM metric_snapshots WHERE tweet_id = 'tweet_url'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(clicks, Some(42));
    }

    #[test]
    fn metric_snapshot_persists_url_clicks_null_when_none() {
        let store = test_store();
        store
            .log_post("tweet_null", "no link", "text", None, None, None, None, None)
            .unwrap();
        store
            .log_metric_snapshot("tweet_null", 1, 0, 0, 10, 0, 0, 0, 5, None)
            .unwrap();
        let clicks: Option<i64> = store
            .conn
            .query_row(
                "SELECT url_clicks FROM metric_snapshots WHERE tweet_id = 'tweet_null'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(clicks, None);
    }

    #[test]
    fn metric_snapshot_and_retrieval() {
        let store = test_store();
        store
            .log_post("tweet_metrics", "Test post", "opinion", None, None, None, None, None)
            .unwrap();
        store
            .log_metric_snapshot("tweet_metrics", 10, 5, 3, 1000, 2, 1, 50, 60, None)
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

    #[test]
    fn classify_reply_question() {
        assert_eq!(classify_reply("What do you think about this?"), ReplyStyle::Question);
    }

    #[test]
    fn classify_reply_counterpoint() {
        assert_eq!(classify_reply("However, the evidence suggests otherwise"), ReplyStyle::Counterpoint);
    }

    #[test]
    fn classify_reply_anecdote() {
        assert_eq!(classify_reply("I've been using this protocol for 6 months"), ReplyStyle::Anecdote);
    }

    #[test]
    fn classify_reply_humor() {
        assert_eq!(classify_reply("lol that's incredible"), ReplyStyle::Humor);
    }

    #[test]
    fn classify_reply_agreement_fallback() {
        assert_eq!(classify_reply("Great insight, totally agree"), ReplyStyle::Agreement);
    }

    #[test]
    fn record_published_post_works() {
        let store = test_store();
        store
            .record_published_post(
                "tweet_pub_001",
                "Test published post",
                "text",
                None,
                None,
                Some(75.0),
                Some(r#"{"score":75}"#),
                Some("sched_123"),
            )
            .unwrap();

        let posts = store.get_post_history(10).unwrap();
        assert_eq!(posts.len(), 1);
        assert_eq!(posts[0].tweet_id, "tweet_pub_001");
    }

    #[test]
    fn log_reply_with_style() {
        let store = test_store();
        let style = classify_reply("What makes you think that?");
        store
            .log_reply("target_001", None, Some("testuser"), Some(5000), "reply_001", Some(&style))
            .unwrap();

        let pending = store.get_pending_replies(24).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].reply_tweet_id, "reply_001");
    }

    #[test]
    fn discovered_posts_upsert_and_query() {
        let store = test_store();
        let tweet = crate::providers::xapi::TweetData {
            id: "dp_123".into(),
            text: "Longevity research is the future".into(),
            author_id: Some("user1".into()),
            author_username: Some("testuser".into()),
            created_at: Some("2026-01-01T00:00:00Z".into()),
            conversation_id: None,
            referenced_tweets: None,
            public_metrics: Some(crate::providers::xapi::TweetMetrics {
                like_count: 42,
                retweet_count: 5,
                reply_count: 3,
                impression_count: 2000,
                bookmark_count: 1,
            }),
            author_followers: Some(500),
            media_urls: vec![],
        };
        // First insert
        store.record_discovered_post("search", &tweet).unwrap();
        assert_eq!(store.discovered_posts_count().unwrap(), 1);

        // Upsert: same tweet, different source — updates last_source, preserves first_source
        store.record_discovered_post("timeline", &tweet).unwrap();
        assert_eq!(store.discovered_posts_count().unwrap(), 1);
        let last: String = store.conn.query_row(
            "SELECT last_source FROM discovered_posts WHERE tweet_id = 'dp_123'", [], |r| r.get(0)
        ).unwrap();
        assert_eq!(last, "timeline");
        let first: String = store.conn.query_row(
            "SELECT first_source FROM discovered_posts WHERE tweet_id = 'dp_123'", [], |r| r.get(0)
        ).unwrap();
        assert_eq!(first, "search");

        // Query: by topic
        let results = store.query_discovered_posts(Some("longevity"), None, None, 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].tweet_id, "dp_123");

        // Query: by min_likes
        let results = store.query_discovered_posts(None, None, Some(100), 10).unwrap();
        assert_eq!(results.len(), 0); // 42 < 100
        let results = store.query_discovered_posts(None, None, Some(10), 10).unwrap();
        assert_eq!(results.len(), 1);

        // Query: by author
        let results = store.query_discovered_posts(None, Some("testuser"), None, 10).unwrap();
        assert_eq!(results.len(), 1);
    }

    // -- hot reply targets (items D + E) -------------------------------------

    /// Helper: log an outgoing reply-post + the matching engagement_action.
    /// Mirrors the production flow where `record_published_post` writes the post row.
    fn log_outgoing_reply(
        store: &IntelStore,
        target_tweet_id: &str,
        target_user_id: Option<&str>,
        target_username: &str,
        target_followers: i64,
        reply_tweet_id: &str,
    ) {
        store
            .log_post(reply_tweet_id, "reply body", "text", Some(target_tweet_id), None, None, None, None)
            .unwrap();
        store
            .log_reply(
                target_tweet_id,
                target_user_id,
                Some(target_username),
                Some(target_followers),
                reply_tweet_id,
                None,
            )
            .unwrap();
    }

    #[test]
    fn find_hot_reply_targets_returns_high_impression_reply() {
        let store = test_store();
        log_outgoing_reply(&store, "target_tweet_1", Some("uid_42"), "hottarget", 5000, "my_reply_1");
        store
            .log_metric_snapshot("my_reply_1", 10, 2, 1, 500, 0, 0, 3, 15, None)
            .unwrap();

        let promoted = store
            .find_hot_reply_targets(100, 1, 1_000, 24 * 14)
            .unwrap();
        assert_eq!(promoted.len(), 1);
        assert_eq!(promoted[0].username, "hottarget");
        assert_eq!(promoted[0].user_id.as_deref(), Some("uid_42"));
        assert_eq!(promoted[0].target_followers, 5000);
    }

    #[test]
    fn find_hot_reply_targets_skips_low_follower_targets() {
        let store = test_store();
        log_outgoing_reply(&store, "t1", None, "smallfry", 500, "r1");
        store
            .log_metric_snapshot("r1", 0, 0, 0, 1000, 0, 0, 5, 10, None)
            .unwrap();

        let promoted = store
            .find_hot_reply_targets(100, 1, 1_000, 24 * 14)
            .unwrap();
        assert!(promoted.is_empty(), "should filter target below min_target_followers");
    }

    #[test]
    fn find_hot_reply_targets_skips_already_watchlisted() {
        let store = test_store();
        store
            .add_watchlist("alreadyhere", Some("uid_1"), None, 10_000)
            .unwrap();
        log_outgoing_reply(&store, "t1", Some("uid_1"), "alreadyhere", 10_000, "r1");
        store
            .log_metric_snapshot("r1", 0, 0, 0, 1000, 0, 0, 5, 10, None)
            .unwrap();

        let promoted = store
            .find_hot_reply_targets(100, 1, 1_000, 24 * 14)
            .unwrap();
        assert!(promoted.is_empty(), "should not re-promote watchlist members");
    }

    #[test]
    fn find_hot_reply_targets_promotes_on_profile_clicks_alone() {
        let store = test_store();
        log_outgoing_reply(&store, "t1", None, "clickytarget", 2000, "r1");
        // Low impressions but 2 profile_clicks
        store
            .log_metric_snapshot("r1", 0, 0, 0, 10, 0, 0, 2, 5, None)
            .unwrap();

        let promoted = store
            .find_hot_reply_targets(100, 1, 1_000, 24 * 14)
            .unwrap();
        assert_eq!(promoted.len(), 1);
        assert_eq!(promoted[0].username, "clickytarget");
    }

    #[test]
    fn find_hot_reply_targets_promotes_on_reply_back() {
        let store = test_store();
        log_outgoing_reply(&store, "t1", None, "replier", 5000, "r1");
        store
            .log_metric_snapshot("r1", 0, 0, 0, 10, 0, 0, 0, 3, None)
            .unwrap();
        let action_id: i64 = store
            .conn
            .query_row(
                "SELECT id FROM engagement_actions WHERE reply_tweet_id = 'r1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        store.set_reply_back(action_id, true).unwrap();

        let promoted = store
            .find_hot_reply_targets(100, 1, 1_000, 24 * 14)
            .unwrap();
        assert_eq!(promoted.len(), 1);
        assert_eq!(promoted[0].username, "replier");
    }

    #[test]
    fn find_hot_reply_targets_skips_stale_replies() {
        let store = test_store();
        store
            .log_post("r1", "old reply body", "text", Some("t1"), None, None, None, None)
            .unwrap();
        let old_ts = chrono::Utc::now().timestamp() - 30 * 24 * 3600;
        store
            .conn
            .execute(
                "INSERT INTO engagement_actions
                    (action_type, target_tweet_id, target_user_id, target_username,
                     performed_at, target_followers, reply_tweet_id, reply_style)
                 VALUES ('reply', 't1', NULL, LOWER('oldtarget'), ?1, 5000, 'r1', NULL)",
                params![old_ts],
            )
            .unwrap();
        store
            .log_metric_snapshot("r1", 0, 0, 0, 10_000, 0, 0, 10, 30, None)
            .unwrap();

        let promoted = store
            .find_hot_reply_targets(100, 1, 1_000, 24 * 14)
            .unwrap();
        assert!(promoted.is_empty(), "should skip replies older than max_age_hours");
    }

    #[test]
    fn rank_hot_reply_targets_aggregates_by_username() {
        let store = test_store();
        log_outgoing_reply(&store, "t1", None, "target_a", 3000, "r1");
        store
            .log_metric_snapshot("r1", 0, 0, 0, 100, 0, 0, 1, 5, None)
            .unwrap();
        log_outgoing_reply(&store, "t2", None, "target_a", 3000, "r2");
        store
            .log_metric_snapshot("r2", 0, 0, 0, 300, 0, 0, 3, 5, None)
            .unwrap();

        let ranked = store.rank_hot_reply_targets(7, 1, 0.0, 0.0).unwrap();
        assert_eq!(ranked.len(), 1);
        assert_eq!(ranked[0].username, "target_a");
        assert_eq!(ranked[0].sample_count, 2);
        assert!((ranked[0].avg_impressions - 200.0).abs() < 0.01);
        assert!((ranked[0].avg_profile_clicks - 2.0).abs() < 0.01);
    }

    #[test]
    fn rank_hot_reply_targets_respects_min_samples() {
        let store = test_store();
        log_outgoing_reply(&store, "t1", None, "target_a", 3000, "r1");
        store
            .log_metric_snapshot("r1", 0, 0, 0, 1000, 0, 0, 5, 5, None)
            .unwrap();

        let ranked = store.rank_hot_reply_targets(7, 2, 0.0, 0.0).unwrap();
        assert!(ranked.is_empty(), "single sample should be filtered by min_samples=2");
    }

    #[test]
    fn rank_hot_reply_targets_computes_reply_back_rate() {
        let store = test_store();
        log_outgoing_reply(&store, "t1", None, "target_a", 3000, "r1");
        store
            .log_metric_snapshot("r1", 0, 0, 0, 100, 0, 0, 0, 5, None)
            .unwrap();
        let id1: i64 = store
            .conn
            .query_row(
                "SELECT id FROM engagement_actions WHERE reply_tweet_id = 'r1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        store.set_reply_back(id1, true).unwrap();

        log_outgoing_reply(&store, "t2", None, "target_a", 3000, "r2");
        store
            .log_metric_snapshot("r2", 0, 0, 0, 100, 0, 0, 0, 5, None)
            .unwrap();
        let id2: i64 = store
            .conn
            .query_row(
                "SELECT id FROM engagement_actions WHERE reply_tweet_id = 'r2'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        store.set_reply_back(id2, false).unwrap();

        let ranked = store.rank_hot_reply_targets(7, 1, 0.0, 0.0).unwrap();
        assert_eq!(ranked.len(), 1);
        assert!((ranked[0].reply_back_rate - 0.5).abs() < 0.01);
    }

    #[test]
    fn hot_reply_targets_end_to_end_promotion_loop() {
        let store = test_store();
        log_outgoing_reply(&store, "t1", Some("uid_new"), "newhot", 5000, "r1");
        store
            .log_metric_snapshot("r1", 0, 0, 0, 500, 0, 0, 3, 15, None)
            .unwrap();

        let hot = store
            .find_hot_reply_targets(100, 1, 1_000, 24 * 14)
            .unwrap();
        assert_eq!(hot.len(), 1);

        for row in &hot {
            store
                .add_watchlist(&row.username, row.user_id.as_deref(), None, row.target_followers)
                .unwrap();
        }

        let wl = store.list_watchlist().unwrap();
        assert_eq!(wl.len(), 1);
        assert_eq!(wl[0].username, "newhot");
        assert_eq!(wl[0].user_id.as_deref(), Some("uid_new"));
        assert_eq!(wl[0].followers, 5000);
        assert!(wl[0].topic.is_none(), "auto-promotion should preserve NULL topic");

        // Second call should skip because target is now watchlisted
        let hot_again = store
            .find_hot_reply_targets(100, 1, 1_000, 24 * 14)
            .unwrap();
        assert!(hot_again.is_empty());
    }

    #[test]
    fn rank_hot_reply_targets_applies_days_filter() {
        let store = test_store();
        log_outgoing_reply(&store, "t1", None, "fresh_target", 3000, "r1");
        store
            .log_metric_snapshot("r1", 0, 0, 0, 1000, 0, 0, 5, 5, None)
            .unwrap();
        store
            .log_post("r2", "stale reply body", "text", Some("t2"), None, None, None, None)
            .unwrap();
        let old_ts = chrono::Utc::now().timestamp() - 20 * 24 * 3600;
        store
            .conn
            .execute(
                "INSERT INTO engagement_actions
                    (action_type, target_tweet_id, target_user_id, target_username,
                     performed_at, target_followers, reply_tweet_id, reply_style)
                 VALUES ('reply', 't2', NULL, LOWER('stale_target'), ?1, 3000, 'r2', NULL)",
                params![old_ts],
            )
            .unwrap();
        store
            .log_metric_snapshot("r2", 0, 0, 0, 1000, 0, 0, 5, 5, None)
            .unwrap();

        let ranked = store.rank_hot_reply_targets(7, 1, 0.0, 0.0).unwrap();
        assert_eq!(ranked.len(), 1);
        assert_eq!(ranked[0].username, "fresh_target");
    }
}
