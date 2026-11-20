mod clipboard;
mod db;
mod popup;

use clipboard::{normalize_exe_name, write_clip_to_clipboard, ClipboardWatcherHandle};
use db::{ClipItem, SettingEntry};
use popup::paste_clip_to_previous_window;
use rusqlite::Connection;
use serde::Serialize;
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicIsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use tauri::image::Image;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, RunEvent, State, WebviewUrl, WebviewWindowBuilder, WindowEvent};
use tauri_plugin_autostart::ManagerExt;
use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
use winreg::RegKey;

const TRAY_ID: &str = "klip-tray";

const ICON_TRAY_DARK: &[u8] = include_bytes!("../icons/tray-dark.png");
const ICON_TRAY_DARK_PAUSED: &[u8] = include_bytes!("../icons/tray-dark-paused.png");
const ICON_TRAY_LIGHT: &[u8] = include_bytes!("../icons/tray-light.png");
const ICON_TRAY_LIGHT_PAUSED: &[u8] = include_bytes!("../icons/tray-light-paused.png");

const ICON_WIN_DARK: &[u8] = include_bytes!("../icons/128x128.png");
const ICON_WIN_LIGHT: &[u8] = include_bytes!("../icons/icon-light.png");

pub struct AppState {
    pub conn: Mutex<Connection>,
    pub db_path: PathBuf,
    pub images_dir: PathBuf,
    pub app_icons_dir: PathBuf,
    pub paused: Mutex<bool>,
    pub paused_flag: Arc<AtomicBool>,
    pub excluded_apps_cache: Arc<RwLock<Vec<String>>>,
    pub theme: Mutex<String>,
    pub pause_menu_item: Mutex<Option<MenuItem<tauri::Wry>>>,
    pub allow_exit: AtomicBool,
    pub watcher: Mutex<Option<ClipboardWatcherHandle>>,
    pub popup_prev_hwnd: AtomicIsize,
    pub popup_keep_open: AtomicBool,
    pub popup_opened_at_ms: AtomicI64,
}

#[derive(Serialize)]
pub struct Phase1Status {
    pub version: String,
    pub db_path: String,
    pub images_dir: String,
    pub tables: Vec<String>,
    pub paused: bool,
    pub theme: String,
    pub font_family: String,
    pub settings: Vec<SettingEntry>,
}

#[derive(Serialize)]
pub struct PopupInitData {
    pub theme: String,
    pub font_family: String,
    pub paste_mode: String,
    pub paused: bool,
    pub clips: Vec<ClipItem>,
}

fn select_tray_icon_bytes(theme: &str, paused: bool) -> &'static [u8] {
    match (theme, paused) {
        ("light", false) => ICON_TRAY_LIGHT,
        ("light", true) => ICON_TRAY_LIGHT_PAUSED,
        (_, false) => ICON_TRAY_DARK,
        (_, true) => ICON_TRAY_DARK_PAUSED,
    }
}

pub fn select_window_icon_bytes(theme: &str) -> &'static [u8] {
    if theme == "light" {
        ICON_WIN_LIGHT
    } else {
        ICON_WIN_DARK
    }
}

fn sync_tray_visuals(app: &AppHandle, theme: &str, paused: bool) {
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        if let Ok(img) = Image::from_bytes(select_tray_icon_bytes(theme, paused)) {
            let _ = tray.set_icon(Some(img));
        }
        let tooltip = if paused {
            "Klip \u{2014} Saving Paused"
        } else {
            "Klip \u{2014} Clipboard Manager"
        };
        let _ = tray.set_tooltip(Some(tooltip));
    }

    if let Some(win) = app.get_webview_window("main") {
        if let Ok(win_img) = Image::from_bytes(select_window_icon_bytes(theme)) {
            let _ = win.set_icon(win_img);
        }
    }
}

fn open_or_focus_main_window(app: &AppHandle) {
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.unminimize();
        let _ = win.show();
        let _ = win.set_focus();
        return;
    }

    let theme = {
        let state = app.state::<AppState>();
        state
            .theme
            .lock()
            .map(|g| g.clone())
            .unwrap_or_else(|_| "dark".to_string())
    };

    if let Ok(win) = WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
        .title("Klip")
        .inner_size(980.0, 660.0)
        .min_inner_size(680.0, 460.0)
        .center()
        .resizable(true)
        .build()
    {
        if let Ok(win_img) = Image::from_bytes(select_window_icon_bytes(&theme)) {
            let _ = win.set_icon(win_img);
        }
        let _ = win.set_focus();
    }
}

fn apply_pause_state(app: &AppHandle, state: &AppState, new_paused: bool) -> Result<(), String> {
    state.paused_flag.store(new_paused, Ordering::Relaxed);
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

    let theme = state
        .theme
        .lock()
        .map(|g| g.clone())
        .unwrap_or_else(|_| "dark".to_string());
    sync_tray_visuals(app, &theme, new_paused);

    let _ = app.emit("klip://paused-changed", new_paused);
    Ok(())
}

fn clean_font_registry_name(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let base = match trimmed.find(" (") {
        Some(idx) => &trimmed[..idx],
        None => trimmed,
    };
    let primary = base.split('&').next().unwrap_or(base).trim();
    if primary.is_empty() || primary.starts_with('@') {
        return None;
    }

    let mut clean = primary.to_string();
    let suffixes = [
        " Bold Italic",
        " Bold Oblique",
        " SemiBold Italic",
        " Semibold Italic",
        " Light Italic",
        " Medium Italic",
        " Regular",
        " Bold",
        " Italic",
        " Oblique",
    ];
    for sfx in suffixes {
        if clean.len() > sfx.len() && clean.ends_with(sfx) {
            clean.truncate(clean.len() - sfx.len());
            break;
        }
    }

    let final_name = clean.trim().to_string();
    if final_name.is_empty() {
        None
    } else {
        Some(final_name)
    }
}

#[tauri::command]
fn list_installed_fonts() -> Vec<String> {
    let mut set = BTreeSet::new();
    set.insert("Segoe UI".to_string());
    set.insert("Nirmala UI".to_string());

    let subkey_path = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\Fonts";
    let roots = [
        RegKey::predef(HKEY_LOCAL_MACHINE),
        RegKey::predef(HKEY_CURRENT_USER),
    ];

    for root in roots {
        if let Ok(fonts_key) = root.open_subkey(subkey_path) {
            for (name, _) in fonts_key.enum_values().flatten() {
                if let Some(font_name) = clean_font_registry_name(&name) {
                    set.insert(font_name);
                }
            }
        }
    }

    set.into_iter().collect()
}

#[tauri::command]
fn get_phase1_status(app: AppHandle, state: State<'_, AppState>) -> Result<Phase1Status, String> {
    let (tables, settings, font_family) = {
        let conn = state.conn.lock().map_err(|_| "DB lock poisoned".to_string())?;
        let tables = db::list_tables(&conn)?;
        let settings = db::list_settings(&conn)?;
        let font_family = db::get_setting(&conn, "font_family")?
            .unwrap_or_else(|| "Segoe UI, Nirmala UI, sans-serif".to_string());
        (tables, settings, font_family)
    };
    let paused = *state.paused.lock().map_err(|_| "Paused lock poisoned".to_string())?;
    let theme = state
        .theme
        .lock()
        .map(|g| g.clone())
        .unwrap_or_else(|_| "dark".to_string());

    Ok(Phase1Status {
        version: app.package_info().version.to_string(),
        db_path: state.db_path.to_string_lossy().into_owned(),
        images_dir: state.images_dir.to_string_lossy().into_owned(),
        tables,
        paused,
        theme,
        font_family,
        settings,
    })
}

#[tauri::command]
fn get_popup_init_data(state: State<'_, AppState>) -> Result<PopupInitData, String> {
    let (theme, font_family, paste_mode, clips) = {
        let conn = state.conn.lock().map_err(|_| "DB lock poisoned".to_string())?;
        let theme = db::get_setting(&conn, "theme")?.unwrap_or_else(|| "dark".to_string());
        let font_family = db::get_setting(&conn, "font_family")?
            .unwrap_or_else(|| "Segoe UI, Nirmala UI, sans-serif".to_string());
        let paste_mode = db::get_setting(&conn, "paste_mode")?
            .unwrap_or_else(|| "auto_paste".to_string());
        let clips = db::list_clips(&conn, None, false, 150)?;
        (theme, font_family, paste_mode, clips)
    };
    let paused = state.paused_flag.load(Ordering::Relaxed);

    Ok(PopupInitData {
        theme,
        font_family,
        paste_mode,
        paused,
        clips,
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

#[tauri::command]
fn set_theme(app: AppHandle, state: State<'_, AppState>, theme: String) -> Result<String, String> {
    let normalized = if theme == "light" { "light" } else { "dark" }.to_string();

    {
        let mut guard = state.theme.lock().map_err(|_| "Theme lock poisoned".to_string())?;
        *guard = normalized.clone();
    }
    {
        let conn = state.conn.lock().map_err(|_| "DB lock poisoned".to_string())?;
        db::set_setting(&conn, "theme", &normalized)?;
    }

    let paused = *state.paused.lock().map_err(|_| "Paused lock poisoned".to_string())?;
    sync_tray_visuals(&app, &normalized, paused);
    let _ = app.emit("klip://theme-changed", normalized.clone());

    Ok(normalized)
}

#[tauri::command]
fn set_font_family(state: State<'_, AppState>, font_family: String) -> Result<(), String> {
    let conn = state.conn.lock().map_err(|_| "DB lock poisoned".to_string())?;
    db::set_setting(&conn, "font_family", font_family.trim())
}

#[tauri::command]
fn list_clips(
    state: State<'_, AppState>,
    search: Option<String>,
    pinned_only: Option<bool>,
    limit: Option<i64>,
) -> Result<Vec<ClipItem>, String> {
    let conn = state.conn.lock().map_err(|_| "DB lock poisoned".to_string())?;
    db::list_clips(
        &conn,
        search.as_deref(),
        pinned_only.unwrap_or(false),
        limit.unwrap_or(200),
    )
}

#[tauri::command]
fn copy_clip(
    state: State<'_, AppState>,
    clip_id: i64,
    plain_only: Option<bool>,
) -> Result<(), String> {
    let item = {
        let conn = state.conn.lock().map_err(|_| "DB lock poisoned".to_string())?;
        db::get_clip_by_id(&conn, clip_id)?
            .ok_or_else(|| "Clip not found".to_string())?
    };
    write_clip_to_clipboard(&item, plain_only.unwrap_or(false))
}

#[tauri::command]
fn paste_clip(
    app: AppHandle,
    state: State<'_, AppState>,
    clip_id: i64,
    plain_only: Option<bool>,
) -> Result<(), String> {
    let item = {
        let conn = state.conn.lock().map_err(|_| "DB lock poisoned".to_string())?;
        db::get_clip_by_id(&conn, clip_id)?
            .ok_or_else(|| "Clip not found".to_string())?
    };
    paste_clip_to_previous_window(&app, &item, plain_only.unwrap_or(false))
}

#[tauri::command]
fn close_popup(app: AppHandle) {
    if let Some(win) = app.get_webview_window("popup") {
        let _ = win.close();
    }
}

#[tauri::command]
fn set_popup_keep_open(state: State<'_, AppState>, keep_open: bool) {
    state.popup_keep_open.store(keep_open, Ordering::SeqCst);
}

#[tauri::command]
fn toggle_pin_clip(app: AppHandle, state: State<'_, AppState>, clip_id: i64) -> Result<bool, String> {
    let pinned = {
        let conn = state.conn.lock().map_err(|_| "DB lock poisoned".to_string())?;
        db::toggle_pin(&conn, clip_id)?
    };
    let _ = app.emit("klip://clip-pinned", (clip_id, pinned));
    Ok(pinned)
}

#[tauri::command]
fn delete_clip(app: AppHandle, state: State<'_, AppState>, clip_id: i64) -> Result<(), String> {
    {
        let conn = state.conn.lock().map_err(|_| "DB lock poisoned".to_string())?;
        db::delete_clip(&conn, clip_id)?;
    }
    let _ = app.emit("klip://clip-deleted", clip_id);
    Ok(())
}

#[tauri::command]
fn get_excluded_apps(state: State<'_, AppState>) -> Result<Vec<String>, String> {
    let conn = state.conn.lock().map_err(|_| "DB lock poisoned".to_string())?;
    let raw = db::get_setting(&conn, "excluded_apps")?.unwrap_or_else(|| "[]".to_string());
    Ok(serde_json::from_str(&raw).unwrap_or_default())
}

#[tauri::command]
fn set_excluded_apps(state: State<'_, AppState>, apps: Vec<String>) -> Result<Vec<String>, String> {
    let mut cleaned: Vec<String> = Vec::new();
    for item in apps {
        let norm = normalize_exe_name(&item);
        if !norm.is_empty() && !cleaned.iter().any(|existing| existing == &norm) {
            cleaned.push(norm);
        }
    }
    let json = serde_json::to_string(&cleaned).map_err(|e| format!("JSON error: {e}"))?;
    {
        let conn = state.conn.lock().map_err(|_| "DB lock poisoned".to_string())?;
        db::set_setting(&conn, "excluded_apps", &json)?;
    }
    if let Ok(mut guard) = state.excluded_apps_cache.write() {
        *guard = cleaned.clone();
    }
    Ok(cleaned)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let storage = db::resolve_storage_paths().expect("Failed to resolve Klip storage paths");
    let conn = db::open_and_init(&storage.db_path).expect("Failed to initialize Klip SQLite DB");
    let _ = db::enforce_retention_and_limits(&conn);

    let initial_paused = db::get_setting(&conn, "paused")
        .ok()
        .flatten()
        .map(|v| v == "true")
        .unwrap_or(false);

    let initial_theme = db::get_setting(&conn, "theme")
        .ok()
        .flatten()
        .map(|v| {
            if v == "light" {
                "light".to_string()
            } else {
                "dark".to_string()
            }
        })
        .unwrap_or_else(|| "dark".to_string());

    let initial_hotkey = db::get_setting(&conn, "hotkey")
        .ok()
        .flatten()
        .unwrap_or_else(|| "Ctrl+Shift+Backquote".to_string());

    let initial_excluded: Vec<String> = db::get_setting(&conn, "excluded_apps")
        .ok()
        .flatten()
        .and_then(|raw| serde_json::from_str::<Vec<String>>(&raw).ok())
        .unwrap_or_default()
        .into_iter()
        .map(|s| normalize_exe_name(&s))
        .filter(|s| !s.is_empty())
        .collect();

    let initial_autostart_set = db::get_setting(&conn, "autostart_initialized")
        .ok()
        .flatten()
        .is_some();

    let app_state = AppState {
        conn: Mutex::new(conn),
        db_path: storage.db_path,
        images_dir: storage.images_dir.clone(),
        app_icons_dir: storage.app_icons_dir.clone(),
        paused: Mutex::new(initial_paused),
        paused_flag: Arc::new(AtomicBool::new(initial_paused)),
        excluded_apps_cache: Arc::new(RwLock::new(initial_excluded)),
        theme: Mutex::new(initial_theme.clone()),
        pause_menu_item: Mutex::new(None),
        allow_exit: AtomicBool::new(false),
        watcher: Mutex::new(None),
        popup_prev_hwnd: AtomicIsize::new(0),
        popup_keep_open: AtomicBool::new(false),
        popup_opened_at_ms: AtomicI64::new(0),
    };

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            open_or_focus_main_window(app);
        }))
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .manage(app_state)
        .on_window_event(|window, event| {
            if window.label() == "popup" {
                match event {
                    WindowEvent::Focused(false) => {
                        let app_handle = window.app_handle();
                        let state = app_handle.state::<AppState>();
                        let keep_open = state.popup_keep_open.load(Ordering::SeqCst);
                        let opened_at = state.popup_opened_at_ms.load(Ordering::SeqCst);
                        let elapsed = db::now_unix_ms() - opened_at;
                        if !keep_open && elapsed > 180 {
                            let _ = window.close();
                        }
                    }
                    WindowEvent::Moved(pos) => {
                        let app_handle = window.app_handle();
                        let state = app_handle.state::<AppState>();
                        if let Ok(conn) = state.conn.lock() {
                            let _ = db::set_setting(&conn, "popup_last_x", &pos.x.to_string());
                            let _ = db::set_setting(&conn, "popup_last_y", &pos.y.to_string());
                        };
                    }
                    _ => {}
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_phase1_status,
            get_popup_init_data,
            toggle_pause,
            set_theme,
            set_font_family,
            list_installed_fonts,
            list_clips,
            copy_clip,
            paste_clip,
            close_popup,
            set_popup_keep_open,
            toggle_pin_clip,
            delete_clip,
            get_excluded_apps,
            set_excluded_apps
        ])
        .setup(move |app| {
            let scope = app.asset_protocol_scope();
            let _ = scope.allow_directory(&storage.images_dir, true);
            let _ = scope.allow_directory(&storage.app_icons_dir, true);

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

            let tray_icon =
                Image::from_bytes(select_tray_icon_bytes(&initial_theme, initial_paused))?;
            let tooltip = if initial_paused {
                "Klip \u{2014} Saving Paused"
            } else {
                "Klip \u{2014} Clipboard Manager"
            };

            TrayIconBuilder::with_id(TRAY_ID)
                .icon(tray_icon)
                .tooltip(tooltip)
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "open" => open_or_focus_main_window(app),
                    "pause" => {
                        let state = app.state::<AppState>();
                        let next_paused = state.paused.lock().map(|g| !*g).unwrap_or(false);
                        let _ = apply_pause_state(app, &state, next_paused);
                    }
                    "quit" => {
                        let state = app.state::<AppState>();
                        if let Ok(mut w) = state.watcher.lock() {
                            if let Some(handle) = w.take() {
                                handle.stop();
                            }
                        };
                        state.allow_exit.store(true, Ordering::SeqCst);
                        app.exit(0);
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        open_or_focus_main_window(tray.app_handle());
                    }
                })
                .build(app)?;

            let handle = clipboard::start_clipboard_listener(app.handle().clone(), initial_hotkey);
            {
                let state = app.state::<AppState>();
                if let Ok(mut guard) = state.watcher.lock() {
                    *guard = Some(handle);
                };
            }

            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building Klip");

    app.run(|app_handle, event| {
        if let RunEvent::ExitRequested { api, .. } = event {
            let state = app_handle.state::<AppState>();
            if !state.allow_exit.load(Ordering::SeqCst) {
                api.prevent_exit();
            }
        }
    });
}