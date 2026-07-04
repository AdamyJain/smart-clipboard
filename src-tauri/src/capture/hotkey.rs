//! Alt+C intentional capture (Windows) — extension-free design.
//!
//! One shortcut, every app:
//!   1. snapshot the foreground window: title, exe, screenshot (PNG);
//!   2. browsers only: read the URL from the address bar via UI Automation;
//!   3. synthesize Ctrl+C (releasing the still-held Alt first) and watch
//!      `GetClipboardSequenceNumber` — if it never changes there was no
//!      selection, and we must NOT capture the stale clipboard;
//!   4. queue the event; the pipeline stores the screenshot as an asset,
//!      OCRs it into searchable context, and assigns a session.
//!
//! No selection is not a failure: the OCR text becomes the capture itself,
//! which makes non-selectable content (images, PDFs, videos) capturable.

#![cfg(windows)]

use crate::capture::clipboard::{foreground_app, RawClipboardEvent};
use crate::capture::{screenshot, uia};
use std::sync::mpsc::Sender;
use windows::Win32::System::DataExchange::GetClipboardSequenceNumber;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP,
    VK_C, VK_CONTROL, VK_LMENU, VK_MENU, VK_RMENU,
};
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowTextW};

pub const BROWSERS: &[&str] = &[
    "chrome.exe",
    "msedge.exe",
    "firefox.exe",
    "brave.exe",
    "opera.exe",
    "vivaldi.exe",
];

fn key(vk: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY, up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn send_ctrl_c() {
    // release Alt in all its forms first — the user is still holding it, and
    // the target app would otherwise see Ctrl+Alt+C
    let seq = [
        key(VK_MENU, true),
        key(VK_LMENU, true),
        key(VK_RMENU, true),
        key(VK_CONTROL, false),
        key(VK_C, false),
        key(VK_C, true),
        key(VK_CONTROL, true),
    ];
    unsafe {
        SendInput(&seq, std::mem::size_of::<INPUT>() as i32);
    }
}

pub fn foreground_title() -> Option<String> {
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.0.is_null() {
            return None;
        }
        let mut buf = [0u16; 512];
        let n = GetWindowTextW(hwnd, &mut buf);
        if n > 0 {
            Some(String::from_utf16_lossy(&buf[..n as usize]))
        } else {
            None
        }
    }
}

/// Wait for the target app to service the synthesized Ctrl+C. Returns the
/// fresh selection text, or None if the clipboard never changed (no selection).
fn selection_after_ctrl_c() -> Option<String> {
    let seq0 = unsafe { GetClipboardSequenceNumber() };
    send_ctrl_c();
    for _ in 0..16 {
        std::thread::sleep(std::time::Duration::from_millis(25));
        if unsafe { GetClipboardSequenceNumber() } != seq0 {
            // writer may still hold the clipboard; short retry
            for _ in 0..3 {
                if let Ok(t) = arboard::Clipboard::new().and_then(|mut c| c.get_text()) {
                    return Some(t);
                }
                std::thread::sleep(std::time::Duration::from_millis(40));
            }
            return None;
        }
    }
    None
}

/// Runs on the global-shortcut callback. Spawns a worker thread so the
/// callback returns immediately.
pub fn handle_alt_c(tx: Sender<RawClipboardEvent>) {
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.0.is_null() {
        return;
    }
    let app = foreground_app();
    if app.as_deref() == Some("smart-clipboard.exe") {
        return; // our own palette/HUD focused: nothing to capture
    }
    let title = foreground_title();
    let is_browser = app.as_deref().is_some_and(|a| BROWSERS.iter().any(|b| *b == a));
    // raw HWND is not Send; move the isize across the thread boundary
    let hwnd_raw = hwnd.0 as isize;

    std::thread::spawn(move || {
        let hwnd = windows::Win32::Foundation::HWND(hwnd_raw as *mut _);
        // context first, while the window looks exactly as the user sees it
        let screenshot_png = screenshot::capture_window_png(hwnd);
        let source_url = if is_browser { uia::browser_url(hwnd) } else { None };

        let text = selection_after_ctrl_c().unwrap_or_default();
        if text.trim().is_empty() && screenshot_png.is_none() {
            eprintln!("[hotkey] Alt+C: no selection and no screenshot — nothing to capture");
            return;
        }
        let _ = tx.send(RawClipboardEvent {
            text,
            source_app: app,
            origin: Some("hotkey".into()),
            window_title: title,
            source_url,
            screenshot_png,
            ..Default::default()
        });
    });
}
