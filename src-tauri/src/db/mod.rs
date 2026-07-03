//! Connection management: SQLCipher key from OS keychain, sqlite-vec
//! registration, migrations.

use anyhow::{Context, Result};
use rusqlite::{ffi::sqlite3_auto_extension, Connection};
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
    Ok(())
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}
