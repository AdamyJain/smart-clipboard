//! Merged retrieval: FTS5 (exact/substring) + sqlite-vec (semantic), with a
//! recency boost. Fully offline.

use anyhow::Result;
use fastembed::TextEmbedding;
use rusqlite::{params, Connection};
use std::collections::HashMap;

#[derive(Debug, Clone, serde::Serialize)]
pub struct SearchHit {
    pub id: String,
    pub raw_text: String,
    pub entity_type: String,
    pub source_app: Option<String>,
    pub captured_at: i64,
    pub score: f64,
    pub matched_by: String, // fts | vec | both
}

pub fn search(
    conn: &Connection,
    model: &TextEmbedding,
    query: &str,
    entity_filter: Option<&str>,
    limit: usize,
) -> Result<Vec<SearchHit>> {
    let mut scores: HashMap<String, (f64, &'static str)> = HashMap::new();

    // --- FTS leg (trigram needs >= 3 chars) ---
    if query.chars().count() >= 3 {
        let mut stmt = conn.prepare(
            "SELECT c.id, bm25(captures_fts) FROM captures_fts
             JOIN captures c ON c.rowid = captures_fts.rowid
             WHERE captures_fts MATCH ?1 AND c.deleted_at IS NULL
             ORDER BY bm25(captures_fts) LIMIT 50",
        )?;
        // quote the query so FTS operators in user text don't error
        let quoted = format!("\"{}\"", query.replace('"', "\"\""));
        let rows: Vec<(String, f64)> = stmt
            .query_map([quoted], |r| Ok((r.get(0)?, r.get(1)?)))?
            .filter_map(|r| r.ok())
            .collect();
        for (id, bm25) in rows {
            // bm25 is negative-better; normalize to 0..1-ish
            let s = 1.0 / (1.0 + (-bm25).max(0.0).recip().min(10.0));
            scores.insert(id, (0.6 + 0.4 * s, "fts"));
        }
    }

    // --- vector leg ---
    let qvec = crate::pipeline::embed::embed_query(model, query)?;
    let blob = crate::pipeline::embed::to_blob(&qvec);
    let mut stmt = conn.prepare(
        "SELECT capture_id, distance FROM captures_vec
         WHERE embedding MATCH ?1 AND k = 50",
    )?;
    let rows: Vec<(String, f64)> = stmt
        .query_map([blob], |r| Ok((r.get(0)?, r.get(1)?)))?
        .filter_map(|r| r.ok())
        .collect();
    for (id, dist) in rows {
        let s = (1.0 - dist / 2.0).clamp(0.0, 1.0); // cosine distance 0..2 → similarity
        scores
            .entry(id)
            .and_modify(|(sc, tag)| {
                *sc += 0.5 * s;
                *tag = "both";
            })
            .or_insert((0.5 * s, "vec"));
    }

    if scores.is_empty() {
        return Ok(vec![]);
    }

    // --- hydrate + recency boost + filter ---
    let now = crate::db::now_ms();
    let mut hits = Vec::new();
    for (id, (base, tag)) in scores {
        let row = conn.query_row(
            "SELECT raw_text, entity_type, source_app, captured_at FROM captures
             WHERE id = ?1 AND deleted_at IS NULL AND sensitivity != 'secret'",
            params![id],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, i64>(3)?,
                ))
            },
        );
        let Ok((raw_text, entity_type, source_app, captured_at)) = row else {
            continue;
        };
        if let Some(f) = entity_filter {
            if !entity_type.starts_with(f) {
                continue;
            }
        }
        let age_days = ((now - captured_at) as f64 / 86_400_000.0).max(0.0);
        let recency = 0.1 * (-age_days / 7.0).exp(); // half-ish life of a week
        hits.push(SearchHit {
            id,
            raw_text,
            entity_type,
            source_app,
            captured_at,
            score: base + recency,
            matched_by: tag.into(),
        });
    }
    hits.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    hits.truncate(limit);
    Ok(hits)
}

/// Recent captures for the palette's empty state.
pub fn recent(conn: &Connection, limit: usize) -> Result<Vec<SearchHit>> {
    let mut stmt = conn.prepare(
        "SELECT id, raw_text, entity_type, source_app, captured_at FROM captures
         WHERE deleted_at IS NULL AND sensitivity != 'secret'
         ORDER BY captured_at DESC LIMIT ?1",
    )?;
    let rows = stmt
        .query_map([limit], |r| {
            Ok(SearchHit {
                id: r.get(0)?,
                raw_text: r.get(1)?,
                entity_type: r.get(2)?,
                source_app: r.get(3)?,
                captured_at: r.get(4)?,
                score: 0.0,
                matched_by: "recent".into(),
            })
        })?
        .filter_map(|r| r.ok())
        .collect();
    Ok(rows)
}
