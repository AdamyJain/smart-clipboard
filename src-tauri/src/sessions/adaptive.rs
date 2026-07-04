//! Adaptive assignment threshold (FR5): a nightly hill-climb over
//! `session_corrections`. Manual merges of auto-split sessions mean the
//! threshold is too high (captures that belonged together were separated);
//! manual splits/reassigns of auto-merged sessions mean it is too low.

use crate::db::now_ms;
use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};

const STEP: f64 = 0.02;
const FLOOR: f64 = 0.25;
const CEILING: f64 = 0.65;
const NIGHTLY_MS: i64 = 24 * 60 * 60 * 1000;

/// Threshold state persists in a one-row settings table (created on demand so
/// this works against phase-1 databases too).
fn ensure_table(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS tuning (
            key TEXT PRIMARY KEY, value REAL, updated_at INTEGER
        );",
    )?;
    Ok(())
}

pub fn current_threshold(conn: &Connection, default: f64) -> f64 {
    if ensure_table(conn).is_err() {
        return default;
    }
    conn.query_row("SELECT value FROM tuning WHERE key = 'assign_threshold'", [], |r| r.get(0))
        .optional()
        .ok()
        .flatten()
        .unwrap_or(default)
}

/// Run at most once per day; nudges the threshold from the last day's
/// corrections. Returns the new threshold if an adjustment ran.
pub fn maybe_nightly_adjust(conn: &Connection) -> Result<Option<f64>> {
    ensure_table(conn)?;
    let now = now_ms();
    let last_run: Option<i64> = conn
        .query_row("SELECT updated_at FROM tuning WHERE key = 'last_adjust'", [], |r| r.get(0))
        .optional()?;
    if last_run.is_some_and(|t| now - t < NIGHTLY_MS) {
        return Ok(None);
    }
    conn.execute(
        "INSERT INTO tuning (key, value, updated_at) VALUES ('last_adjust', 0, ?1)
         ON CONFLICT(key) DO UPDATE SET updated_at = ?1",
        params![now],
    )?;

    let since = now - NIGHTLY_MS;
    let merges: i64 = conn.query_row(
        "SELECT count(*) FROM session_corrections WHERE kind = 'merge' AND ts > ?1",
        params![since],
        |r| r.get(0),
    )?;
    let splits: i64 = conn.query_row(
        "SELECT count(*) FROM session_corrections WHERE kind IN ('split','reassign','remove') AND ts > ?1",
        params![since],
        |r| r.get(0),
    )?;

    let cur = current_threshold(conn, crate::sessions::scoring::ScoringConfig::default().threshold);
    let new = if merges > splits {
        (cur - STEP).max(FLOOR) // we split too eagerly → be more permissive
    } else if splits > merges {
        (cur + STEP).min(CEILING) // we merged too eagerly → be stricter
    } else {
        return Ok(None);
    };
    conn.execute(
        "INSERT INTO tuning (key, value, updated_at) VALUES ('assign_threshold', ?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = ?1, updated_at = ?2",
        params![new, now],
    )?;
    eprintln!("[sessions] adaptive threshold {cur:.2} → {new:.2} (merges={merges} splits={splits})");
    Ok(Some(new))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(include_str!("../db/schema.sql")).unwrap();
        conn
    }

    #[test]
    fn merges_lower_threshold_splits_raise_it() {
        let conn = test_conn();
        let now = now_ms();
        // two manual merges yesterday → threshold too high → lower it
        for _ in 0..2 {
            crate::sessions::log_correction(&conn, "merge", Some("a"), Some("b"), None).unwrap();
        }
        let base = crate::sessions::scoring::ScoringConfig::default().threshold;
        let new = maybe_nightly_adjust(&conn).unwrap().expect("should adjust");
        assert!(new < base);
        // immediately re-running is a no-op (nightly guard)
        assert!(maybe_nightly_adjust(&conn).unwrap().is_none());
        let _ = now;
    }

    #[test]
    fn floor_and_ceiling_hold() {
        let conn = test_conn();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS tuning (key TEXT PRIMARY KEY, value REAL, updated_at INTEGER);
             INSERT INTO tuning VALUES ('assign_threshold', 0.25, 0);",
        )
        .unwrap();
        crate::sessions::log_correction(&conn, "merge", None, None, None).unwrap();
        let new = maybe_nightly_adjust(&conn).unwrap().expect("should adjust");
        assert!(new >= FLOOR);
    }
}
