//! Fast tier: classify → secret-check → dedupe → write (captures + FTS).
//! Synchronous, local, no network. Everything here must stay well under 200ms.

use crate::db::now_ms;
use crate::pipeline::{entities, secrets};
use anyhow::Result;
use rusqlite::{params, Connection};
use sha2::{Digest, Sha256};

#[derive(Default)]
pub struct IncomingCapture {
    pub raw_text: String,
    pub source_app: Option<String>,
    /// ambient | hotkey | extension
    pub origin: Option<String>,
    pub window_title: Option<String>,
    pub source_url: Option<String>,
    pub context_before: Option<String>,
    pub context_after: Option<String>,
    pub asset_id: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct StoredCapture {
    pub id: String,
    pub entity_type: String,
    pub sensitivity: String,
    pub preview: String,
    pub deduped: bool,
    pub captured_at: i64,
}

const DEDUPE_WINDOW_MS: i64 = 10 * 60 * 1000;

pub fn process(conn: &Connection, cap: IncomingCapture) -> Result<Option<StoredCapture>> {
    let text = cap.raw_text;
    if text.trim().is_empty() {
        return Ok(None);
    }

    let entity = entities::classify(&text);
    let sensitivity = secrets::detect(&text);
    let is_secret = sensitivity == secrets::Sensitivity::Secret;

    let hash = {
        let mut h = Sha256::new();
        h.update(text.as_bytes());
        h.update(cap.source_app.as_deref().unwrap_or("").as_bytes());
        format!("{:x}", h.finalize())
    };

    // dedupe against recent identical capture: bump timestamp instead of re-inserting
    let now = now_ms();
    let existing: Option<String> = conn
        .query_row(
            "SELECT id FROM captures WHERE dedupe_hash = ?1 AND captured_at > ?2 AND deleted_at IS NULL",
            params![hash, now - DEDUPE_WINDOW_MS],
            |r| r.get(0),
        )
        .ok();
    if let Some(id) = existing {
        conn.execute("UPDATE captures SET captured_at = ?1 WHERE id = ?2", params![now, id])?;
        // an intentional (Alt+C) re-capture of an ambient row upgrades it in
        // place: richer context wins, no duplicate row. The FTS row must be
        // dropped and re-added around the update (external-content FTS5 can't
        // see column changes on its own).
        if cap.origin.as_deref().is_some_and(|o| o != "ambient") {
            if !is_secret {
                let old: rusqlite::Result<(i64, String, String, String)> = conn.query_row(
                    "SELECT rowid, raw_text, ifnull(context_before,''), ifnull(context_after,'')
                     FROM captures WHERE id = ?1",
                    params![id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                );
                if let Ok((rowid, rt, cb, ca)) = old {
                    let _ = conn.execute(
                        "INSERT INTO captures_fts(captures_fts, rowid, raw_text, context_before, context_after)
                         VALUES('delete', ?1, ?2, ?3, ?4)",
                        params![rowid, rt, cb, ca],
                    );
                }
            }
            conn.execute(
                "UPDATE captures SET
                    origin = ?1,
                    page_title = coalesce(?2, page_title),
                    source_url = coalesce(?3, source_url),
                    context_before = coalesce(?4, context_before),
                    context_after = coalesce(?5, context_after),
                    asset_id = coalesce(?6, asset_id)
                 WHERE id = ?7",
                params![cap.origin, cap.window_title, cap.source_url, cap.context_before, cap.context_after, cap.asset_id, id],
            )?;
            if !is_secret {
                conn.execute(
                    "INSERT INTO captures_fts(rowid, raw_text, context_before, context_after)
                     SELECT rowid, raw_text, context_before, context_after FROM captures WHERE id = ?1",
                    params![id],
                )?;
            }
        }
        return Ok(Some(StoredCapture {
            id,
            entity_type: entity.as_str().into(),
            sensitivity: if is_secret { "secret" } else { "public" }.into(),
            preview: preview_of(&text),
            deduped: true,
            captured_at: now,
        }));
    }

    let id = ulid::Ulid::new().to_string();
    let content_type = if is_secret {
        "secret"
    } else if entity.as_str().starts_with("code") {
        "code"
    } else if entity.as_str() == "url" {
        "url"
    } else {
        "text"
    };

    conn.execute(
        "INSERT INTO captures (id, captured_at, content_type, entity_type, raw_text, source_app,
                               sensitivity, dedupe_hash, enrichment_status, origin, page_title,
                               source_url, context_before, context_after, asset_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'pending', ?9, ?10, ?11, ?12, ?13, ?14)",
        params![
            id,
            now,
            content_type,
            entity.as_str(),
            text,
            cap.source_app,
            if is_secret { "secret" } else { "public" },
            hash,
            cap.origin.as_deref().unwrap_or("ambient"),
            cap.window_title,
            cap.source_url,
            cap.context_before,
            cap.context_after,
            cap.asset_id,
        ],
    )?;

    // secrets are stored (encrypted at rest) but NEVER indexed — not in FTS,
    // not in the vector index, never sent anywhere (PRD FR10)
    if !is_secret {
        conn.execute(
            "INSERT INTO captures_fts(rowid, raw_text, context_before, context_after)
             SELECT rowid, raw_text, context_before, context_after FROM captures WHERE id = ?1",
            params![id],
        )?;
    } else {
        // mark as done so the embed worker skips it without querying content
        conn.execute("UPDATE captures SET embedded = 1 WHERE id = ?1", params![id])?;
    }

    Ok(Some(StoredCapture {
        id,
        entity_type: entity.as_str().into(),
        sensitivity: if is_secret { "secret" } else { "public" }.into(),
        preview: preview_of(&text),
        deduped: false,
        captured_at: now,
    }))
}

/// Type-enriched embedding input (FR9a): the entity type recognized here is
/// what makes a bare value like "#3B82F6" retrievable by "what color…".
pub fn embedding_input(entity_type: &str, raw_text: &str) -> String {
    let entity = entities::classify(raw_text); // cheap; avoids storing prefix
    let _ = entity_type;
    let prefix = entity.enrich_prefix();
    let clipped: String = raw_text.chars().take(2000).collect();
    format!("{prefix}: {clipped}")
}

fn preview_of(text: &str) -> String {
    let t = text.trim().replace(['\n', '\r'], " ");
    let p: String = t.chars().take(80).collect();
    if t.chars().count() > 80 { format!("{p}…") } else { p }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    fn test_conn() -> Connection {
        db::register_vec_extension();
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(include_str!("../db/schema.sql")).unwrap();
        conn.execute_batch("ALTER TABLE captures ADD COLUMN origin TEXT DEFAULT 'ambient';")
            .unwrap();
        conn
    }

    #[test]
    fn capture_is_searchable_immediately() {
        let conn = test_conn();
        let stored = process(&conn, IncomingCapture {
            raw_text: "const store = useAuthStore();".into(),
            source_app: Some("Code.exe".into()),
            ..Default::default()
        }).unwrap().unwrap();
        assert_eq!(stored.entity_type, "code:js");
        let hits: i64 = conn.query_row(
            "SELECT count(*) FROM captures_fts WHERE captures_fts MATCH 'AuthStore'",
            [], |r| r.get(0),
        ).unwrap();
        assert_eq!(hits, 1, "trigram substring must find it");
    }

    #[test]
    fn secret_stored_but_never_indexed() {
        let conn = test_conn();
        let stored = process(&conn, IncomingCapture {
            raw_text: "ghp_16C7e42F292c6912E7710c838347Ae178B4a".into(),
            ..Default::default()
        }).unwrap().unwrap();
        assert_eq!(stored.sensitivity, "secret");
        // external-content FTS: bare count(*) reads the content table, so the
        // index-emptiness check must go through MATCH
        let fts: i64 = conn.query_row(
            "SELECT count(*) FROM captures_fts WHERE captures_fts MATCH 'ghp'",
            [], |r| r.get(0),
        ).unwrap();
        assert_eq!(fts, 0, "secret must not enter FTS index");
        let embedded: i64 = conn.query_row(
            "SELECT embedded FROM captures WHERE id = ?1", [&stored.id], |r| r.get(0),
        ).unwrap();
        assert_eq!(embedded, 1, "secret must be pre-marked so embed worker skips it");
    }

    #[test]
    fn duplicate_within_window_dedupes() {
        let conn = test_conn();
        let a = process(&conn, IncomingCapture { raw_text: "same text".into(), ..Default::default() }).unwrap().unwrap();
        let b = process(&conn, IncomingCapture { raw_text: "same text".into(), ..Default::default() }).unwrap().unwrap();
        assert_eq!(a.id, b.id);
        assert!(b.deduped);
        let n: i64 = conn.query_row("SELECT count(*) FROM captures", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 1);
    }

    #[test]
    fn intentional_recapture_upgrades_ambient_row() {
        // Alt+C synthesizes Ctrl+C, so the ambient listener stores the row
        // first; the hotkey event must upgrade it in place, not duplicate it
        let conn = test_conn();
        let a = process(&conn, IncomingCapture {
            raw_text: "fn main() {}".into(),
            source_app: Some("cursor.exe".into()),
            ..Default::default()
        }).unwrap().unwrap();
        let b = process(&conn, IncomingCapture {
            raw_text: "fn main() {}".into(),
            source_app: Some("cursor.exe".into()),
            origin: Some("hotkey".into()),
            window_title: Some("main.rs — cursor".into()),
            ..Default::default()
        }).unwrap().unwrap();
        assert_eq!(a.id, b.id);
        let (origin, title): (String, Option<String>) = conn.query_row(
            "SELECT origin, page_title FROM captures WHERE id = ?1",
            [&a.id], |r| Ok((r.get(0)?, r.get(1)?)),
        ).unwrap();
        assert_eq!(origin, "hotkey");
        assert_eq!(title.as_deref(), Some("main.rs — cursor"));
        let n: i64 = conn.query_row("SELECT count(*) FROM captures", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 1);
    }

    #[test]
    fn empty_text_ignored() {
        let conn = test_conn();
        assert!(process(&conn, IncomingCapture { raw_text: "   ".into(), ..Default::default() }).unwrap().is_none());
    }
}
