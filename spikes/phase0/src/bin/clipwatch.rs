//! Spike leg 3: clipboard listener + conceal-format detection.
//!
//! Creates a hidden window, registers AddClipboardFormatListener, and on every
//! WM_CLIPBOARDUPDATE enumerates the formats on the clipboard, printing whether
//! the standard "don't record me" formats are present:
//!   - ExcludeClipboardContentFromMonitorProcessing
//!   - CanIncludeInClipboardHistory (value 0 means "don't")
//!   - CanUploadToCloudClipboard
//!
//! Usage: clipwatch [seconds]   (default 30; exits after the timeout)
//! Test:  run clipset in another terminal, and/or copy from a password manager.

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::System::DataExchange::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;

const CONCEAL_FORMATS: &[&str] = &[
    "ExcludeClipboardContentFromMonitorProcessing",
    "CanIncludeInClipboardHistory",
    "CanUploadToCloudClipboard",
];

fn format_name(fmt: u32) -> String {
    // predefined formats have no registered name
    match fmt {
        1 => return "CF_TEXT".into(),
        2 => return "CF_BITMAP".into(),
        8 => return "CF_DIB".into(),
        13 => return "CF_UNICODETEXT".into(),
        16 => return "CF_LOCALE".into(),
        17 => return "CF_DIBV5".into(),
        _ => {}
    }
    let mut buf = [0u16; 256];
    let n = unsafe { GetClipboardFormatNameW(fmt, &mut buf) };
    if n > 0 {
        String::from_utf16_lossy(&buf[..n as usize])
    } else {
        format!("#{fmt}")
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    if msg == WM_CLIPBOARDUPDATE {
        println!("\n-- clipboard updated --");
        if OpenClipboard(hwnd).is_ok() {
            let mut fmt = 0u32;
            let mut names = Vec::new();
            loop {
                fmt = EnumClipboardFormats(fmt);
                if fmt == 0 {
                    break;
                }
                names.push(format_name(fmt));
            }
            let _ = CloseClipboard();
            println!("formats: {names:?}");
            let concealed: Vec<&&str> = CONCEAL_FORMATS
                .iter()
                .filter(|c| names.iter().any(|n| n.eq_ignore_ascii_case(c)))
                .collect();
            if concealed.is_empty() {
                println!("verdict: CAPTURE (no conceal formats)");
            } else {
                println!("verdict: DROP — conceal formats present: {concealed:?}");
            }
        } else {
            println!("(could not open clipboard to enumerate)");
        }
        return LRESULT(0);
    }
    DefWindowProcW(hwnd, msg, w, l)
}

fn main() -> Result<()> {
    let secs: u64 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(30);
    unsafe {
        let hinst = GetModuleHandleW(None)?;
        let class = w!("phase0_clipwatch");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: hinst.into(),
            lpszClassName: class,
            ..Default::default()
        };
        RegisterClassW(&wc);
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            class,
            w!("clipwatch"),
            WINDOW_STYLE::default(),
            0, 0, 0, 0,
            HWND_MESSAGE, // message-only window: receives WM_CLIPBOARDUPDATE fine
            None,
            hinst,
            None,
        )?;
        AddClipboardFormatListener(hwnd)?;
        println!("listening for clipboard changes for {secs}s — copy things now (try a password manager)...");

        // pump messages, quit after timeout
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(secs);
        let mut msg = MSG::default();
        loop {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            if std::time::Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(30));
        }
        let _ = RemoveClipboardFormatListener(hwnd);
    }
    println!("\nCLIPWATCH SPIKE: done (verdicts above)");
    Ok(())
}
