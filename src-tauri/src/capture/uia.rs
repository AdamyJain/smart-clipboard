//! Browser URL via UI Automation: read the address bar's Value pattern from
//! the foreground browser window. No extension needed; works on
//! Chrome/Edge/Firefox/Brave (they all expose the omnibox as an Edit control).

#![cfg(windows)]

use windows::core::{Interface, VARIANT};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationValuePattern, TreeScope_Descendants,
    UIA_ControlTypePropertyId, UIA_EditControlTypeId, UIA_ValuePatternId,
};

/// Best-effort; returns None on any hiccup. Call from a worker thread.
pub fn browser_url(hwnd: HWND) -> Option<String> {
    unsafe {
        // fine if the thread is already initialized (S_FALSE)
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let auto: IUIAutomation = CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).ok()?;
        let root = auto.ElementFromHandle(hwnd).ok()?;
        let cond = auto
            .CreatePropertyCondition(UIA_ControlTypePropertyId, &VARIANT::from(UIA_EditControlTypeId.0))
            .ok()?;
        let edit = root.FindFirst(TreeScope_Descendants, &cond).ok()?;
        let pattern = edit
            .GetCurrentPattern(UIA_ValuePatternId)
            .ok()?
            .cast::<IUIAutomationValuePattern>()
            .ok()?;
        let raw = pattern.CurrentValue().ok()?.to_string();
        let raw = raw.trim();
        if raw.is_empty() || raw.contains(' ') {
            return None; // empty omnibox or a search phrase, not a URL
        }
        // browsers strip the scheme in the omnibox: "docs.rs/rmcp" or, for
        // local files, a bare drive path "C:/Users/…"
        Some(if raw.contains("://") {
            raw.to_string()
        } else if raw.len() > 2 && raw.as_bytes()[1] == b':' {
            format!("file:///{raw}")
        } else {
            format!("https://{raw}")
        })
    }
}
