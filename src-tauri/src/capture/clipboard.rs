//! Ambient clipboard capture (Windows): AddClipboardFormatListener message
//! loop on a dedicated thread. Gate 0 (OS conceal formats) and gate 1 (per-app
//! exclusion list) run HERE, before anything is queued (PRD FR3/FR3a).

#![cfg(windows)]

use std::sync::mpsc::Sender;
use windows::core::w;
use windows::Win32::Foundation::*;
use windows::Win32::System::DataExchange::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION};
use windows::Win32::UI::WindowsAndMessaging::*;

#[derive(Debug, Clone)]
pub struct RawClipboardEvent {
    pub text: String,
    pub source_app: Option<String>,
}

pub struct GateConfig {
    pub excluded_apps: Vec<String>, // lowercase exe names, e.g. "keepass.exe"
}

const CONCEAL_FORMATS: &[&str] = &[
    "ExcludeClipboardContentFromMonitorProcessing",
    "CanIncludeInClipboardHistory",
    "CanUploadToCloudClipboard",
];

// wndproc has no user-data pointer worth the ceremony here; the listener
// thread owns these statics exclusively.
static mut TX: Option<Sender<RawClipboardEvent>> = None;
static mut GATES: Option<GateConfig> = None;

fn format_name(fmt: u32) -> String {
    let mut buf = [0u16; 256];
    let n = unsafe { GetClipboardFormatNameW(fmt, &mut buf) };
    if n > 0 {
        String::from_utf16_lossy(&buf[..n as usize])
    } else {
        String::new()
    }
}

/// Foreground window's process image name, lowercased (e.g. "chrome.exe").
fn foreground_app() -> Option<String> {
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.0.is_null() {
            return None;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == 0 {
            return None;
        }
        let proc = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 512];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(proc, PROCESS_NAME_WIN32, windows::core::PWSTR(buf.as_mut_ptr()), &mut len);
        let _ = CloseHandle(proc);
        ok.ok()?;
        let full = String::from_utf16_lossy(&buf[..len as usize]);
        full.rsplit(['\\', '/']).next().map(|s| s.to_lowercase())
    }
}

/// Open clipboard with a short retry — the copying app often still holds it
/// when WM_CLIPBOARDUPDATE lands (observed in the phase-0 spike).
unsafe fn open_clipboard_retry(hwnd: HWND) -> bool {
    for _ in 0..5 {
        if OpenClipboard(hwnd).is_ok() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(15));
    }
    false
}

unsafe fn read_event(hwnd: HWND) -> Option<RawClipboardEvent> {
    // Gate 1 first — it needs no clipboard access
    let source_app = foreground_app();
    if let (Some(app), Some(gates)) = (&source_app, (*std::ptr::addr_of!(GATES)).as_ref()) {
        if gates.excluded_apps.iter().any(|e| e == app) {
            eprintln!("[capture] gate1 DROP (excluded app: {app})");
            return None;
        }
    }

    if !open_clipboard_retry(hwnd) {
        eprintln!("[capture] could not open clipboard after retries");
        return None;
    }
    let result = (|| {
        // Gate 0: conceal formats
        let mut fmt = 0u32;
        let mut has_text = false;
        loop {
            fmt = EnumClipboardFormats(fmt);
            if fmt == 0 {
                break;
            }
            if fmt == 13 {
                has_text = true; // CF_UNICODETEXT
            }
            let name = format_name(fmt);
            if CONCEAL_FORMATS.iter().any(|c| name.eq_ignore_ascii_case(c)) {
                eprintln!("[capture] gate0 DROP (conceal format: {name})");
                return None;
            }
        }
        if !has_text {
            return None; // image/file payloads: phase 5
        }
        let handle = GetClipboardData(13).ok()?;
        let hglobal = windows::Win32::System::Memory::GlobalLock(
            windows::Win32::Foundation::HGLOBAL(handle.0),
        ) as *const u16;
        if hglobal.is_null() {
            return None;
        }
        let mut len = 0usize;
        while *hglobal.add(len) != 0 {
            len += 1;
        }
        let text = String::from_utf16_lossy(std::slice::from_raw_parts(hglobal, len));
        let _ = windows::Win32::System::Memory::GlobalUnlock(
            windows::Win32::Foundation::HGLOBAL(handle.0),
        );
        Some(RawClipboardEvent { text, source_app })
    })();
    let _ = CloseClipboard();
    result
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    if msg == WM_CLIPBOARDUPDATE {
        if let Some(ev) = read_event(hwnd) {
            if let Some(tx) = (*std::ptr::addr_of!(TX)).as_ref() {
                let _ = tx.send(ev);
            }
        }
        return LRESULT(0);
    }
    DefWindowProcW(hwnd, msg, w, l)
}

/// Blocks forever — run on a dedicated thread.
pub fn listener_thread(tx: Sender<RawClipboardEvent>, gates: GateConfig) {
    unsafe {
        TX = Some(tx);
        GATES = Some(gates);
        let hinst = GetModuleHandleW(None).expect("module handle");
        let class = w!("smart_clipboard_listener");
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
            w!("listener"),
            WINDOW_STYLE::default(),
            0, 0, 0, 0,
            HWND_MESSAGE,
            None,
            hinst,
            None,
        )
        .expect("listener window");
        AddClipboardFormatListener(hwnd).expect("clipboard listener");
        eprintln!("[capture] clipboard listener running");
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}
