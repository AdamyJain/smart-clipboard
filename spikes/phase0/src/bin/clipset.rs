//! Helper for the clipwatch spike: puts text on the clipboard WITH the
//! ExcludeClipboardContentFromMonitorProcessing conceal format set — simulating
//! what a password manager does. Run clipwatch first, then this; clipwatch
//! should print verdict: DROP.

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::System::DataExchange::*;
use windows::Win32::System::Memory::*;

const CF_UNICODETEXT: u32 = 13;

unsafe fn hglobal_from_bytes(bytes: &[u8]) -> Result<HGLOBAL> {
    let h = GlobalAlloc(GMEM_MOVEABLE, bytes.len())?;
    let p = GlobalLock(h) as *mut u8;
    std::ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len());
    let _ = GlobalUnlock(h);
    Ok(h)
}

fn main() -> Result<()> {
    unsafe {
        let exclude_fmt =
            RegisterClipboardFormatW(w!("ExcludeClipboardContentFromMonitorProcessing"));
        OpenClipboard(HWND::default())?;
        EmptyClipboard()?;

        // the "secret" text
        let text: Vec<u8> = "hunter2\0"
            .encode_utf16()
            .flat_map(|u| u.to_le_bytes())
            .collect();
        let htext = hglobal_from_bytes(&text)?;
        SetClipboardData(CF_UNICODETEXT, HANDLE(htext.0))?;

        // the conceal marker (content is irrelevant; presence is the signal —
        // a 4-byte DWORD is what password managers typically set)
        let marker = hglobal_from_bytes(&1u32.to_le_bytes())?;
        SetClipboardData(exclude_fmt, HANDLE(marker.0))?;

        CloseClipboard()?;
    }
    println!("clipboard set with conceal format — check clipwatch output");
    Ok(())
}
