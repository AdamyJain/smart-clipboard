pub mod capture;
pub mod db;
pub mod pipeline;
pub mod search;

use anyhow::Result;
use fastembed::TextEmbedding;
use rusqlite::Connection;
use search::retrieval::SearchHit;
use std::sync::{mpsc, Arc, Mutex, RwLock};
use tauri::{Emitter, Manager};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Config {
    pub excluded_apps: Vec<String>,
    pub palette_shortcut: String,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            excluded_apps: vec![
                "keepass.exe".into(),
                "keepassxc.exe".into(),
                "1password.exe".into(),
                "bitwarden.exe".into(),
            ],
            palette_shortcut: "Alt+Space".into(),
        }
    }
}

pub struct AppState {
    pub conn: Mutex<Connection>,
    // None until the model finishes loading on its background thread
    pub model: Arc<RwLock<Option<Arc<TextEmbedding>>>>,
}

#[tauri::command]
fn search(
    state: tauri::State<'_, AppState>,
    query: String,
    entity_filter: Option<String>,
) -> Result<Vec<SearchHit>, String> {
    let conn = state.conn.lock().unwrap();
    if query.trim().is_empty() {
        return search::retrieval::recent(&conn, 30).map_err(|e| e.to_string());
    }
    let model_slot = state.model.read().unwrap();
    match model_slot.as_ref() {
        Some(model) => {
            search::retrieval::search(&conn, model, &query, entity_filter.as_deref(), 30)
                .map_err(|e| e.to_string())
        }
        // model still warming up: degrade gracefully to FTS-only via LIKE
        None => search::retrieval::recent(&conn, 30)
            .map(|hits| {
                hits.into_iter()
                    .filter(|h| h.raw_text.to_lowercase().contains(&query.to_lowercase()))
                    .collect()
            })
            .map_err(|e| e.to_string()),
    }
}

#[tauri::command]
fn copy_to_clipboard(text: String) -> Result<(), String> {
    let mut cb = arboard::Clipboard::new().map_err(|e| e.to_string())?;
    cb.set_text(text).map_err(|e| e.to_string())
}

#[tauri::command]
fn hide_window(window: tauri::WebviewWindow) {
    let _ = window.hide();
}

fn load_config(dir: &std::path::Path) -> Config {
    let path = dir.join("config.json");
    if let Ok(bytes) = std::fs::read(&path) {
        if let Ok(cfg) = serde_json::from_slice(&bytes) {
            return cfg;
        }
    }
    let cfg = Config::default();
    let _ = std::fs::write(&path, serde_json::to_vec_pretty(&cfg).unwrap());
    cfg
}

pub fn run() {
    db::register_vec_extension();

    tauri::Builder::default()
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            let config = load_config(&data_dir);
            let db_path = data_dir.join("smart-clipboard.db");

            // UI-facing connection
            let ui_conn = db::open(&db_path)?;
            let model_slot: Arc<RwLock<Option<Arc<TextEmbedding>>>> =
                Arc::new(RwLock::new(None));
            app.manage(AppState {
                conn: Mutex::new(ui_conn),
                model: model_slot.clone(),
            });

            // ---- embedding model loads on its own thread (15s on first run) ----
            let model_cache = data_dir.join("models");
            let model_for_worker: Arc<RwLock<Option<Arc<TextEmbedding>>>> = model_slot.clone();
            let (wake_tx, wake_rx) = mpsc::channel::<()>();
            let embed_db = db_path.clone();
            std::thread::spawn(move || {
                match pipeline::embed::load_model(model_cache) {
                    Ok(m) => {
                        let m = Arc::new(m);
                        *model_for_worker.write().unwrap() = Some(m.clone());
                        eprintln!("[embed] model ready");
                        match db::open(&embed_db) {
                            Ok(conn) => pipeline::embed::worker_loop_shared(conn, m, wake_rx),
                            Err(e) => eprintln!("[embed] worker db open failed: {e:#}"),
                        }
                    }
                    Err(e) => eprintln!("[embed] model load failed: {e:#}"),
                }
            });

            // ---- clipboard listener + fast-tier pipeline ----
            let (cap_tx, cap_rx) = mpsc::channel::<capture::clipboard::RawClipboardEvent>();
            let gates = capture::clipboard::GateConfig {
                excluded_apps: config.excluded_apps.clone(),
            };
            std::thread::spawn(move || capture::clipboard::listener_thread(cap_tx, gates));

            let pipe_db = db_path.clone();
            let app_handle = app.handle().clone();
            std::thread::spawn(move || {
                let conn = match db::open(&pipe_db) {
                    Ok(c) => c,
                    Err(e) => {
                        eprintln!("[pipeline] db open failed: {e:#}");
                        return;
                    }
                };
                for ev in cap_rx {
                    let incoming = pipeline::fast_tier::IncomingCapture {
                        raw_text: ev.text,
                        source_app: ev.source_app,
                    };
                    match pipeline::fast_tier::process(&conn, incoming) {
                        Ok(Some(stored)) => {
                            let _ = wake_tx.send(());
                            let _ = app_handle.emit("capture", &stored);
                            show_hud(&app_handle);
                        }
                        Ok(None) => {}
                        Err(e) => eprintln!("[pipeline] {e:#}"),
                    }
                }
            });

            // ---- palette global shortcut ----
            use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};
            let shortcut: Shortcut = config.palette_shortcut.parse()
                .unwrap_or_else(|_| "Alt+Space".parse().unwrap());
            app.global_shortcut().on_shortcut(shortcut, |app, _sc, event| {
                if event.state() == ShortcutState::Pressed {
                    if let Some(win) = app.get_webview_window("main") {
                        if win.is_visible().unwrap_or(false) {
                            let _ = win.hide();
                        } else {
                            let _ = win.show();
                            let _ = win.set_focus();
                            let _ = app.emit("palette-opened", ());
                        }
                    }
                }
            })?;

            // ---- tray ----
            use tauri::menu::{Menu, MenuItem};
            use tauri::tray::TrayIconBuilder;
            let open = MenuItem::with_id(app, "open", "Open palette", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&open, &quit])?;
            TrayIconBuilder::new()
                .icon(app.default_window_icon().unwrap().clone())
                .menu(&menu)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "open" => {
                        if let Some(win) = app.get_webview_window("main") {
                            let _ = win.show();
                            let _ = win.set_focus();
                        }
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .build(app)?;

            Ok(())
        })
        // closing the window hides it — capture keeps running (NFR: background
        // work never depends on the UI)
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main" {
                    let _ = window.hide();
                    api.prevent_close();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![search, copy_to_clipboard, hide_window])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

fn show_hud(app: &tauri::AppHandle) {
    if let Some(hud) = app.get_webview_window("hud") {
        // bottom-right of the primary work area
        if let Ok(Some(monitor)) = hud.primary_monitor() {
            let size = monitor.size();
            let scale = monitor.scale_factor();
            let (w, h) = (340.0 * scale, 76.0 * scale);
            let _ = hud.set_position(tauri::PhysicalPosition::new(
                (size.width as f64 - w - 16.0 * scale) as i32,
                (size.height as f64 - h - 56.0 * scale) as i32,
            ));
        }
        let _ = hud.show();
    }
}
