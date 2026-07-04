//! OS-native OCR (Windows.Media.Ocr) — local, free, private. Good enough for
//! search recall over screenshots; not for verbatim code extraction (that is
//! what the synthesized-copy leg of Alt+C is for).

#![cfg(windows)]

use anyhow::{bail, Context, Result};
use windows::core::HSTRING;
use windows::Graphics::Imaging::BitmapDecoder;
use windows::Media::Ocr::OcrEngine;
use windows::Storage::{FileAccessMode, StorageFile};

/// OCR a PNG file on disk (assets are written there anyway). Blocking; call
/// from a worker thread. ~40ms warm on Windows 10 (phase-0 measurement).
pub fn ocr_png_file(path: &std::path::Path) -> Result<String> {
    let abs = std::fs::canonicalize(path)?;
    // WinRT APIs reject the \\?\ extended-length prefix canonicalize adds
    let path_str = abs.to_string_lossy().trim_start_matches(r"\\?\").to_string();

    let file = StorageFile::GetFileFromPathAsync(&HSTRING::from(&path_str))
        .context("open storage file")?
        .get()?;
    let stream = file.OpenAsync(FileAccessMode::Read)?.get()?;
    let decoder = BitmapDecoder::CreateAsync(&stream)?.get()?;
    let bitmap = decoder.GetSoftwareBitmapAsync()?.get()?;

    let engine = match OcrEngine::TryCreateFromUserProfileLanguages() {
        Ok(e) => e,
        Err(e) => bail!("no OCR engine for user profile languages: {e}"),
    };
    let result = engine.RecognizeAsync(&bitmap)?.get()?;
    Ok(result.Text()?.to_string())
}
