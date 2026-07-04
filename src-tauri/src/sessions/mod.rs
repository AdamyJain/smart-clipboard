pub mod adaptive;
pub mod finalize;
pub mod scoring;
pub mod sweep;

use anyhow::Result;
use rusqlite::{params, Connection};

/// Every manual session correction is training signal for the adaptive
/// threshold (FR5): kind = merge | split | reassign | remove | rename.
pub fn log_correction(
    conn: &Connection,
    kind: &str,
    from_session: Option<&str>,
    to_session: Option<&str>,
    capture_id: Option<&str>,
) -> Result<()> {
    conn.execute(
        "INSERT INTO session_corrections (id, ts, kind, from_session, to_session, capture_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            ulid::Ulid::new().to_string(),
            crate::db::now_ms(),
            kind,
            from_session,
            to_session,
            capture_id
        ],
    )?;
    Ok(())
}
