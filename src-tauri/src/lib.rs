pub mod capture;
pub mod db;
pub mod mcp;
pub mod pipeline;
pub mod search;
pub mod sessions;

use anyhow::Result;
use fastembed::TextEmbedding;
use rusqlite::Connection;
use search::retrieval::SearchHit;
use std::sync::{mpsc, Arc, Mutex, RwLock};
use tauri::{Emitter, Manager};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Config {
    pub excluded_apps: Vec<String>,
    pub palette_shortcut: String,
    pub capture_shortcut: String,
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
            capture_shortcut: "Alt+C".into(),
        }
    }
}

/// Same location Tauri's `app_data_dir()` resolves to on Windows — used by
/// launcher modes (`--mcp`) that run without a Tauri app handle.
pub fn default_data_dir() -> std::path::PathBuf {
    let base = std::env::var("APPDATA").expect("APPDATA not set");
    std::path::PathBuf::from(base).join("com.adamy.smart-clipboard")
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

#[derive(serde::Serialize)]
pub struct SessionRow {
    id: String,
    topic: Option<String>,
    status: String,
    started_at: i64,
    last_activity_at: i64,
    capture_count: i64,
}

#[tauri::command]
fn list_sessions(state: tauri::State<'_, AppState>) -> Result<Vec<SessionRow>, String> {
    let conn = state.conn.lock().unwrap();
    let mut stmt = conn
        .prepare(
            "SELECT s.id, s.topic, s.status, s.started_at, s.last_activity_at,
                    (SELECT count(*) FROM captures c WHERE c.session_id = s.id AND c.deleted_at IS NULL)
             FROM sessions s WHERE s.deleted_at IS NULL
             ORDER BY (s.status = 'open') DESC, s.last_activity_at DESC LIMIT 30",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok(SessionRow {
                id: r.get(0)?,
                topic: r.get(1)?,
                status: r.get(2)?,
                started_at: r.get(3)?,
                last_activity_at: r.get(4)?,
                capture_count: r.get(5)?,
            })
        })
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();
    Ok(rows)
}

#[tauri::command]
fn session_captures(
    state: tauri::State<'_, AppState>,
    session_id: String,
) -> Result<Vec<SearchHit>, String> {
    let conn = state.conn.lock().unwrap();
    let mut stmt = conn
        .prepare(
            "SELECT id, raw_text, entity_type, source_app, captured_at FROM captures
             WHERE session_id = ?1 AND deleted_at IS NULL AND sensitivity != 'secret'
             ORDER BY captured_at ASC",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([&session_id], |r| {
            Ok(SearchHit {
                id: r.get(0)?,
                raw_text: r.get(1)?,
                entity_type: r.get(2)?,
                source_app: r.get(3)?,
                captured_at: r.get(4)?,
                score: 0.0,
                matched_by: "session".into(),
            })
        })
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();
    Ok(rows)
}

#[tauri::command]
fn rename_session(
    state: tauri::State<'_, AppState>,
    session_id: String,
    topic: String,
) -> Result<(), String> {
    let conn = state.conn.lock().unwrap();
    conn.execute(
        "UPDATE sessions SET topic = ?1 WHERE id = ?2",
        rusqlite::params![topic, session_id],
    )
    .map_err(|e| e.to_string())?;
    sessions::log_correction(&conn, "rename", Some(&session_id), None, None)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn merge_sessions(
    state: tauri::State<'_, AppState>,
    from_id: String,
    to_id: String,
) -> Result<(), String> {
    let conn = state.conn.lock().unwrap();
    conn.execute(
        "UPDATE captures SET session_id = ?1 WHERE session_id = ?2",
        rusqlite::params![to_id, from_id],
    )
    .map_err(|e| e.to_string())?;
    conn.execute(
        "UPDATE sessions SET status = 'closed', ended_at = ?1, deleted_at = ?1 WHERE id = ?2",
        rusqlite::params![db::now_ms(), from_id],
    )
    .map_err(|e| e.to_string())?;
    sessions::log_correction(&conn, "merge", Some(&from_id), Some(&to_id), None)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn reassign_capture(
    state: tauri::State<'_, AppState>,
    capture_id: String,
    to_session: Option<String>,
) -> Result<(), String> {
    let conn = state.conn.lock().unwrap();
    let from: Option<String> = conn
        .query_row(
            "SELECT session_id FROM captures WHERE id = ?1",
            [&capture_id],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    conn.execute(
        "UPDATE captures SET session_id = ?1 WHERE id = ?2",
        rusqlite::params![to_session, capture_id],
    )
    .map_err(|e| e.to_string())?;
    let kind = if to_session.is_some() { "reassign" } else { "remove" };
    sessions::log_correction(&conn, kind, from.as_deref(), to_session.as_deref(), Some(&capture_id))
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn close_session(state: tauri::State<'_, AppState>, session_id: String) -> Result<(), String> {
    let conn = state.conn.lock().unwrap();
    conn.execute(
        "UPDATE sessions SET status = 'closed', ended_at = ?1 WHERE id = ?2",
        rusqlite::params![db::now_ms(), session_id],
    )
    .map_err(|e| e.to_string())?;
    sessions::log_correction(&conn, "close", Some(&session_id), None, None)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn delete_capture(state: tauri::State<'_, AppState>, capture_id: String) -> Result<(), String> {
    let conn = state.conn.lock().unwrap();
    db::delete_capture(&conn, &capture_id).map_err(|e| e.to_string())
}

#[tauri::command]
fn delete_all_captures(state: tauri::State<'_, AppState>) -> Result<usize, String> {
    let conn = state.conn.lock().unwrap();
    db::delete_all_captures(&conn).map_err(|e| e.to_string())
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
            let hotkey_tx = cap_tx.clone();
            let gates = capture::clipboard::GateConfig {
                excluded_apps: config.excluded_apps.clone(),
            };
            std::thread::spawn(move || capture::clipboard::listener_thread(cap_tx, gates));

            let pipe_db = db_path.clone();
            let pipe_data_dir = data_dir.clone();
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
                    let intentional = ev.origin.as_deref().is_some_and(|o| o != "ambient");

                    // Alt+C screenshots: store asset + OCR. With a selection
                    // the OCR text becomes searchable context; without one it
                    // becomes the capture itself (non-selectable content).
                    let mut asset_id = None;
                    let mut ocr_context = None;
                    let mut raw_text = ev.text;
                    if let Some(png) = &ev.screenshot_png {
                        match pipeline::assets::store_screenshot(&conn, &pipe_data_dir, png) {
                            Ok(asset) => {
                                asset_id = Some(asset.id);
                                if raw_text.trim().is_empty() {
                                    raw_text = asset.ocr_text.unwrap_or_default();
                                } else {
                                    ocr_context = asset
                                        .ocr_text
                                        .map(|t| t.chars().take(4000).collect::<String>());
                                }
                            }
                            Err(e) => eprintln!("[pipeline] asset store failed: {e:#}"),
                        }
                    }

                    let incoming = pipeline::fast_tier::IncomingCapture {
                        raw_text,
                        source_app: ev.source_app,
                        origin: ev.origin,
                        window_title: ev.window_title,
                        source_url: ev.source_url,
                        context_before: ev.context_before.or(ocr_context),
                        context_after: ev.context_after,
                        asset_id,
                    };
                    match pipeline::fast_tier::process(&conn, incoming) {
                        Ok(Some(stored)) => {
                            let _ = wake_tx.send(());
                            // intentional captures join a session (FR5);
                            // secrets stay sessionless by design
                            let mut session_topic: Option<String> = None;
                            if intentional && stored.sensitivity != "secret" {
                                let mut cfg = sessions::scoring::ScoringConfig::default();
                                cfg.threshold =
                                    sessions::adaptive::current_threshold(&conn, cfg.threshold);
                                let (app, url): (Option<String>, Option<String>) = conn
                                    .query_row(
                                        "SELECT source_app, source_url FROM captures WHERE id = ?1",
                                        [&stored.id],
                                        |r| Ok((r.get(0)?, r.get(1)?)),
                                    )
                                    .unwrap_or((None, None));
                                match sessions::scoring::assign(
                                    &conn,
                                    &cfg,
                                    &stored.id,
                                    stored.captured_at,
                                    app.as_deref(),
                                    url.as_deref(),
                                ) {
                                    Ok(a) => session_topic = Some(a.topic),
                                    Err(e) => eprintln!("[pipeline] session assign: {e:#}"),
                                }
                            }
                            let payload = serde_json::json!({
                                "id": stored.id,
                                "entity_type": stored.entity_type,
                                "sensitivity": stored.sensitivity,
                                "preview": stored.preview,
                                "deduped": stored.deduped,
                                "session_topic": session_topic,
                            });
                            let _ = app_handle.emit("capture", &payload);
                            show_hud(&app_handle);
                        }
                        Ok(None) => {}
                        Err(e) => eprintln!("[pipeline] {e:#}"),
                    }
                }
            });

            // ---- session sweep (idle close + nightly tuning) ----
            let sweep_db = db_path.clone();
            std::thread::spawn(move || match db::open(&sweep_db) {
                Ok(conn) => sessions::sweep::sweep_loop(conn),
                Err(e) => eprintln!("[sessions] sweep db open failed: {e:#}"),
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

            // ---- Alt+C intentional capture ----
            let capture_sc: Shortcut = config.capture_shortcut.parse()
                .unwrap_or_else(|_| "Alt+C".parse().unwrap());
            app.global_shortcut().on_shortcut(capture_sc, move |_app, _sc, event| {
                if event.state() == ShortcutState::Pressed {
                    capture::hotkey::handle_alt_c(hotkey_tx.clone());
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
        .invoke_handler(tauri::generate_handler![
            search,
            copy_to_clipboard,
            hide_window,
            list_sessions,
            session_captures,
            rename_session,
            merge_sessions,
            reassign_capture,
            close_session,
            delete_capture,
            delete_all_captures
        ])
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
