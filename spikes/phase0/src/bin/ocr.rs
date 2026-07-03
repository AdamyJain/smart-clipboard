//! Spike leg 4: Windows.Media.Ocr on a PNG (Windows 10 built-in OCR, no cloud).
//!
//! Usage: ocr <path-to-png>

use anyhow::{bail, Result};
use windows::core::HSTRING;
use windows::Graphics::Imaging::BitmapDecoder;
use windows::Media::Ocr::OcrEngine;
use windows::Storage::{FileAccessMode, StorageFile};

fn main() -> Result<()> {
    let arg = match std::env::args().nth(1) {
        Some(a) => a,
        None => bail!("usage: ocr <path-to-png>"),
    };
    let abs = std::fs::canonicalize(&arg)?;
    // WinRT APIs reject the \\?\ extended-length prefix canonicalize adds
    let path = abs.to_string_lossy().trim_start_matches(r"\\?\").to_string();

    let file = StorageFile::GetFileFromPathAsync(&HSTRING::from(&path))?.get()?;
    let stream = file.OpenAsync(FileAccessMode::Read)?.get()?;
    let decoder = BitmapDecoder::CreateAsync(&stream)?.get()?;
    let bitmap = decoder.GetSoftwareBitmapAsync()?.get()?;

    let engine = match OcrEngine::TryCreateFromUserProfileLanguages() {
        Ok(e) => e,
        Err(e) => bail!("no OCR engine for user profile languages: {e}"),
    };
    let t = std::time::Instant::now();
    let result = engine.RecognizeAsync(&bitmap)?.get()?;
    let elapsed = t.elapsed();

    let text = result.Text()?.to_string();
    println!("--- OCR text ({} chars, {elapsed:?}) ---", text.len());
    println!("{text}");
    if text.trim().is_empty() {
        bail!("OCR returned empty text");
    }
    println!("\nOCR SPIKE: PASS");
    Ok(())
}
