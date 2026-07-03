//! Spike leg 2: fastembed (bge-small-en-v1.5, 384-dim) → sqlite-vec KNN.
//!
//! Exit criteria:
//!  - model downloads/caches and loads
//!  - embed→insert→query round-trip < ~50ms/item after warm-up
//!  - sanity ranking on hand-written samples: type-enriched color / code / URL
//!    are retrieved by natural-language queries

use anyhow::{bail, Result};
use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};
use rusqlite::{ffi::sqlite3_auto_extension, Connection};
use sqlite_vec::sqlite3_vec_init;
use std::time::Instant;

fn to_blob(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_le_bytes()).collect()
}

fn main() -> Result<()> {
    let t0 = Instant::now();
    let model = TextEmbedding::try_new(
        InitOptions::new(EmbeddingModel::BGESmallENV15).with_show_download_progress(true),
    )?;
    println!("model load (incl. download on first run): {:?}", t0.elapsed());

    // ~20 samples shaped like real captures, type-enriched like the pipeline will do
    let samples: Vec<&str> = vec![
        "hex color code: #3B82F6",
        "hex color code: #EF4444",
        "rgb color: rgb(16, 185, 129)",
        "email address: alice@example.com",
        "phone number: +91 98765 43210",
        "ip address: 192.168.1.1",
        "uuid: 550e8400-e29b-41d4-a716-446655440000",
        "currency amount: $1,299.99",
        "date: 2026-07-03",
        "file path: C:\\Users\\adamy\\Desktop\\projects\\smart-clipboard",
        "url: https://github.com/asg017/sqlite-vec",
        "url: https://docs.rs/fastembed/latest/fastembed/",
        "javascript code: const store = useAuthStore(); store.login(user);",
        "rust code: let conn = Connection::open(path)?; conn.pragma_update(None, \"key\", key)?;",
        "sql code: CREATE VIRTUAL TABLE captures_fts USING fts5(raw_text);",
        "text: SQLCipher provides transparent 256-bit AES encryption of database files",
        "text: Tauri apps have a much smaller memory footprint than Electron",
        "text: the mitochondria is the powerhouse of the cell",
        "text: flight AI-302 departs Delhi at 6:40am on Tuesday",
        "text: paneer tikka recipe with charred peppers and smoked yogurt",
    ];

    // warm-up + embed corpus
    let t1 = Instant::now();
    let embs = model.embed(samples.clone(), None)?;
    let dim = embs[0].len();
    println!("embedded {} samples in {:?} (dim = {dim})", samples.len(), t1.elapsed());
    if dim != 384 {
        bail!("expected 384-dim, got {dim}");
    }

    // per-item latency, warm
    let t2 = Instant::now();
    let n_timed = 10;
    for i in 0..n_timed {
        let _ = model.embed(vec![samples[i % samples.len()]], None)?;
    }
    let per_item = t2.elapsed() / n_timed as u32;
    println!("warm per-item embed latency: {per_item:?}");

    // store in sqlite-vec
    unsafe {
        sqlite3_auto_extension(Some(std::mem::transmute(sqlite3_vec_init as *const ())));
    }
    let conn = Connection::open_in_memory()?;
    conn.execute_batch(
        "CREATE VIRTUAL TABLE cap_vec USING vec0(id INTEGER PRIMARY KEY, embedding FLOAT[384]);",
    )?;
    for (i, e) in embs.iter().enumerate() {
        conn.execute(
            "INSERT INTO cap_vec(id, embedding) VALUES (?1, ?2)",
            rusqlite::params![i as i64, to_blob(e)],
        )?;
    }

    // NL queries → expected top hit (by substring of the sample text)
    let cases: &[(&str, &str)] = &[
        ("what blue color did I copy", "#3B82F6"),
        ("someone's email address", "alice@example.com"),
        ("that github link for the vector sqlite extension", "sqlite-vec"),
        ("code for logging in a user with the auth store", "useAuthStore"),
        ("how much did that thing cost", "$1,299.99"),
        ("database encryption info", "SQLCipher"),
        ("indian food recipe", "paneer"),
    ];

    let mut failures = 0;
    for (query, expect) in cases {
        // BGE models: queries benefit from an instruction prefix
        let q = format!("Represent this sentence for searching relevant passages: {query}");
        let qe = model.embed(vec![q], None)?.remove(0);
        let t = Instant::now();
        let mut stmt = conn.prepare(
            "SELECT id, distance FROM cap_vec WHERE embedding MATCH ?1 AND k = 3 ORDER BY distance",
        )?;
        let top: Vec<(i64, f64)> = stmt
            .query_map([to_blob(&qe)], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<std::result::Result<_, _>>()?;
        let knn_time = t.elapsed();
        let top_texts: Vec<&str> = top.iter().map(|(i, _)| samples[*i as usize]).collect();
        let rank = top_texts.iter().position(|t| t.contains(expect));
        let ok = rank.is_some();
        if !ok {
            failures += 1;
        }
        println!(
            "[{}] \"{query}\" -> top1: {:?}  (expected '{expect}' in top3: {}, knn {knn_time:?})",
            if ok { "ok" } else { "MISS" },
            top_texts.first().unwrap_or(&""),
            rank.map(|r| format!("rank {}", r + 1)).unwrap_or("NO".into()),
        );
    }

    println!(
        "\nEMBED SPIKE: {} ({} / {} queries hit top-3, warm latency {per_item:?})",
        if failures == 0 { "ALL PASS" } else { "PARTIAL" },
        cases.len() - failures,
        cases.len(),
    );
    if failures > cases.len() / 2 {
        bail!("retrieval quality unacceptable");
    }
    Ok(())
}
