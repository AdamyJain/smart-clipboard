//! Debug: dump capture rows from the real (encrypted) DB.
//! Usage: cargo run --example dump

fn main() -> anyhow::Result<()> {
    smart_clipboard_lib::db::register_vec_extension();
    let data_dir = dirs_path();
    let conn = smart_clipboard_lib::db::open(&data_dir.join("smart-clipboard.db"))?;
    let mut stmt = conn.prepare(
        "SELECT id, entity_type, sensitivity, embedded, substr(replace(raw_text, char(10), ' '), 1, 40), source_app
         FROM captures WHERE deleted_at IS NULL ORDER BY captured_at DESC LIMIT 20",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, i64>(3)?,
            r.get::<_, String>(4)?,
            r.get::<_, Option<String>>(5)?,
        ))
    })?;
    println!("{:<8} {:<12} {:<8} {:<4} {:<42} {}", "id…", "entity", "sens", "emb", "text", "app");
    for row in rows {
        let (id, et, sens, emb, txt, app) = row?;
        println!("{:<8} {:<12} {:<8} {:<4} {:<42} {}", &id[..8], et, sens, emb, txt, app.unwrap_or_default());
    }
    let fts_secret: i64 = conn.query_row(
        "SELECT count(*) FROM captures_fts WHERE captures_fts MATCH 'ghp'",
        [], |r| r.get(0),
    )?;
    let vec_count: i64 = conn.query_row("SELECT count(*) FROM captures_vec", [], |r| r.get(0))?;
    let cap_count: i64 = conn.query_row("SELECT count(*) FROM captures WHERE deleted_at IS NULL", [], |r| r.get(0))?;
    println!("\ncaptures={cap_count}  vectors={vec_count}  fts-hits-for-'ghp'={fts_secret} (must be 0)");
    Ok(())
}

fn dirs_path() -> std::path::PathBuf {
    let appdata = std::env::var("APPDATA").expect("APPDATA");
    std::path::PathBuf::from(appdata).join("com.adamy.smart-clipboard")
}
