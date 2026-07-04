//! Screenshot asset storage: PNG file on disk + `assets` row, then native OCR
//! into `assets.ocr_text`. Note: asset FILES are outside the encrypted DB —
//! documented gap until phase-5 hardening.

use anyhow::Result;
use rusqlite::{params, Connection};
use std::path::Path;

pub struct StoredAsset {
    pub id: String,
    pub ocr_text: Option<String>,
}

/// Write the PNG, insert the row, OCR it (best-effort). Blocking (~40ms OCR).
pub fn store_screenshot(conn: &Connection, data_dir: &Path, png: &[u8]) -> Result<StoredAsset> {
    let id = ulid::Ulid::new().to_string();
    let dir = data_dir.join("assets");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{id}.png"));
    std::fs::write(&path, png)?;

    let ocr_text = match crate::pipeline::ocr::ocr_png_file(&path) {
        Ok(t) if !t.trim().is_empty() => Some(t),
        Ok(_) => None,
        Err(e) => {
            eprintln!("[assets] ocr failed: {e:#}");
            None
        }
    };
    // OCR text that trips the secret detector is dropped from the DB entirely
    // (the pixels may still contain it — phase-5 hardening decides retention)
    let safe_ocr = ocr_text.filter(|t| {
        crate::pipeline::secrets::detect(t) != crate::pipeline::secrets::Sensitivity::Secret
    });

    conn.execute(
        "INSERT INTO assets (id, file_path, ocr_text, ocr_source) VALUES (?1, ?2, ?3, 'native')",
        params![id, path.to_string_lossy(), safe_ocr],
    )?;
    Ok(StoredAsset { id, ocr_text: safe_ocr })
}
