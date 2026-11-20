mod db;

use rusqlite::Connection;
use serde::Serialize;
use std::path::PathBuf;
use std::sync::Mutex;
use tauri::image::Image;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, State, WindowEvent};
use tauri_plugin_autostart::ManagerExt;

const TRAY_ID: &str = "klip-tray";
const ICON_TRAY_NORMAL: &[u8] = include_bytes!("../icons/tray.png");
const ICON_TRAY_PAUSED: &[u8] = include_bytes!("../icons/tray-paused.png");

pub struct AppState {
    pub conn: Mutex<Connection>,
    pub db_path: PathBuf,
    pub images_dir: PathBuf,
    pub paused: Mutex<bool>,
    pub pause_menu_item: Mutex<Option<MenuItem<tauri::Wry>>>,
}

#[derive(Serialize)]
pub struct Phase1Status {
    pub version: String,
    pub db_path: String,
    pub images_dir: String,
    pub tables: Vec<String>,
    pub paused: bool,
}

fn show_main_window(app: &AppHandle) {
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.unminimize();
        let _ = win.show();
        let _ = win.set_focus();
    }
}

fn apply_pause_state(app: &AppHandle, state: &AppState, new_paused: bool) -> Result<(), String> {
    {
        let mut guard = state.paused.lock().map_err(|_| "Paused lock poisoned".to_string())?;
        *guard = new_paused;
    }

    {
        let conn = state.conn.lock().map_err(|_| "DB lock poisoned".to_string())?;
        db::set_setting(&conn, "paused", if new_paused { "true" } else { "false" })?;
    }

    if let Some(item) = state
        .pause_menu_item
        .lock()
        .map_err(|_| "Menu lock poisoned".to_string())?
        .as_ref()
    {
        let label = if new_paused {
            "Resume Saving"
        } else {
            "Pause Saving"
        };
        let _ = item.set_text(label);
    }

    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        let icon_bytes = if new_paused {
            ICON_TRAY_PAUSED
        } else {
            ICON_TRAY_NORMAL
        };
        if let Ok(img) = Image::from_bytes(icon_bytes) {
            let _ = tray.set_icon(Some(img));
        }
        let tooltip = if new_paused {
            "Klip â€” Saving Paused"
        } else {
            "Klip â€” Clipboard Manager"
        };
        let _ = tray.set_tooltip(Some(tooltip));
    }

    let _ = app.emit("klip://paused-changed", new_paused);
    Ok(())
}

#[tauri::command]
fn get_phase1_status(app: AppHandle, state: State<'_, AppState>) -> Result<Phase1Status, String> {
    let tables = {
        let conn = state.conn.lock().map_err(|_| "DB lock poisoned".to_string())?;
        db::list_tables(&conn)?
    };
    let paused = *state.paused.lock().map_err(|_| "Paused lock poisoned".to_string())?;

    Ok(Phase1Status {
        version: app.package_info().version.to_string(),
        db_path: state.db_path.to_string_lossy().into_owned(),
        images_dir: state.images_dir.to_string_lossy().into_owned(),
        tables,
        paused,
    })
}

#[tauri::command]
fn toggle_pause(app: AppHandle, state: State<'_, AppState>) -> Result<bool, String> {
    let next_paused = {
        let guard = state.paused.lock().map_err(|_| "Paused lock poisoned".to_string())?;
        !*guard
    };
    apply_pause_state(&app, &state, next_paused)?;
    Ok(next_paused)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let storage = db::resolve_storage_paths().expect("Failed to resolve Klip storage paths");
    let conn = db::open_and_init(&storage.db_path).expect("Failed to initialize Klip SQLite DB");
    let initial_paused = db::get_setting(&conn, "paused")
        .ok()
        .flatten()
        .map(|v| v == "true")
        .unwrap_or(false);

    let initial_autostart_set = db::get_setting(&conn, "autostart_initialized")
        .ok()
        .flatten()
        .is_some();

    let app_state = AppState {
        conn: Mutex::new(conn),
        db_path: storage.db_path,
        images_dir: storage.images_dir,
        paused: Mutex::new(initial_paused),
        pause_menu_item: Mutex::new(None),
    };

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_main_window(app);
        }))
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .manage(app_state)
        .invoke_handler(tauri::generate_handler![get_phase1_status, toggle_pause])
        .on_window_event(|window, event| {
            if window.label() == "main" {
                if let WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .setup(move |app| {
            if !initial_autostart_set {
                let _ = app.autolaunch().enable();
                let state = app.state::<AppState>();
                if let Ok(conn) = state.conn.lock() {
                    let _ = db::set_setting(&conn, "autostart_initialized", "true");
                };
            }

            let open_item = MenuItem::with_id(app, "open", "Open Klip", true, None::<&str>)?;
            let pause_label = if initial_paused {
                "Resume Saving"
            } else {
                "Pause Saving"
            };
            let pause_item = MenuItem::with_id(app, "pause", pause_label, true, None::<&str>)?;
            let sep = PredefinedMenuItem::separator(app)?;
            let quit_item = MenuItem::with_id(app, "quit", "Quit Klip", true, None::<&str>)?;

            {
                let state = app.state::<AppState>();
                if let Ok(mut guard) = state.pause_menu_item.lock() {
                    *guard = Some(pause_item.clone());
                };
            }

            let menu = Menu::with_items(app, &[&open_item, &pause_item, &sep, &quit_item])?;

            let icon_bytes = if initial_paused {
                ICON_TRAY_PAUSED
            } else {
                ICON_TRAY_NORMAL
            };
            let tray_icon = Image::from_bytes(icon_bytes)?;
            let tooltip = if initial_paused {
                "Klip â€” Saving Paused"
            } else {
                "Klip â€” Clipboard Manager"
            };

            TrayIconBuilder::with_id(TRAY_ID)
                .icon(tray_icon)
                .tooltip(tooltip)
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "open" => show_main_window(app),
                    "pause" => {
                        let state = app.state::<AppState>();
                        let next_paused = state.paused.lock().map(|g| !*g).unwrap_or(false);
                        let _ = apply_pause_state(app, &state, next_paused);
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        show_main_window(tray.app_handle());
                    }
                })
                .build(app)?;

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Klip");
}