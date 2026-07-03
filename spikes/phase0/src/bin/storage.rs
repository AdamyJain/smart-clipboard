//! Spike leg 1: SQLCipher + sqlite-vec + FTS5 trigram in ONE rusqlite connection.
//!
//! Exit criteria:
//!  - DB file is unreadable without the key (header is not "SQLite format 3",
//!    opening with the wrong key fails)
//!  - FTS5 trigram substring match works ("AuthStore" finds "useAuthStore")
//!  - vec0 KNN query returns sane order

use anyhow::{bail, Result};
use rusqlite::{ffi::sqlite3_auto_extension, Connection};
use sqlite_vec::sqlite3_vec_init;

fn to_blob(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_le_bytes()).collect()
}

fn main() -> Result<()> {
    let db_path = std::env::temp_dir().join("phase0_storage_spike.db");
    let _ = std::fs::remove_file(&db_path);

    // Register sqlite-vec for every connection opened from here on.
    unsafe {
        sqlite3_auto_extension(Some(std::mem::transmute(sqlite3_vec_init as *const ())));
    }

    // --- open encrypted DB ---
    let conn = Connection::open(&db_path)?;
    conn.pragma_update(None, "key", "spike-test-key")?;
    conn.pragma_update(None, "journal_mode", "wal")?;

    let cipher_version: String =
        conn.query_row("PRAGMA cipher_version", [], |r| r.get(0))?;
    let sqlite_version: String =
        conn.query_row("SELECT sqlite_version()", [], |r| r.get(0))?;
    let vec_version: String =
        conn.query_row("SELECT vec_version()", [], |r| r.get(0))?;
    println!("sqlcipher = {cipher_version}");
    println!("sqlite    = {sqlite_version}");
    println!("sqlite-vec= {vec_version}");

    // --- FTS5 trigram ---
    conn.execute_batch(
        "CREATE VIRTUAL TABLE cap_fts USING fts5(raw_text, tokenize='trigram');
         INSERT INTO cap_fts(raw_text) VALUES
           ('const store = useAuthStore();'),
           ('SELECT * FROM api_key_id WHERE 1'),
           ('completely unrelated sentence about cooking');",
    )?;
    let hit: String = conn.query_row(
        "SELECT raw_text FROM cap_fts WHERE raw_text MATCH 'AuthStore'",
        [],
        |r| r.get(0),
    )?;
    if !hit.contains("useAuthStore") {
        bail!("trigram substring match failed, got: {hit}");
    }
    println!("trigram substring match: OK ({hit})");

    // --- vec0 KNN ---
    conn.execute_batch(
        "CREATE VIRTUAL TABLE cap_vec USING vec0(capture_id TEXT PRIMARY KEY, embedding FLOAT[4]);",
    )?;
    let items: &[(&str, [f32; 4])] = &[
        ("a", [1.0, 0.0, 0.0, 0.0]),
        ("b", [0.9, 0.1, 0.0, 0.0]),
        ("c", [0.0, 1.0, 0.0, 0.0]),
        ("d", [0.0, 0.0, 1.0, 0.0]),
    ];
    for (id, v) in items {
        conn.execute(
            "INSERT INTO cap_vec(capture_id, embedding) VALUES (?1, ?2)",
            rusqlite::params![id, to_blob(v)],
        )?;
    }
    let query = to_blob(&[1.0, 0.05, 0.0, 0.0]);
    let mut stmt = conn.prepare(
        "SELECT capture_id, distance FROM cap_vec WHERE embedding MATCH ?1 AND k = 3 ORDER BY distance",
    )?;
    let order: Vec<String> = stmt
        .query_map([query], |r| r.get::<_, String>(0))?
        .collect::<std::result::Result<_, _>>()?;
    println!("knn order = {order:?}");
    if order.first().map(String::as_str) != Some("a") || order.get(1).map(String::as_str) != Some("b") {
        bail!("KNN order not sane: {order:?}");
    }
    println!("vec0 KNN: OK");

    conn.execute("CREATE TABLE plain(t TEXT)", [])?;
    conn.execute("INSERT INTO plain(t) VALUES ('hello encrypted world')", [])?;
    drop(stmt);
    drop(conn);

    // --- encryption checks ---
    let mut header = [0u8; 16];
    use std::io::Read;
    std::fs::File::open(&db_path)?.read_exact(&mut header)?;
    if header.starts_with(b"SQLite format 3") {
        bail!("DB header is plaintext SQLite — encryption NOT active");
    }
    println!("file header is not plaintext: OK");

    let conn2 = Connection::open(&db_path)?;
    conn2.pragma_update(None, "key", "wrong-key")?;
    match conn2.query_row("SELECT count(*) FROM plain", [], |r| r.get::<_, i64>(0)) {
        Err(_) => println!("wrong key rejected: OK"),
        Ok(_) => bail!("opened with wrong key — encryption NOT active"),
    }
    drop(conn2);

    let conn3 = Connection::open(&db_path)?;
    conn3.pragma_update(None, "key", "spike-test-key")?;
    let n: i64 = conn3.query_row("SELECT count(*) FROM plain", [], |r| r.get(0))?;
    assert_eq!(n, 1);
    println!("correct key reopens: OK");

    println!("\nSTORAGE SPIKE: ALL PASS");
    Ok(())
}
