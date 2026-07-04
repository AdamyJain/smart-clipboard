//! Debug: verify Alt+C screenshot assets + OCR context + FTS-on-context.

fn main() -> anyhow::Result<()> {
    smart_clipboard_lib::db::register_vec_extension();
    let appdata = std::env::var("APPDATA")?;
    let dir = std::path::PathBuf::from(appdata).join("com.adamy.smart-clipboard");
    let conn = smart_clipboard_lib::db::open(&dir.join("smart-clipboard.db"))?;

    let mut stmt = conn.prepare(
        "SELECT substr(c.raw_text,1,40), c.asset_id IS NOT NULL,
                substr(ifnull(c.context_before,''),1,60), ifnull(a.file_path,''),
                length(ifnull(a.ocr_text,''))
         FROM captures c LEFT JOIN assets a ON a.id = c.asset_id
         WHERE c.origin = 'hotkey' ORDER BY c.captured_at DESC LIMIT 5",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, bool>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, i64>(4)?,
        ))
    })?;
    for row in rows {
        let (text, has_asset, ctx, path, ocr_len) = row?;
        println!("text={text:<42} asset={has_asset} ocr_len={ocr_len}");
        println!("   ctx={ctx}");
        if !path.is_empty() {
            let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            println!("   file={path} ({size} bytes)");
        }
    }

    // FTS must find the selection capture via a word present ONLY in the OCR
    // context (window chrome text like "Untitled")
    let hits: i64 = conn.query_row(
        "SELECT count(*) FROM captures_fts WHERE captures_fts MATCH 'Untitled'",
        [],
        |r| r.get(0),
    )?;
    println!("\nfts hits for 'Untitled' (context-only word) = {hits} (expect >= 1)");
    Ok(())
}
