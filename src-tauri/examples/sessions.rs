//! Debug: dump sessions + their captures from the real (encrypted) DB.
//! Usage: cargo run --example sessions

fn main() -> anyhow::Result<()> {
    smart_clipboard_lib::db::register_vec_extension();
    let appdata = std::env::var("APPDATA")?;
    let dir = std::path::PathBuf::from(appdata).join("com.adamy.smart-clipboard");
    let conn = smart_clipboard_lib::db::open(&dir.join("smart-clipboard.db"))?;

    let mut stmt = conn.prepare(
        "SELECT s.id, ifnull(s.topic,''), s.status,
                (SELECT count(*) FROM captures c WHERE c.session_id = s.id)
         FROM sessions s WHERE s.deleted_at IS NULL ORDER BY s.last_activity_at DESC LIMIT 15",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, i64>(3)?,
        ))
    })?;
    for row in rows {
        let (id, topic, status, n) = row?;
        println!("session {:<10} [{status:<6}] {n} captures — {topic}", &id[..10]);
        let mut cs = conn.prepare(
            "SELECT origin, substr(replace(raw_text, char(10), ' '), 1, 50), ifnull(source_url,'')
             FROM captures WHERE session_id = ?1 ORDER BY captured_at",
        )?;
        let caps = cs.query_map([&id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))
        })?;
        for c in caps {
            let (origin, text, url) = c?;
            println!("   [{origin:<9}] {text:<52} {url}");
        }
    }
    let orphan_secret: i64 = conn.query_row(
        "SELECT count(*) FROM captures WHERE sensitivity='secret' AND session_id IS NOT NULL",
        [],
        |r| r.get(0),
    )?;
    println!("\nsecrets inside sessions = {orphan_secret} (must be 0)");
    Ok(())
}
