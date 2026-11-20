use crate::clipboard::write_clip_to_clipboard;
use crate::db::{self, ClipItem};
use crate::{select_window_icon_bytes, AppState};
use std::mem::{size_of, zeroed};
use std::ptr::null_mut;
use std::sync::atomic::Ordering;
use std::thread;
use std::time::Duration;
use tauri::image::Image;
use tauri::{AppHandle, Manager, PhysicalPosition, WebviewUrl, WebviewWindowBuilder};
use windows_sys::Win32::Foundation::{HWND, POINT};
use windows_sys::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows_sys::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, SendInput, INPUT, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, GetForegroundWindow, GetWindowThreadProcessId, IsWindow, SetForegroundWindow,
};

pub const HOTKEY_POPUP_ID: i32 = 1;

const MOD_ALT: u32 = 0x0001;
const MOD_CONTROL: u32 = 0x0002;
const MOD_SHIFT: u32 = 0x0004;
const MOD_WIN: u32 = 0x0008;
const MOD_NOREPEAT: u32 = 0x4000;

const POPUP_LOGICAL_W: f64 = 660.0;
const POPUP_LOGICAL_H: f64 = 430.0;

pub fn parse_hotkey_string(spec: &str) -> Option<(u32, u32)> {
    let mut mods: u32 = MOD_NOREPEAT;
    let mut key_token: Option<&str> = None;

    for raw in spec.split('+') {
        let tok = raw.trim();
        if tok.is_empty() {
            continue;
        }
        match tok.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => mods |= MOD_CONTROL,
            "shift" => mods |= MOD_SHIFT,
            "alt" | "menu" => mods |= MOD_ALT,
            "win" | "super" | "meta" => mods |= MOD_WIN,
            _ => key_token = Some(tok),
        }
    }

    let vk = match key_token?.to_ascii_lowercase().as_str() {
        "backquote" | "`" | "~" | "tilde" => 0xC0,
        "space" => 0x20,
        "tab" => 0x09,
        "enter" | "return" => 0x0D,
        "escape" | "esc" => 0x1B,
        "insert" | "ins" => 0x2D,
        "delete" | "del" => 0x2E,
        "home" => 0x24,
        "end" => 0x23,
        "pageup" => 0x21,
        "pagedown" => 0x22,
        "slash" | "/" => 0xBF,
        "backslash" | "\\" => 0xDC,
        "semicolon" | ";" => 0xBA,
        "quote" | "'" => 0xDE,
        "comma" | "," => 0xBC,
        "period" | "." => 0xBE,
        "minus" | "-" => 0xBD,
        "equal" | "=" => 0xBB,
        "bracketleft" | "[" => 0xDB,
        "bracketright" | "]" => 0xDD,
        s if s.starts_with('f') && s.len() <= 3 => {
            let n: u32 = s[1..].parse().ok()?;
            if (1..=12).contains(&n) {
                0x6F + n
            } else {
                return None;
            }
        }
        s if s.len() == 1 => {
            let ch = s.chars().next()?;
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_uppercase() as u32
            } else {
                return None;
            }
        }
        _ => return None,
    };

    Some((mods, vk))
}

fn get_cursor_and_work_area() -> (POINT, (i32, i32, i32, i32)) {
    unsafe {
        let mut pt = POINT { x: 200, y: 200 };
        GetCursorPos(&mut pt);

        let hmon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
        let mut mi: MONITORINFO = zeroed();
        mi.cbSize = size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(hmon, &mut mi) != 0 {
            let r = mi.rcWork;
            (pt, (r.left, r.top, r.right, r.bottom))
        } else {
            (pt, (0, 0, 1920, 1040))
        }
    }
}

pub fn toggle_popup_window(app: &AppHandle, captured_fg_hwnd: isize) {
    if let Some(existing) = app.get_webview_window("popup") {
        let _ = existing.close();
        return;
    }

    let state = app.state::<AppState>();
    state
        .popup_prev_hwnd
        .store(captured_fg_hwnd, Ordering::SeqCst);
    state.popup_keep_open.store(false, Ordering::SeqCst);
    state.popup_has_focused.store(false, Ordering::SeqCst);
    state
        .popup_opened_at_ms
        .store(db::now_unix_ms(), Ordering::SeqCst);

    let (theme, position_mode, last_x, last_y) = {
        let Ok(conn) = state.conn.lock() else {
            return;
        };
        let theme = db::get_setting(&conn, "theme")
            .ok()
            .flatten()
            .unwrap_or_else(|| "dark".to_string());
        let pos_mode = db::get_setting(&conn, "popup_position")
            .ok()
            .flatten()
            .unwrap_or_else(|| "mouse".to_string());
        let lx = db::get_setting(&conn, "popup_last_x")
            .ok()
            .flatten()
            .and_then(|v| v.parse::<i32>().ok());
        let ly = db::get_setting(&conn, "popup_last_y")
            .ok()
            .flatten()
            .and_then(|v| v.parse::<i32>().ok());
        (theme, pos_mode, lx, ly)
    };

    let (cursor, (wl, wt, wr, wb)) = get_cursor_and_work_area();

    let Ok(win) = WebviewWindowBuilder::new(
        app,
        "popup",
        WebviewUrl::App("index.html?view=popup".into()),
    )
    .title("Klip Quick Popup")
    .inner_size(POPUP_LOGICAL_W, POPUP_LOGICAL_H)
    .decorations(false)
    .always_on_top(true)
    .skip_taskbar(true)
    .resizable(false)
    .visible(false)
    .build()
    else {
        return;
    };

    if let Ok(icon_img) = Image::from_bytes(select_window_icon_bytes(&theme)) {
        let _ = win.set_icon(icon_img);
    }

    let _ = win.set_position(PhysicalPosition::new(cursor.x, cursor.y));
    let scale = win.scale_factor().unwrap_or(1.0);
    let phys_w = (POPUP_LOGICAL_W * scale).round() as i32;
    let phys_h = (POPUP_LOGICAL_H * scale).round() as i32;
    let margin = (10.0 * scale).round() as i32;

    let (raw_x, raw_y) = match position_mode.as_str() {
        "center" => (
            wl + ((wr - wl) - phys_w) / 2,
            wt + ((wb - wt) - phys_h) / 2,
        ),
        "bottom_right" => (wr - phys_w - margin, wb - phys_h - margin),
        "last" => {
            if let (Some(lx), Some(ly)) = (last_x, last_y) {
                (lx, ly)
            } else {
                (cursor.x + margin, cursor.y + margin)
            }
        }
        _ => (cursor.x + margin, cursor.y + margin),
    };

    let min_x = wl + 4;
    let min_y = wt + 4;
    let max_x = (wr - phys_w - 4).max(min_x);
    let max_y = (wb - phys_h - 4).max(min_y);

    let final_x = raw_x.clamp(min_x, max_x);
    let final_y = raw_y.clamp(min_y, max_y);

    let _ = win.set_position(PhysicalPosition::new(final_x, final_y));

    unsafe {
        let fg = GetForegroundWindow();
        let fg_tid = if !fg.is_null() {
            GetWindowThreadProcessId(fg, null_mut())
        } else {
            0
        };
        let cur_tid = GetCurrentThreadId();
        let attached = fg_tid != 0 && fg_tid != cur_tid && AttachThreadInput(fg_tid, cur_tid, 1) != 0;
        let _ = win.show();
        let _ = win.set_focus();
        if attached {
            AttachThreadInput(fg_tid, cur_tid, 0);
        }
    }
}

fn send_ctrl_v() {
    unsafe {
        let mut list: Vec<INPUT> = Vec::with_capacity(8);

        if GetAsyncKeyState(0x10) < 0 {
            let mut up: INPUT = zeroed();
            up.r#type = INPUT_KEYBOARD;
            up.Anonymous.ki = KEYBDINPUT {
                wVk: 0x10,
                wScan: 0,
                dwFlags: KEYEVENTF_KEYUP,
                time: 0,
                dwExtraInfo: 0,
            };
            list.push(up);
        }
        if GetAsyncKeyState(0x12) < 0 {
            let mut up: INPUT = zeroed();
            up.r#type = INPUT_KEYBOARD;
            up.Anonymous.ki = KEYBDINPUT {
                wVk: 0x12,
                wScan: 0,
                dwFlags: KEYEVENTF_KEYUP,
                time: 0,
                dwExtraInfo: 0,
            };
            list.push(up);
        }

        let keys = [
            (0x11u16, 0u32),
            (0x56u16, 0u32),
            (0x56u16, KEYEVENTF_KEYUP),
            (0x11u16, KEYEVENTF_KEYUP),
        ];

        for (vk, flags) in keys {
            let mut inp: INPUT = zeroed();
            inp.r#type = INPUT_KEYBOARD;
            inp.Anonymous.ki = KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            };
            list.push(inp);
        }

        SendInput(list.len() as u32, list.as_ptr(), size_of::<INPUT>() as i32);
    }
}

pub fn paste_clip_to_previous_window(
    app: &AppHandle,
    item: &ClipItem,
    plain_only: bool,
) -> Result<(), String> {
    write_clip_to_clipboard(item, plain_only)?;

    let state = app.state::<AppState>();
    let keep_open = state.popup_keep_open.load(Ordering::SeqCst);
    let prev_hwnd = state.popup_prev_hwnd.load(Ordering::SeqCst);
    let paste_mode = {
        let conn = state.conn.lock().map_err(|_| "DB lock poisoned".to_string())?;
        db::get_setting(&conn, "paste_mode")?
            .unwrap_or_else(|| "auto_paste".to_string())
    };

    if paste_mode == "auto_paste" && prev_hwnd != 0 {
        unsafe {
            let target = prev_hwnd as HWND;
            if IsWindow(target) != 0 {
                SetForegroundWindow(target);
            }
        }
    }

    if !keep_open {
        if let Some(win) = app.get_webview_window("popup") {
            let _ = win.close();
        }
    }

    if paste_mode == "auto_paste" && prev_hwnd != 0 {
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(65));
            send_ctrl_v();
        });
    }

    Ok(())
}