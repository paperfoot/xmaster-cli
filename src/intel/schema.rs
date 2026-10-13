//! The one schema for xmaster's two SQLite files, versioned with
//! `PRAGMA user_version`.
//!
//! Every store opens its file through [`open`]. When `user_version` already
//! matches [`Schema::version`], opening costs one pragma read: no DDL and no
//! write lock. Otherwise [`migrate`] brings the file up to date in a single
//! `BEGIN IMMEDIATE` transaction:
//!
//! 1. create missing tables;
//! 2. rebuild any table that lacks a current column or declares an epoch
//!    column with non-INTEGER affinity. Early releases declared
//!    `posts.posted_at`, `metric_snapshots.snapshot_at`,
//!    `engagement_actions.performed_at`, `timing_stats.last_updated`,
//!    `bookmarks.bookmarked_at` and `bookmark_sync_log.synced_at` as TEXT, so
//!    epochs were stored as text (some as ISO-8601 strings) and every
//!    `row.get::<i64>` on them failed. SQLite can't change a column's
//!    affinity in place, so the table is copied: create `<table>__rebuild`
//!    from the current DDL, `INSERT … SELECT` with each epoch converted, drop
//!    the old table, rename;
//! 3. convert stray text epochs left in INTEGER columns;
//! 4. recreate indexes and views, then stamp `user_version`.
//!
//! Every step is idempotent, so a failed run rolls back whole and is retried
//! on the next open. Through [`open`] a failure is logged, never fatal.

use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};

use crate::errors::XmasterError;

pub(crate) struct Table {
    pub name: &'static str,
    /// Column definitions: the body of `CREATE TABLE name ( … )`.
    pub columns: &'static str,
    /// Unix-epoch columns. They must hold INTEGERs.
    pub epochs: &'static [&'static str],
}

pub(crate) struct Schema {
    pub file: &'static str,
    /// Bump whenever a table or `finish` changes; `migrate` re-runs on the
    /// next open of every existing file.
    pub version: i32,
    pub tables: &'static [Table],
    /// Indexes and views, run once the tables are current.
    pub finish: &'static str,
}

pub(crate) const XMASTER: Schema = Schema {
    file: "xmaster.db",
    version: 1,
    tables: &[
        Table {
            name: "posts",
            columns: "
                id                INTEGER PRIMARY KEY AUTOINCREMENT,
                tweet_id          TEXT UNIQUE NOT NULL,
                text              TEXT NOT NULL,
                content_type      TEXT NOT NULL DEFAULT 'text',
                char_count        INTEGER NOT NULL DEFAULT 0,
                has_link          INTEGER NOT NULL DEFAULT 0,
                has_media         INTEGER NOT NULL DEFAULT 0,
                has_poll          INTEGER NOT NULL DEFAULT 0,
                hashtag_count     INTEGER NOT NULL DEFAULT 0,
                hook_text         TEXT,
                posted_at         INTEGER NOT NULL,
                day_of_week       INTEGER NOT NULL DEFAULT 0,
                hour_of_day       INTEGER NOT NULL DEFAULT 0,
                reply_to_id       TEXT,
                quote_of_id       TEXT,
                preflight_score   REAL,
                local_day_of_week INTEGER,
                local_hour_of_day INTEGER,
                tz_offset_minutes INTEGER,
                analysis_json     TEXT,
                analysis_version  INTEGER DEFAULT 1,
                scheduled_post_id TEXT",
            epochs: &["posted_at"],
        },
        Table {
            name: "metric_snapshots",
            columns: "
                id                 INTEGER PRIMARY KEY AUTOINCREMENT,
                tweet_id           TEXT NOT NULL,
                snapshot_at        INTEGER NOT NULL,
                minutes_since_post INTEGER NOT NULL DEFAULT 0,
                likes              INTEGER NOT NULL DEFAULT 0,
                retweets           INTEGER NOT NULL DEFAULT 0,
                replies            INTEGER NOT NULL DEFAULT 0,
                impressions        INTEGER NOT NULL DEFAULT 0,
                bookmarks          INTEGER NOT NULL DEFAULT 0,
                quotes             INTEGER NOT NULL DEFAULT 0,
                profile_clicks     INTEGER NOT NULL DEFAULT 0,
                url_clicks         INTEGER",
            epochs: &["snapshot_at"],
        },
        Table {
            name: "engagement_actions",
            columns: "
                id               INTEGER PRIMARY KEY AUTOINCREMENT,
                action_type      TEXT NOT NULL,
                target_tweet_id  TEXT,
                target_user_id   TEXT,
                target_username  TEXT,
                performed_at     INTEGER NOT NULL,
                got_reply_back   INTEGER DEFAULT NULL,
                target_followers INTEGER,
                reply_tweet_id   TEXT,
                reply_style      TEXT,
                reply_checked_at INTEGER",
            epochs: &["performed_at", "reply_checked_at"],
        },
        Table {
            name: "watchlist_accounts",
            columns: "
                id        INTEGER PRIMARY KEY AUTOINCREMENT,
                username  TEXT UNIQUE NOT NULL,
                user_id   TEXT,
                topic     TEXT,
                followers INTEGER NOT NULL DEFAULT 0,
                added_at  INTEGER NOT NULL",
            epochs: &["added_at"],
        },
        Table {
            name: "timing_stats",
            columns: "
                id                  INTEGER PRIMARY KEY AUTOINCREMENT,
                day_of_week         INTEGER NOT NULL,
                hour_of_day         INTEGER NOT NULL,
                content_type        TEXT NOT NULL DEFAULT 'all',
                avg_impressions     REAL,
                avg_engagement_rate REAL,
                sample_count        INTEGER NOT NULL DEFAULT 0,
                last_updated        INTEGER NOT NULL",
            epochs: &["last_updated"],
        },
        Table {
            name: "discovered_posts",
            columns: "
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
                last_seen_at           INTEGER NOT NULL",
            epochs: &["first_discovered_at", "last_seen_at"],
        },
        Table {
            name: "account_snapshots",
            columns: "
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                snapshot_at INTEGER NOT NULL,
                followers   INTEGER NOT NULL DEFAULT 0,
                following   INTEGER NOT NULL DEFAULT 0,
                tweets      INTEGER NOT NULL DEFAULT 0",
            epochs: &["snapshot_at"],
        },
        Table {
            name: "follower_list",
            columns: "
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                snapshot_at INTEGER NOT NULL,
                user_id     TEXT NOT NULL,
                username    TEXT NOT NULL,
                followers   INTEGER NOT NULL DEFAULT 0",
            epochs: &["snapshot_at"],
        },
        Table {
            name: "scheduled_posts",
            columns: "
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
                auto_scheduled  INTEGER NOT NULL DEFAULT 0,
                fire_token      TEXT",
            epochs: &["scheduled_at", "created_at", "fired_at"],
        },
    ],
    finish: "
        CREATE INDEX IF NOT EXISTS idx_posts_posted_at ON posts(posted_at);
        CREATE INDEX IF NOT EXISTS idx_snapshots_tweet ON metric_snapshots(tweet_id);
        CREATE INDEX IF NOT EXISTS idx_follower_list_snapshot ON follower_list(snapshot_at);
        CREATE INDEX IF NOT EXISTS idx_discovered_author ON discovered_posts(author_username);
        CREATE INDEX IF NOT EXISTS idx_discovered_last_seen ON discovered_posts(last_seen_at DESC);
        CREATE INDEX IF NOT EXISTS idx_discovered_impressions ON discovered_posts(impression_count DESC);
        CREATE INDEX IF NOT EXISTS idx_scheduled_status ON scheduled_posts(status, scheduled_at);

        -- Each logged reply with the latest snapshot of our reply post.
        -- Not read by xmaster itself; kept for ad-hoc SQL.
        DROP VIEW IF EXISTS reply_outcomes;
        CREATE VIEW reply_outcomes AS
            SELECT ea.id AS action_id, ea.target_tweet_id, ea.target_user_id,
                   ea.target_username, ea.target_followers, ea.reply_tweet_id,
                   ea.reply_style, ea.performed_at, ea.got_reply_back,
                   ms.likes, ms.retweets, ms.replies, ms.impressions, ms.bookmarks,
                   ms.quotes, ms.profile_clicks, ms.url_clicks, ms.snapshot_at,
                   ms.minutes_since_post
            FROM engagement_actions ea
            LEFT JOIN metric_snapshots ms
              ON ms.id = (SELECT MAX(id) FROM metric_snapshots WHERE tweet_id = ea.reply_tweet_id)
            WHERE ea.action_type = 'reply' AND ea.reply_tweet_id IS NOT NULL;
    ",
};

pub(crate) const BOOKMARKS: Schema = Schema {
    file: "bookmarks.db",
    version: 1,
    tables: &[
        Table {
            name: "bookmarks",
            columns: "
                tweet_id        TEXT PRIMARY KEY,
                author_username TEXT NOT NULL,
                author_name     TEXT,
                text            TEXT NOT NULL,
                created_at      TEXT,
                bookmarked_at   INTEGER NOT NULL,
                likes           INTEGER DEFAULT 0,
                retweets        INTEGER DEFAULT 0,
                replies         INTEGER DEFAULT 0,
                impressions     INTEGER DEFAULT 0,
                has_media       INTEGER DEFAULT 0,
                has_link        INTEGER DEFAULT 0,
                tags            TEXT DEFAULT '',
                notes           TEXT DEFAULT '',
                read            INTEGER DEFAULT 0,
                exported        INTEGER DEFAULT 0",
            epochs: &["bookmarked_at"],
        },
        Table {
            name: "bookmark_sync_log",
            columns: "
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                synced_at   INTEGER NOT NULL,
                new_count   INTEGER NOT NULL,
                total_count INTEGER NOT NULL",
            epochs: &["synced_at"],
        },
    ],
    finish: "
        CREATE INDEX IF NOT EXISTS idx_bookmarks_date ON bookmarks(bookmarked_at);
        CREATE INDEX IF NOT EXISTS idx_bookmarks_author ON bookmarks(author_username);
        CREATE INDEX IF NOT EXISTS idx_bookmarks_read ON bookmarks(read);
    ",
};

/// Open `schema.file` in the config dir at the current schema version.
pub(crate) fn open(schema: &Schema) -> Result<Connection, XmasterError> {
    let conn = super::open_db(schema.file)?;
    if let Err(e) = migrate(&conn, schema) {
        tracing::warn!(db = schema.file, error = %e, "schema migration failed; retrying on next open");
    }
    Ok(conn)
}

/// Bring `conn` to `schema.version`. Returns whether anything ran.
pub(crate) fn migrate(conn: &Connection, schema: &Schema) -> rusqlite::Result<bool> {
    if user_version(conn)? >= schema.version {
        return Ok(false);
    }
    // SQLite's recipe for table rebuilds: foreign keys off (DROP TABLE would
    // otherwise run an FK-checked DELETE; the bundled SQLite enforces FKs),
    // and legacy rename so RENAME doesn't re-parse views that name a table
    // mid-rebuild (`finish` recreates them). Both only take effect outside a
    // transaction.
    let fk_on: bool = conn.query_row("PRAGMA foreign_keys", [], |r| r.get(0))?;
    conn.execute_batch("PRAGMA foreign_keys = OFF; PRAGMA legacy_alter_table = ON;")?;
    let result = migrate_locked(conn, schema);
    conn.execute_batch(&format!(
        "PRAGMA legacy_alter_table = OFF; PRAGMA foreign_keys = {};",
        if fk_on { "ON" } else { "OFF" }
    ))?;
    result
}

fn migrate_locked(conn: &Connection, schema: &Schema) -> rusqlite::Result<bool> {
    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    // Another process may have migrated while we waited for the write lock.
    if user_version(&tx)? >= schema.version {
        return Ok(false);
    }
    for table in schema.tables {
        ensure_table(&tx, table)?;
    }
    tx.execute_batch(schema.finish)?;
    tx.pragma_update(None, "user_version", schema.version)?;
    tx.commit()?;
    Ok(true)
}

fn user_version(conn: &Connection) -> rusqlite::Result<i32> {
    conn.query_row("PRAGMA user_version", [], |r| r.get(0))
}

/// `(name, declared type)` of each column; empty when the table doesn't exist.
fn columns(conn: &Connection, table: &str) -> rusqlite::Result<Vec<(String, String)>> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let rows = stmt.query_map([], |r| Ok((r.get(1)?, r.get(2)?)))?;
    rows.collect()
}

fn foreign_keys(conn: &Connection, table: &str) -> rusqlite::Result<i64> {
    conn.query_row(
        "SELECT COUNT(*) FROM pragma_foreign_key_list(?1)",
        [table],
        |r| r.get(0),
    )
}

fn ensure_table(tx: &Connection, t: &Table) -> rusqlite::Result<()> {
    let old = columns(tx, t.name)?;
    if old.is_empty() {
        return tx.execute_batch(&format!("CREATE TABLE {} ({});", t.name, t.columns));
    }

    let tmp = format!("{}__rebuild", t.name);
    tx.execute_batch(&format!("DROP TABLE IF EXISTS {tmp}; CREATE TABLE {tmp} ({});", t.columns))?;
    let new = columns(tx, &tmp)?;
    let has = |cols: &[(String, String)], name: &str| cols.iter().any(|(c, _)| c == name);
    let missing_column = new.iter().any(|(c, _)| !has(&old, c));
    let text_epoch = old
        .iter()
        .any(|(c, ty)| t.epochs.contains(&c.as_str()) && !ty.eq_ignore_ascii_case("INTEGER"));
    let fk_changed = foreign_keys(tx, t.name)? != foreign_keys(tx, &tmp)?;

    if missing_column || text_epoch || fk_changed {
        // Keep columns the current DDL doesn't know rather than drop data.
        for (c, ty) in old.iter().filter(|(c, _)| !has(&new, c)) {
            tx.execute_batch(&format!("ALTER TABLE {tmp} ADD COLUMN \"{c}\" {ty};"))?;
        }
        let names: Vec<String> = old.iter().map(|(c, _)| format!("\"{c}\"")).collect();
        let values: Vec<String> = old
            .iter()
            .map(|(c, _)| {
                if t.epochs.contains(&c.as_str()) {
                    epoch_sql(c)
                } else {
                    format!("\"{c}\"")
                }
            })
            .collect();
        let copied = tx.execute(
            &format!(
                "INSERT INTO {tmp} ({}) SELECT {} FROM {}",
                names.join(", "),
                values.join(", "),
                t.name
            ),
            [],
        )?;
        let expected: usize = tx.query_row(&format!("SELECT COUNT(*) FROM {}", t.name), [], |r| r.get(0))?;
        if copied != expected {
            return Err(rusqlite::Error::StatementChangedRows(copied));
        }
        let seq: Option<i64> = tx
            .query_row("SELECT seq FROM sqlite_sequence WHERE name = ?1", [t.name], |r| r.get(0))
            .optional()
            .ok()
            .flatten();
        tx.execute_batch(&format!("DROP TABLE {0}; ALTER TABLE {tmp} RENAME TO {0};", t.name))?;
        if let Some(seq) = seq {
            // AUTOINCREMENT never reuses an id, even one whose row was deleted.
            tx.execute(
                "UPDATE sqlite_sequence SET seq = MAX(seq, ?1) WHERE name = ?2",
                params![seq, t.name],
            )?;
        }
    } else {
        tx.execute_batch(&format!("DROP TABLE {tmp};"))?;
    }

    // Text that slipped into an INTEGER column (an ISO string can't take
    // INTEGER affinity) is converted in place.
    for c in t.epochs {
        tx.execute(
            &format!(
                "UPDATE {} SET {c} = {} WHERE typeof({c}) NOT IN ('integer', 'null')",
                t.name,
                epoch_sql(c)
            ),
            [],
        )?;
    }
    Ok(())
}

/// SQL converting column `c` to a Unix epoch: integers and NULL pass through,
/// reals and digit strings are cast, anything else is parsed as a date
/// (`2026-03-24T19:20:38.05+00:00`). Digit strings must not reach
/// `strftime`, which reads a bare number as a Julian day.
fn epoch_sql(c: &str) -> String {
    format!(
        "CASE WHEN typeof(\"{c}\") IN ('integer', 'null') THEN \"{c}\" \
              WHEN typeof(\"{c}\") = 'real' THEN CAST(\"{c}\" AS INTEGER) \
              WHEN trim(\"{c}\") GLOB '[0-9]*' AND trim(\"{c}\") NOT GLOB '*[^0-9]*' \
                   THEN CAST(trim(\"{c}\") AS INTEGER) \
              ELSE COALESCE(CAST(strftime('%s', \"{c}\") AS INTEGER), CAST(\"{c}\" AS INTEGER)) END"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn types(conn: &Connection, table: &str, col: &str) -> Vec<(String, i64)> {
        conn.prepare(&format!("SELECT typeof({col}), COUNT(*) FROM {table} GROUP BY 1 ORDER BY 1"))
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    }

    /// The shapes found in the owner's DB: TEXT-declared epoch columns holding
    /// digit strings and ISO-8601 strings, a missing column, a view over the
    /// rebuilt tables, and an AUTOINCREMENT sequence ahead of the max id.
    fn legacy_xmaster(conn: &Connection) {
        conn.execute_batch(
            "CREATE TABLE posts (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                tweet_id TEXT UNIQUE NOT NULL, text TEXT NOT NULL,
                content_type TEXT NOT NULL DEFAULT 'text', char_count INTEGER NOT NULL,
                has_link BOOLEAN NOT NULL DEFAULT 0, has_media BOOLEAN NOT NULL DEFAULT 0,
                has_poll BOOLEAN NOT NULL DEFAULT 0, hashtag_count INTEGER NOT NULL DEFAULT 0,
                hook_text TEXT, posted_at TEXT NOT NULL,
                day_of_week INTEGER NOT NULL, hour_of_day INTEGER NOT NULL,
                reply_to_id TEXT, quote_of_id TEXT, preflight_score REAL,
                local_day_of_week INTEGER, local_hour_of_day INTEGER, tz_offset_minutes INTEGER,
                analysis_json TEXT, analysis_version INTEGER DEFAULT 1, scheduled_post_id TEXT,
                legacy_note TEXT);
            CREATE TABLE metric_snapshots (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                tweet_id TEXT NOT NULL REFERENCES posts(tweet_id), snapshot_at TEXT NOT NULL,
                minutes_since_post INTEGER NOT NULL, likes INTEGER NOT NULL DEFAULT 0,
                retweets INTEGER NOT NULL DEFAULT 0, replies INTEGER NOT NULL DEFAULT 0,
                impressions INTEGER NOT NULL DEFAULT 0, bookmarks INTEGER NOT NULL DEFAULT 0,
                quotes INTEGER NOT NULL DEFAULT 0, profile_clicks INTEGER NOT NULL DEFAULT 0);
            CREATE TABLE engagement_actions (
                id INTEGER PRIMARY KEY AUTOINCREMENT, action_type TEXT NOT NULL,
                target_tweet_id TEXT, target_user_id TEXT, target_username TEXT,
                performed_at TEXT NOT NULL, got_reply_back BOOLEAN DEFAULT NULL,
                target_followers INTEGER, reply_tweet_id TEXT, reply_style TEXT);
            CREATE VIEW reply_outcomes AS SELECT ea.id FROM engagement_actions ea
                LEFT JOIN metric_snapshots ms ON ms.tweet_id = ea.reply_tweet_id;

            INSERT INTO posts (id, tweet_id, text, char_count, posted_at, day_of_week, hour_of_day, legacy_note)
            VALUES (1, 'p1', 'iso post', 8, '2026-03-24T23:54:00.649877+00:00', 1, 23, 'kept'),
                   (7, 'p2', 'epoch post', 10, '1774748322', 6, 1, NULL);
            DELETE FROM posts WHERE id = 7;
            INSERT INTO posts (id, tweet_id, text, char_count, posted_at, day_of_week, hour_of_day)
            VALUES (2, 'p2', 'epoch post', 10, '1774748322', 6, 1);
            INSERT INTO metric_snapshots (tweet_id, snapshot_at, minutes_since_post, impressions)
            VALUES ('p2', '1774748400', 1, 10), ('p2', '1774752000', 61, 500);
            INSERT INTO engagement_actions (action_type, target_username, performed_at, reply_tweet_id)
            VALUES ('reply', 'someone', '1774414349', 'p1');",
        )
        .unwrap();
    }

    #[test]
    fn rebuilds_legacy_text_epochs_and_keeps_every_row() {
        let conn = Connection::open_in_memory().unwrap();
        legacy_xmaster(&conn);

        assert!(migrate(&conn, &XMASTER).unwrap());
        assert_eq!(user_version(&conn).unwrap(), XMASTER.version);

        // Same rows, now INTEGER epochs; ISO strings parsed, not cast to 2026.
        assert_eq!(types(&conn, "posts", "posted_at"), vec![("integer".into(), 2)]);
        let posted: Vec<i64> = conn
            .prepare("SELECT posted_at FROM posts ORDER BY id")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(posted, vec![1774396440, 1774748322]);
        assert_eq!(types(&conn, "metric_snapshots", "snapshot_at"), vec![("integer".into(), 2)]);
        assert_eq!(types(&conn, "engagement_actions", "performed_at"), vec![("integer".into(), 1)]);

        // Declared types are fixed, missing columns added, unknown ones kept.
        let cols = columns(&conn, "posts").unwrap();
        assert!(cols.iter().any(|(c, ty)| c == "posted_at" && ty == "INTEGER"));
        assert!(cols.iter().any(|(c, _)| c == "legacy_note"));
        assert!(columns(&conn, "metric_snapshots").unwrap().iter().any(|(c, _)| c == "url_clicks"));
        // metrics may baseline posts xmaster didn't log: no FK to posts.
        assert_eq!(foreign_keys(&conn, "metric_snapshots").unwrap(), 0);
        let fk_on: bool = conn.query_row("PRAGMA foreign_keys", [], |r| r.get(0)).unwrap();
        assert!(fk_on, "migrate restores the connection's FK setting");
        assert!(columns(&conn, "engagement_actions").unwrap().iter().any(|(c, _)| c == "reply_checked_at"));
        let note: String = conn
            .query_row("SELECT legacy_note FROM posts WHERE id = 1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(note, "kept");

        // AUTOINCREMENT high-water mark survives (id 7 was used then deleted).
        let seq: i64 = conn
            .query_row("SELECT seq FROM sqlite_sequence WHERE name = 'posts'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(seq, 7);

        // Indexes and the view are back; no scratch tables remain.
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name IN ('idx_snapshots_tweet', 'reply_outcomes')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 2);
        let scratch: i64 = conn
            .query_row("SELECT COUNT(*) FROM sqlite_master WHERE name LIKE '%__rebuild'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(scratch, 0);
        let outcome: i64 = conn
            .query_row("SELECT performed_at FROM reply_outcomes", [], |r| r.get(0))
            .unwrap();
        assert_eq!(outcome, 1774414349);

        // Idempotent: a second run is a no-op.
        assert!(!migrate(&conn, &XMASTER).unwrap());
    }

    #[test]
    fn converts_bookmarks_iso_and_digit_epochs() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE bookmarks (
                tweet_id TEXT PRIMARY KEY, author_username TEXT NOT NULL, author_name TEXT,
                text TEXT NOT NULL, created_at TEXT, bookmarked_at TEXT NOT NULL,
                likes INTEGER DEFAULT 0, retweets INTEGER DEFAULT 0, replies INTEGER DEFAULT 0,
                impressions INTEGER DEFAULT 0, has_media INTEGER DEFAULT 0, has_link INTEGER DEFAULT 0,
                tags TEXT DEFAULT '', notes TEXT DEFAULT '', read INTEGER DEFAULT 0,
                exported INTEGER DEFAULT 0);
            CREATE INDEX idx_bookmarks_date ON bookmarks(bookmarked_at);
            CREATE TABLE bookmark_sync_log (
                id INTEGER PRIMARY KEY AUTOINCREMENT, synced_at TEXT NOT NULL,
                new_count INTEGER NOT NULL, total_count INTEGER NOT NULL);
            INSERT INTO bookmarks (tweet_id, author_username, text, bookmarked_at)
            VALUES ('b1', 'a', 'one', '2026-03-24T19:20:38.054935+00:00'),
                   ('b2', 'b', 'two', '1791857078');
            INSERT INTO bookmark_sync_log (synced_at, new_count, total_count)
            VALUES ('2026-03-24T19:20:38.054935+00:00', 1, 1), ('1791857078', 1, 2);",
        )
        .unwrap();

        assert!(migrate(&conn, &BOOKMARKS).unwrap());
        assert_eq!(types(&conn, "bookmarks", "bookmarked_at"), vec![("integer".into(), 2)]);
        assert_eq!(types(&conn, "bookmark_sync_log", "synced_at"), vec![("integer".into(), 2)]);
        let b1: i64 = conn
            .query_row("SELECT bookmarked_at FROM bookmarks WHERE tweet_id = 'b1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(b1, 1774380038);
        let idx: i64 = conn
            .query_row("SELECT COUNT(*) FROM sqlite_master WHERE name = 'idx_bookmarks_date'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(idx, 1);
    }

    #[test]
    fn fresh_file_gets_current_schema_and_stray_text_is_fixed() {
        let conn = Connection::open_in_memory().unwrap();
        assert!(migrate(&conn, &XMASTER).unwrap());
        assert_eq!(columns(&conn, "scheduled_posts").unwrap().len(), 17);

        // A text epoch in an INTEGER column (ISO strings can't take INTEGER
        // affinity) is repaired by the next migration.
        conn.execute_batch(
            "INSERT INTO watchlist_accounts (username, added_at) VALUES ('x', '2026-03-24T19:20:38Z');
             PRAGMA user_version = 0;",
        )
        .unwrap();
        assert!(migrate(&conn, &XMASTER).unwrap());
        let added: i64 = conn
            .query_row("SELECT added_at FROM watchlist_accounts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(added, 1774380038);
    }
}
