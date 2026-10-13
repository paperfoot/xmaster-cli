pub mod bookmarks;
pub mod store;
pub mod preflight;
pub mod scheduler;
pub mod schema;
pub mod tracker;

use rusqlite::Connection;

use crate::errors::XmasterError;

/// Open (or create) a SQLite database in the xmaster config dir with the
/// standard pragmas (WAL journal, 5s busy timeout, NORMAL sync).
///
/// Opens without touching the schema; stores go through `schema::open`.
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
