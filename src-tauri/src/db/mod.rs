//! Connection management: SQLCipher key from OS keychain, sqlite-vec
//! registration, migrations.

use anyhow::{Context, Result};
use rusqlite::{ffi::sqlite3_auto_extension, params, Connection};
use sqlite_vec::sqlite3_vec_init;
use std::path::Path;

const KEYRING_SERVICE: &str = "smart-clipboard";
const KEYRING_USER: &str = "db-key";

/// Must be called once, before any connection is opened.
pub fn register_vec_extension() {
    unsafe {
        sqlite3_auto_extension(Some(std::mem::transmute(sqlite3_vec_init as *const ())));
    }
}

fn db_key() -> Result<String> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER)?;
    match entry.get_password() {
        Ok(k) => Ok(k),
        Err(keyring::Error::NoEntry) => {
            use rand::RngCore;
            let mut bytes = [0u8; 32];
            rand::thread_rng().fill_bytes(&mut bytes);
            let key: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
            entry
                .set_password(&key)
                .context("storing new db key in OS keychain")?;
            Ok(key)
        }
        Err(e) => Err(e).context("reading db key from OS keychain"),
    }
}

pub fn open(db_path: &Path) -> Result<Connection> {
    let conn = Connection::open(db_path)
        .with_context(|| format!("opening db at {}", db_path.display()))?;
    // hex key form avoids quoting issues in the PRAGMA
    conn.pragma_update(None, "key", db_key()?)?;
    conn.pragma_update(None, "journal_mode", "wal")?;
    conn.pragma_update(None, "busy_timeout", 5000)?;
    migrate(&conn)?;
    Ok(conn)
}

fn migrate(conn: &Connection) -> Result<()> {
    let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if version < 1 {
        conn.execute_batch(include_str!("schema.sql"))?;
        conn.pragma_update(None, "user_version", 1)?;
    }
    if version < 2 {
        // origin of a capture: ambient (clipboard listener) | hotkey (Alt+C) |
        // extension (browser native messaging)
        conn.execute_batch("ALTER TABLE captures ADD COLUMN origin TEXT DEFAULT 'ambient';")?;
        conn.pragma_update(None, "user_version", 2)?;
    }
    Ok(())
}

/// Hard-deletes one capture: FTS entry, vector, tag links, then the row
/// itself. Secrets were never indexed into FTS, so that delete is
/// best-effort (external-content fts5 errors on a 'delete' of content it
/// never saw — ignored here, same as the upgrade-in-place path in fast_tier).
pub fn delete_capture(conn: &Connection, id: &str) -> Result<()> {
    let row: rusqlite::Result<(i64, String, String, String)> = conn.query_row(
        "SELECT rowid, raw_text, ifnull(context_before,''), ifnull(context_after,'')
         FROM captures WHERE id = ?1",
        [id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    );
    if let Ok((rowid, rt, cb, ca)) = row {
        let _ = conn.execute(
            "INSERT INTO captures_fts(captures_fts, rowid, raw_text, context_before, context_after)
             VALUES('delete', ?1, ?2, ?3, ?4)",
            params![rowid, rt, cb, ca],
        );
    }
    conn.execute("DELETE FROM captures_vec WHERE capture_id = ?1", [id])?;
    conn.execute("DELETE FROM capture_tags WHERE capture_id = ?1", [id])?;
    conn.execute("DELETE FROM captures WHERE id = ?1", [id])?;
    Ok(())
}

/// Hard-deletes every capture row. Returns the number removed.
pub fn delete_all_captures(conn: &Connection) -> Result<usize> {
    let mut stmt = conn.prepare("SELECT id FROM captures")?;
    let ids: Vec<String> = stmt
        .query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    drop(stmt);
    for id in &ids {
        delete_capture(conn, id)?;
    }
    Ok(ids.len())
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}
