//! Local embedding worker: runs in the core (never the webview), consumes
//! captures with embedded = 0, writes type-enriched vectors to captures_vec.
//! Free, offline, runs for EVERY capture ambient or session (FR11a).

use crate::db::now_ms;
use crate::pipeline::fast_tier::embedding_input;
use anyhow::Result;
use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};
use rusqlite::{params, Connection};
use std::sync::mpsc::Receiver;

pub fn to_blob(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_le_bytes()).collect()
}

pub fn load_model(cache_dir: std::path::PathBuf) -> Result<TextEmbedding> {
    Ok(TextEmbedding::try_new(
        InitOptions::new(EmbeddingModel::BGESmallENV15).with_cache_dir(cache_dir),
    )?)
}

/// BGE models retrieve better when the query (not the document) carries an
/// instruction prefix.
pub fn embed_query(model: &TextEmbedding, query: &str) -> Result<Vec<f32>> {
    let q = format!("Represent this sentence for searching relevant passages: {query}");
    Ok(model.embed(vec![q], None)?.remove(0))
}

/// Worker loop: wake on signal (or every 5s as a safety net), embed pending
/// captures in small batches.
pub fn worker_loop_shared(conn: Connection, model: std::sync::Arc<TextEmbedding>, wake: Receiver<()>) {
    loop {
        // drain pending wakes; timeout keeps us resilient to missed signals
        let _ = wake.recv_timeout(std::time::Duration::from_secs(5));
        if let Err(e) = drain_pending(&conn, &model) {
            eprintln!("[embed] error: {e:#}");
            std::thread::sleep(std::time::Duration::from_secs(2));
        }
    }
}

pub fn drain_pending(conn: &Connection, model: &TextEmbedding) -> Result<()> {
    loop {
        let mut stmt = conn.prepare(
            "SELECT id, entity_type, raw_text FROM captures
             WHERE embedded = 0 AND deleted_at IS NULL AND sensitivity != 'secret'
             ORDER BY captured_at LIMIT 16",
        )?;
        let batch: Vec<(String, String, String)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<std::result::Result<_, _>>()?;
        drop(stmt);
        if batch.is_empty() {
            return Ok(());
        }
        let inputs: Vec<String> = batch
            .iter()
            .map(|(_, et, txt)| embedding_input(et, txt))
            .collect();
        let t = std::time::Instant::now();
        let embeddings = model.embed(inputs, None)?;
        for ((id, _, _), emb) in batch.iter().zip(embeddings.iter()) {
            conn.execute(
                "INSERT OR REPLACE INTO captures_vec(capture_id, embedding) VALUES (?1, ?2)",
                params![id, to_blob(emb)],
            )?;
            conn.execute("UPDATE captures SET embedded = 1 WHERE id = ?1", params![id])?;
        }
        let _ = now_ms();
        eprintln!("[embed] {} captures embedded in {:?}", batch.len(), t.elapsed());
    }
}
