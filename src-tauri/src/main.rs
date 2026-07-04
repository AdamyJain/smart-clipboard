#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // `smart-clipboard --mcp`: stdio MCP server instead of the tray app.
    // Works whether or not the tray app is running (shared WAL DB).
    if std::env::args().any(|a| a == "--mcp") {
        if let Err(e) = smart_clipboard_lib::mcp::run_stdio() {
            eprintln!("[mcp] fatal: {e:#}");
            std::process::exit(1);
        }
        return;
    }
    smart_clipboard_lib::run()
}
