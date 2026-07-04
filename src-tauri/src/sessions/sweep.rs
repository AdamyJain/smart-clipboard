//! Periodic session maintenance: close sessions idle past the threshold.
//! The nightly adaptive-threshold job (phase 4) hangs off the same loop.

use crate::db::now_ms;
use anyhow::Result;
use rusqlite::{params, Connection};

pub const IDLE_CLOSE_MIN: i64 = 30;
const SWEEP_EVERY_SECS: u64 = 60;

/// Close every open session whose last activity is older than the idle
/// threshold. Returns how many were closed.
pub fn close_idle(conn: &Connection, now: i64, idle_min: i64) -> Result<usize> {
    let cutoff = now - idle_min * 60_000;
    let n = conn.execute(
        "UPDATE sessions SET status = 'closed', ended_at = last_activity_at
         WHERE status = 'open' AND deleted_at IS NULL AND last_activity_at < ?1",
        params![cutoff],
    )?;
    if n > 0 {
        eprintln!("[sessions] sweep closed {n} idle session(s)");
    }
    Ok(n)
}

/// Blocks forever — run on a dedicated thread with its own connection.
pub fn sweep_loop(conn: Connection) {
    loop {
        std::thread::sleep(std::time::Duration::from_secs(SWEEP_EVERY_SECS));
        if let Err(e) = close_idle(&conn, now_ms(), IDLE_CLOSE_MIN) {
            eprintln!("[sessions] sweep error: {e:#}");
        }
        // phase 4: nightly adaptive-threshold nudge from session_corrections
        if let Err(e) = crate::sessions::adaptive::maybe_nightly_adjust(&conn) {
            eprintln!("[sessions] adaptive threshold error: {e:#}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(include_str!("../db/schema.sql")).unwrap();
        conn.execute_batch("ALTER TABLE captures ADD COLUMN origin TEXT DEFAULT 'ambient';")
            .unwrap();
        conn
    }

    #[test]
    fn idle_session_closes_active_stays() {
        let conn = test_conn();
        let now = 100 * 60_000i64;
        conn.execute(
            "INSERT INTO sessions (id, topic, started_at, last_activity_at, status)
             VALUES ('old', 't', 0, ?1, 'open'), ('fresh', 't', 0, ?2, 'open')",
            params![now - 45 * 60_000, now - 5 * 60_000],
        )
        .unwrap();
        assert_eq!(close_idle(&conn, now, IDLE_CLOSE_MIN).unwrap(), 1);
        let status: String = conn
            .query_row("SELECT status FROM sessions WHERE id='old'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(status, "closed");
        let status: String = conn
            .query_row("SELECT status FROM sessions WHERE id='fresh'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(status, "open");
    }
}
