use chrono::Utc;
use rusqlite::{params, Connection};
use serde::Serialize;
use std::path::PathBuf;

use crate::config::config_dir;
use crate::errors::XmasterError;
use crate::providers::xapi::TweetData;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct BookmarkRecord {
    #[serde(rename = "id")]
    pub tweet_id: String,
    #[serde(rename = "author")]
    pub author_username: String,
    pub author_name: Option<String>,
    pub text: String,
    #[serde(rename = "date")]
    pub created_at: Option<String>,
    pub bookmarked_at: i64,
    pub likes: i64,
    pub retweets: i64,
    pub replies: i64,
    pub has_media: bool,
    pub has_link: bool,
    pub tags: String,
    pub notes: String,
    pub read: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct SyncResult {
    pub new_bookmarks: u32,
    pub already_stored: u32,
    pub total_in_db: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct BookmarkStats {
    pub total: u32,
    pub unread: u32,
    pub with_links: u32,
    pub with_media: u32,
    pub top_authors: Vec<(String, u32)>,
    pub oldest: Option<String>,
    pub newest: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BookmarkDigest {
    pub period_days: u32,
    pub count: u32,
    pub by_author: Vec<AuthorGroup>,
    pub link_count: u32,
    pub text_count: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct AuthorGroup {
    pub username: String,
    pub count: u32,
    pub bookmarks: Vec<BookmarkRecord>,
}

// ---------------------------------------------------------------------------
// BookmarkStore
// ---------------------------------------------------------------------------

pub struct BookmarkStore {
    conn: Connection,
}

impl BookmarkStore {
    /// Open (or create) the bookmark database at `~/.config/xmaster/bookmarks.db`.
    pub fn open() -> Result<Self, XmasterError> {
        let dir = config_dir();
        std::fs::create_dir_all(&dir).ok();
        let db_path: PathBuf = dir.join("bookmarks.db");
        let conn = Connection::open(db_path).map_err(|e| {
            XmasterError::Config(format!("Failed to open bookmark database: {e}"))
        })?;
        conn.pragma_update(None, "journal_mode", "wal").ok();
        conn.pragma_update(None, "busy_timeout", 5000).ok();
        conn.pragma_update(None, "synchronous", "NORMAL").ok();
        let store = Self { conn };
        store.init_tables()?;
        Ok(store)
    }

    fn init_tables(&self) -> Result<(), XmasterError> {
        self.conn
            .execute_batch(
                "
            CREATE TABLE IF NOT EXISTS bookmarks (
                tweet_id TEXT PRIMARY KEY,
                author_username TEXT NOT NULL,
                author_name TEXT,
                text TEXT NOT NULL,
                created_at TEXT,
                bookmarked_at INTEGER NOT NULL,
                likes INTEGER DEFAULT 0,
                retweets INTEGER DEFAULT 0,
                replies INTEGER DEFAULT 0,
                impressions INTEGER DEFAULT 0,
                has_media INTEGER DEFAULT 0,
                has_link INTEGER DEFAULT 0,
                tags TEXT DEFAULT '',
                notes TEXT DEFAULT '',
                read INTEGER DEFAULT 0,
                exported INTEGER DEFAULT 0
            );
            CREATE INDEX IF NOT EXISTS idx_bookmarks_date ON bookmarks(bookmarked_at);
            CREATE INDEX IF NOT EXISTS idx_bookmarks_author ON bookmarks(author_username);
            CREATE INDEX IF NOT EXISTS idx_bookmarks_read ON bookmarks(read);

            CREATE TABLE IF NOT EXISTS bookmark_sync_log (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                synced_at INTEGER NOT NULL,
                new_count INTEGER NOT NULL,
                total_count INTEGER NOT NULL
            );
            ",
            )
            .map_err(|e| XmasterError::Config(format!("Failed to init bookmark tables: {e}")))?;
        Ok(())
    }

    // -- sync ----------------------------------------------------------------

    /// Ingest bookmarks fetched from the X API. Idempotent — duplicates are ignored.
    pub fn sync(&self, xapi_bookmarks: Vec<TweetData>) -> Result<SyncResult, XmasterError> {
        let now = Utc::now().timestamp();
        let mut new_count: u32 = 0;

        for tweet in &xapi_bookmarks {
            let author = tweet
                .author_username
                .as_deref()
                .unwrap_or("unknown");
            let has_link =
                tweet.text.contains("http://") || tweet.text.contains("https://");
            // X API v2 doesn't include media info in basic tweet fields; detect via t.co links
            // and entities if present. For now, mark false — can be refined later.
            let has_media = false;

            let (likes, retweets, replies, impressions) = match &tweet.public_metrics {
                Some(m) => (
                    m.like_count as i64,
                    m.retweet_count as i64,
                    m.reply_count as i64,
                    m.impression_count as i64,
                ),
                None => (0, 0, 0, 0),
            };

            let changed = self
                .conn
                .execute(
                    "INSERT OR IGNORE INTO bookmarks
                    (tweet_id, author_username, author_name, text, created_at,
                     bookmarked_at, likes, retweets, replies, impressions,
                     has_media, has_link)
                 VALUES (?1,?2,NULL,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
                    params![
                        tweet.id,
                        author,
                        tweet.text,
                        tweet.created_at,
                        now,
                        likes,
                        retweets,
                        replies,
                        impressions,
                        has_media,
                        has_link,
                    ],
                )
                .map_err(|e| XmasterError::Config(format!("Bookmark insert failed: {e}")))?;

            if changed > 0 {
                new_count += 1;
            }
        }

        let already_stored = xapi_bookmarks.len() as u32 - new_count;

        let total_in_db: u32 = self
            .conn
            .query_row("SELECT COUNT(*) FROM bookmarks", [], |r| r.get(0))
            .map_err(|e| XmasterError::Config(format!("Bookmark count query failed: {e}")))?;

        // Log sync event
        self.conn
            .execute(
                "INSERT INTO bookmark_sync_log (synced_at, new_count, total_count) VALUES (?1,?2,?3)",
                params![now, new_count, total_in_db],
            )
            .map_err(|e| XmasterError::Config(format!("Sync log insert failed: {e}")))?;

        Ok(SyncResult {
            new_bookmarks: new_count,
            already_stored,
            total_in_db,
        })
    }

    // -- search / read -------------------------------------------------------

    /// Full-text search across text, author_username, author_name, tags, notes.
    pub fn search(&self, query: &str) -> Result<Vec<BookmarkRecord>, XmasterError> {
        let pattern = format!("%{query}%");
        let mut stmt = self
            .conn
            .prepare(
                "SELECT tweet_id, author_username, author_name, text, created_at,
                        bookmarked_at, likes, retweets, replies, has_media, has_link,
                        tags, notes, read
                 FROM bookmarks
                 WHERE text LIKE ?1 OR author_username LIKE ?1 OR author_name LIKE ?1
                       OR tags LIKE ?1 OR notes LIKE ?1
                 ORDER BY bookmarked_at DESC",
            )
            .map_err(|e| XmasterError::Config(format!("Search prepare failed: {e}")))?;

        let rows = stmt
            .query_map(params![pattern], Self::row_to_record)
            .map_err(|e| XmasterError::Config(format!("Search query failed: {e}")))?;

        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| XmasterError::Config(format!("Search collect failed: {e}")))
    }

    /// Unread bookmarks, newest first.
    pub fn list_unread(&self, limit: usize) -> Result<Vec<BookmarkRecord>, XmasterError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT tweet_id, author_username, author_name, text, created_at,
                        bookmarked_at, likes, retweets, replies, has_media, has_link,
                        tags, notes, read
                 FROM bookmarks
                 WHERE read = 0
                 ORDER BY bookmarked_at DESC
                 LIMIT ?1",
            )
            .map_err(|e| XmasterError::Config(format!("Unread prepare failed: {e}")))?;

        let rows = stmt
            .query_map(params![limit as i64], Self::row_to_record)
            .map_err(|e| XmasterError::Config(format!("Unread query failed: {e}")))?;

        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| XmasterError::Config(format!("Unread collect failed: {e}")))
    }

    // -- mutations ------------------------------------------------------------

    pub fn mark_read(&self, tweet_id: &str) -> Result<(), XmasterError> {
        self.conn
            .execute(
                "UPDATE bookmarks SET read = 1 WHERE tweet_id = ?1",
                params![tweet_id],
            )
            .map_err(|e| XmasterError::Config(format!("Mark read failed: {e}")))?;
        Ok(())
    }

    /// Append a tag to the comma-separated tags field.
    pub fn tag(&self, tweet_id: &str, tag: &str) -> Result<(), XmasterError> {
        // Read current tags, append, write back
        let current: String = self
            .conn
            .query_row(
                "SELECT tags FROM bookmarks WHERE tweet_id = ?1",
                params![tweet_id],
                |r| r.get(0),
            )
            .map_err(|e| XmasterError::Config(format!("Tag read failed: {e}")))?;

        let new_tags = if current.is_empty() {
            tag.to_string()
        } else {
            // Don't add duplicate tags
            let existing: Vec<&str> = current.split(',').map(|t| t.trim()).collect();
            if existing.contains(&tag) {
                return Ok(());
            }
            format!("{current},{tag}")
        };

        self.conn
            .execute(
                "UPDATE bookmarks SET tags = ?1 WHERE tweet_id = ?2",
                params![new_tags, tweet_id],
            )
            .map_err(|e| XmasterError::Config(format!("Tag update failed: {e}")))?;
        Ok(())
    }

    // -- export ---------------------------------------------------------------

    /// Generate markdown formatted export.
    pub fn export_markdown(bookmarks: &[BookmarkRecord]) -> String {
        let date = Utc::now().format("%Y-%m-%d");
        let mut md = format!("# X Bookmarks Export ({date})\n\n");

        for bm in bookmarks {
            let preview: String = bm.text.chars().take(80).collect();
            let preview = preview.replace('\n', " ");
            md.push_str(&format!("## @{} — \"{preview}...\"\n", bm.author_username));
            let saved_date = chrono::DateTime::from_timestamp(bm.bookmarked_at, 0)
                .map(|dt| dt.format("%Y-%m-%d").to_string())
                .unwrap_or_else(|| bm.bookmarked_at.to_string());
            md.push_str(&format!("- Saved: {saved_date}\n"));
            md.push_str(&format!(
                "- Engagement: {} likes, {} retweets\n",
                bm.likes, bm.retweets
            ));
            md.push_str(&format!(
                "- Link: https://x.com/{}/status/{}\n",
                bm.author_username, bm.tweet_id
            ));
            if !bm.tags.is_empty() {
                md.push_str(&format!("- Tags: {}\n", bm.tags));
            }
            md.push_str("---\n\n");
        }

        md
    }

    // -- stats / digest -------------------------------------------------------

    pub fn get_stats(&self) -> Result<BookmarkStats, XmasterError> {
        let total: u32 = self
            .conn
            .query_row("SELECT COUNT(*) FROM bookmarks", [], |r| r.get(0))
            .map_err(|e| XmasterError::Config(format!("Stats total failed: {e}")))?;

        let unread: u32 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM bookmarks WHERE read = 0",
                [],
                |r| r.get(0),
            )
            .map_err(|e| XmasterError::Config(format!("Stats unread failed: {e}")))?;

        let with_links: u32 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM bookmarks WHERE has_link = 1",
                [],
                |r| r.get(0),
            )
            .map_err(|e| XmasterError::Config(format!("Stats links failed: {e}")))?;

        let with_media: u32 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM bookmarks WHERE has_media = 1",
                [],
                |r| r.get(0),
            )
            .map_err(|e| XmasterError::Config(format!("Stats media failed: {e}")))?;

        let mut author_stmt = self
            .conn
            .prepare(
                "SELECT author_username, COUNT(*) as cnt FROM bookmarks
                 GROUP BY author_username ORDER BY cnt DESC LIMIT 10",
            )
            .map_err(|e| XmasterError::Config(format!("Stats authors prepare failed: {e}")))?;

        let top_authors: Vec<(String, u32)> = author_stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(|e| XmasterError::Config(format!("Stats authors query failed: {e}")))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| XmasterError::Config(format!("Stats authors collect failed: {e}")))?;

        let oldest: Option<String> = self
            .conn
            .query_row(
                "SELECT bookmarked_at FROM bookmarks ORDER BY bookmarked_at ASC LIMIT 1",
                [],
                |r| {
                    let ts: i64 = r.get(0)?;
                    Ok(chrono::DateTime::from_timestamp(ts, 0)
                        .map(|dt| dt.to_rfc3339())
                        .unwrap_or_else(|| ts.to_string()))
                },
            )
            .ok();

        let newest: Option<String> = self
            .conn
            .query_row(
                "SELECT bookmarked_at FROM bookmarks ORDER BY bookmarked_at DESC LIMIT 1",
                [],
                |r| {
                    let ts: i64 = r.get(0)?;
                    Ok(chrono::DateTime::from_timestamp(ts, 0)
                        .map(|dt| dt.to_rfc3339())
                        .unwrap_or_else(|| ts.to_string()))
                },
            )
            .ok();

        Ok(BookmarkStats {
            total,
            unread,
            with_links,
            with_media,
            top_authors,
            oldest,
            newest,
        })
    }

    /// Bookmarks from the last N days, grouped by author.
    pub fn get_digest(&self, days: u32) -> Result<BookmarkDigest, XmasterError> {
        let cutoff = Utc::now().timestamp() - (days as i64 * 86400);

        let mut stmt = self
            .conn
            .prepare(
                "SELECT tweet_id, author_username, author_name, text, created_at,
                        bookmarked_at, likes, retweets, replies, has_media, has_link,
                        tags, notes, read
                 FROM bookmarks
                 WHERE bookmarked_at > ?1
                 ORDER BY author_username, bookmarked_at DESC",
            )
            .map_err(|e| XmasterError::Config(format!("Digest prepare failed: {e}")))?;

        let rows: Vec<BookmarkRecord> = stmt
            .query_map(params![cutoff], Self::row_to_record)
            .map_err(|e| XmasterError::Config(format!("Digest query failed: {e}")))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| XmasterError::Config(format!("Digest collect failed: {e}")))?;

        let count = rows.len() as u32;
        let link_count = rows.iter().filter(|b| b.has_link).count() as u32;
        let text_count = count - link_count;

        // Group by author
        let mut groups: Vec<AuthorGroup> = Vec::new();
        for bm in rows {
            if let Some(group) = groups.iter_mut().find(|g| g.username == bm.author_username) {
                group.count += 1;
                group.bookmarks.push(bm);
            } else {
                let username = bm.author_username.clone();
                groups.push(AuthorGroup {
                    username,
                    count: 1,
                    bookmarks: vec![bm],
                });
            }
        }

        groups.sort_by_key(|group| std::cmp::Reverse(group.count));

        Ok(BookmarkDigest {
            period_days: days,
            count,
            by_author: groups,
            link_count,
            text_count,
        })
    }

    // -- helpers --------------------------------------------------------------

    fn row_to_record(row: &rusqlite::Row) -> rusqlite::Result<BookmarkRecord> {
        Ok(BookmarkRecord {
            tweet_id: row.get(0)?,
            author_username: row.get(1)?,
            author_name: row.get(2)?,
            text: row.get(3)?,
            created_at: row.get(4)?,
            bookmarked_at: row.get(5)?,
            likes: row.get(6)?,
            retweets: row.get(7)?,
            replies: row.get(8)?,
            has_media: row.get::<_, i64>(9)? != 0,
            has_link: row.get::<_, i64>(10)? != 0,
            tags: row.get(11)?,
            notes: row.get(12)?,
            read: row.get::<_, i64>(13)? != 0,
        })
    }
}
