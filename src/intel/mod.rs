pub mod bookmarks;
pub mod store;
pub mod preflight;
pub mod scheduler;
pub mod tracker;
pub mod rails;

use rusqlite::Connection;

use crate::errors::XmasterError;

/// Open (or create) a SQLite database in the xmaster config dir with the
/// standard pragmas (WAL journal, 5s busy timeout, NORMAL sync).
///
/// Used by the tracker, scheduler, and bookmark stores. `IntelStore` keeps its
/// own `open_at` because tests inject explicit paths and it propagates pragma
/// errors instead of ignoring them.
pub(crate) fn open_db(file_name: &str) -> Result<Connection, XmasterError> {
    let dir = crate::config::config_dir();
    std::fs::create_dir_all(&dir).ok();
    let conn = Connection::open(dir.join(file_name))
        .map_err(|e| XmasterError::Config(format!("DB open error ({file_name}): {e}")))?;
    conn.pragma_update(None, "journal_mode", "wal").ok();
    conn.pragma_update(None, "busy_timeout", 5000).ok();
    conn.pragma_update(None, "synchronous", "NORMAL").ok();
    Ok(conn)
}

/// Best-effort additive migration: add `column` to `table` unless it already
/// exists. Errors are swallowed — the same idiom the tracker used inline.
pub(crate) fn add_column_if_missing(conn: &Connection, table: &str, column: &str, decl: &str) {
    let cols: Vec<String> = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .and_then(|mut s| {
            s.query_map([], |row| row.get::<_, String>(1))?
                .collect::<Result<Vec<_>, _>>()
        })
        .unwrap_or_default();
    if !cols.iter().any(|c| c == column) {
        conn.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {column} {decl};"))
            .ok();
    }
}
