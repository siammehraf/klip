use crate::db::{self, ClipItem, NewClip};
use crate::icon_extractor::{encode_rgba_png, ensure_app_icon, resolve_source_from_hwnds};
use crate::popup::{parse_hotkey_string, toggle_popup_window, HOTKEY_POPUP_ID};
use crate::AppState;
use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::c_void;
use std::fs;
use std::io::Cursor;
use std::mem::{size_of, zeroed};
use std::path::PathBuf;
use std::ptr::{copy_nonoverlapping, null, null_mut};
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};
use windows_sys::Win32::Foundation::{FALSE, HANDLE, HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{BI_BITFIELDS, BI_RGB};
use windows_sys::Win32::System::DataExchange::{
    AddClipboardFormatListener, CloseClipboard, EmptyClipboard, GetClipboardData,
    GetClipboardOwner, IsClipboardFormatAvailable, OpenClipboard, RegisterClipboardFormatW,
    RemoveClipboardFormatListener, SetClipboardData,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{RegisterHotKey, UnregisterHotKey};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetForegroundWindow,
    GetMessageW, GetWindowTextW, PostMessageW, PostQuitMessage, RegisterClassExW,
    TranslateMessage, HWND_MESSAGE, WM_CLOSE, WM_DESTROY, WNDCLASSEXW,
};

#[link(name = "kernel32")]
extern "system" {
    fn GlobalFree(hmem: *mut c_void) -> *mut c_void;
}

const CF_DIB: u32 = 8;
const CF_UNICODETEXT: u32 = 13;
const CF_HDROP: u32 = 15;
const CF_DIBV5: u32 = 17;
const WM_CLIPBOARDUPDATE: u32 = 0x031D;
const WM_HOTKEY: u32 = 0x0312;
const WM_APP_RELOAD_HOTKEY: u32 = 0x8001;
const WS_EX_TOOLWINDOW_FLAG: u32 = 0x0000_0080;
const WS_EX_NOACTIVATE_FLAG: u32 = 0x0800_0000;
const GMEM_MOVEABLE_FLAG: u32 = 0x0002;

pub static IGNORE_SELF_COPY: AtomicBool = AtomicBool::new(false);
static IMAGE_SEQ: AtomicU64 = AtomicU64::new(1);

struct FormatIds {
    html: u32,
    png: u32,
    exclude_monitor: u32,
    viewer_ignore: u32,
    can_include_history: u32,
}

enum RawClipboardPayload {
    ImagePng {
        png_bytes: Vec<u8>,
        raw_html: Option<String>,
        fg_hwnd: isize,
        owner_hwnd: isize,
        created_at: i64,
    },
    ImageDib {
        dib_bytes: Vec<u8>,
        raw_html: Option<String>,
        fg_hwnd: isize,
        owner_hwnd: isize,
        created_at: i64,
    },
    Text {
        text: Option<String>,
        raw_html: Option<String>,
        fg_hwnd: isize,
        owner_hwnd: isize,
        created_at: i64,
    },
}

struct ListenerContext {
    app: AppHandle,
    formats: FormatIds,
    worker_tx: Option<Sender<RawClipboardPayload>>,
}

thread_local! {
    static LISTENER_CTX: RefCell<Option<ListenerContext>> = const { RefCell::new(None) };
}

pub struct ClipboardWatcherHandle {
    hwnd: Arc<AtomicIsize>,
}

impl ClipboardWatcherHandle {
    pub fn reload_hotkey(&self) {
        let raw = self.hwnd.load(Ordering::SeqCst);
        if raw != 0 {
            unsafe {
                PostMessageW(raw as HWND, WM_APP_RELOAD_HOTKEY, 0, 0);
            }
        }
    }

    pub fn stop(&self) {
        let raw = self.hwnd.load(Ordering::SeqCst);
        if raw != 0 {
            unsafe {
                PostMessageW(raw as HWND, WM_CLOSE, 0, 0);
            }
        }
    }
}

fn to_wide_null(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn register_format(name: &str) -> u32 {
    let w = to_wide_null(name);
    unsafe { RegisterClipboardFormatW(w.as_ptr()) }
}

struct ClipboardLock;

impl ClipboardLock {
    fn acquire(hwnd: HWND) -> Option<Self> {
        for _ in 0..4 {
            let ok = unsafe { OpenClipboard(hwnd) };
            if ok != 0 {
                return Some(Self);
            }
            thread::sleep(Duration::from_millis(5));
        }
        None
    }
}

impl Drop for ClipboardLock {
    fn drop(&mut self) {
        unsafe {
            CloseClipboard();
        }
    }
}

unsafe fn read_global_bytes(handle: HANDLE) -> Option<Vec<u8>> {
    if handle.is_null() {
        return None;
    }
    let size = GlobalSize(handle);
    if size == 0 {
        return None;
    }
    let ptr = GlobalLock(handle) as *const u8;
    if ptr.is_null() {
        return None;
    }
    let slice = std::slice::from_raw_parts(ptr, size);
    let data = slice.to_vec();
    GlobalUnlock(handle);
    Some(data)
}

unsafe fn alloc_and_set_clipboard(format: u32, bytes: &[u8]) -> Result<(), String> {
    let hmem = GlobalAlloc(GMEM_MOVEABLE_FLAG, bytes.len());
    if hmem.is_null() {
        return Err("GlobalAlloc failed".to_string());
    }
    let ptr = GlobalLock(hmem) as *mut u8;
    if ptr.is_null() {
        GlobalFree(hmem);
        return Err("GlobalLock failed".to_string());
    }
    copy_nonoverlapping(bytes.as_ptr(), ptr, bytes.len());
    GlobalUnlock(hmem);

    if SetClipboardData(format, hmem).is_null() {
        GlobalFree(hmem);
        return Err("SetClipboardData failed".to_string());
    }
    Ok(())
}

pub fn write_clip_to_clipboard(item: &ClipItem, plain_only: bool) -> Result<(), String> {
    let _lock = ClipboardLock::acquire(null_mut())
        .ok_or_else(|| "Windows clipboard is busy".to_string())?;

    if item.clip_type == "image" {
        let img_path = item
            .image_path
            .as_deref()
            .ok_or_else(|| "Missing image path".to_string())?;
        let png_bytes = fs::read(img_path).map_err(|e| format!("Failed reading image: {e}"))?;
        let dib_bytes = png_to_dib(&png_bytes)?;
        let png_fmt = register_format("PNG");

        IGNORE_SELF_COPY.store(true, Ordering::SeqCst);
        unsafe {
            if EmptyClipboard() == 0 {
                IGNORE_SELF_COPY.store(false, Ordering::SeqCst);
                return Err("EmptyClipboard failed".to_string());
            }
            if let Err(e) = alloc_and_set_clipboard(CF_DIB, &dib_bytes) {
                IGNORE_SELF_COPY.store(false, Ordering::SeqCst);
                return Err(e);
            }
            if png_fmt != 0 {
                let _ = alloc_and_set_clipboard(png_fmt, &png_bytes);
            }
        }
        return Ok(());
    }

    let text = item.text_content.as_deref().unwrap_or("");
    let wide = to_wide_null(text);
    let byte_len = wide.len() * 2;
    let utf16_bytes = unsafe { std::slice::from_raw_parts(wide.as_ptr() as *const u8, byte_len) };

    IGNORE_SELF_COPY.store(true, Ordering::SeqCst);
    unsafe {
        if EmptyClipboard() == 0 {
            IGNORE_SELF_COPY.store(false, Ordering::SeqCst);
            return Err("EmptyClipboard failed".to_string());
        }
        if let Err(e) = alloc_and_set_clipboard(CF_UNICODETEXT, utf16_bytes) {
            IGNORE_SELF_COPY.store(false, Ordering::SeqCst);
            return Err(e);
        }

        if !plain_only {
            if let Some(html_frag) = item.html_content.as_deref() {
                if !html_frag.is_empty() {
                    let html_fmt = register_format("HTML Format");
                    if html_fmt != 0 {
                        let cf_html = wrap_cf_html(html_frag);
                        let mut cf_bytes = cf_html.into_bytes();
                        cf_bytes.push(0);
                        let _ = alloc_and_set_clipboard(html_fmt, &cf_bytes);
                    }
                }
            }
        }
    }

    Ok(())
}

fn wrap_cf_html(fragment: &str) -> String {
    let prefix = "Version:0.9\r\nStartHTML:0000000105\r\nEndHTML:0000000000\r\nStartFragment:0000000141\r\nEndFragment:0000000000\r\n<html><body>\r\n<!--StartFragment-->";
    let suffix = "<!--EndFragment-->\r\n</body></html>";
    let full = format!("{prefix}{fragment}{suffix}");
    let start_html = full.find("<html>").unwrap_or(105);
    let end_html = full.len();
    let start_frag = full
        .find("<!--StartFragment-->")
        .map(|i| i + 20)
        .unwrap_or(start_html);
    let end_frag = full.find("<!--EndFragment-->").unwrap_or(end_html);
    format!(
        "Version:0.9\r\nStartHTML:{start_html:010}\r\nEndHTML:{end_html:010}\r\nStartFragment:{start_frag:010}\r\nEndFragment:{end_frag:010}\r\n<html><body>\r\n<!--StartFragment-->{fragment}<!--EndFragment-->\r\n</body></html>"
    )
}

fn png_to_dib(png_bytes: &[u8]) -> Result<Vec<u8>, String> {
    let mut decoder = png::Decoder::new(Cursor::new(png_bytes));
    decoder.set_transformations(png::Transformations::EXPAND);
    let mut reader = decoder
        .read_info()
        .map_err(|e| format!("PNG decode header error: {e}"))?;
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader
        .next_frame(&mut buf)
        .map_err(|e| format!("PNG decode frame error: {e}"))?;
    let samples = &buf[..info.buffer_size()];

    let w = info.width as i32;
    let h = info.height as i32;
    let row_bytes = (info.width as usize) * 4;
    let mut dib = Vec::with_capacity(40 + row_bytes * (info.height as usize));

    dib.extend_from_slice(&40u32.to_le_bytes());
    dib.extend_from_slice(&w.to_le_bytes());
    dib.extend_from_slice(&h.to_le_bytes());
    dib.extend_from_slice(&1u16.to_le_bytes());
    dib.extend_from_slice(&32u16.to_le_bytes());
    dib.extend_from_slice(&0u32.to_le_bytes());
    dib.extend_from_slice(&((row_bytes * info.height as usize) as u32).to_le_bytes());
    dib.extend_from_slice(&0i32.to_le_bytes());
    dib.extend_from_slice(&0i32.to_le_bytes());
    dib.extend_from_slice(&0u32.to_le_bytes());
    dib.extend_from_slice(&0u32.to_le_bytes());

    let src_bpp = match info.color_type {
        png::ColorType::Rgba => 4,
        png::ColorType::Rgb => 3,
        png::ColorType::GrayscaleAlpha => 2,
        png::ColorType::Grayscale => 1,
        _ => return Err("Unsupported PNG color type".into()),
    };

    for y in (0..info.height as usize).rev() {
        let row = &samples
            [y * (info.width as usize) * src_bpp..(y + 1) * (info.width as usize) * src_bpp];
        for x in 0..info.width as usize {
            let s = x * src_bpp;
            match info.color_type {
                png::ColorType::Rgba => {
                    dib.push(row[s + 2]);
                    dib.push(row[s + 1]);
                    dib.push(row[s]);
                    dib.push(row[s + 3]);
                }
                png::ColorType::Rgb => {
                    dib.push(row[s + 2]);
                    dib.push(row[s + 1]);
                    dib.push(row[s]);
                    dib.push(255);
                }
                png::ColorType::GrayscaleAlpha => {
                    dib.push(row[s]);
                    dib.push(row[s]);
                    dib.push(row[s]);
                    dib.push(row[s + 1]);
                }
                png::ColorType::Grayscale => {
                    dib.push(row[s]);
                    dib.push(row[s]);
                    dib.push(row[s]);
                    dib.push(255);
                }
                _ => {}
            }
        }
    }

    Ok(dib)
}

fn is_privacy_blocked(formats: &FormatIds) -> bool {
    unsafe {
        if formats.exclude_monitor != 0 && IsClipboardFormatAvailable(formats.exclude_monitor) != 0
        {
            return true;
        }
        if formats.viewer_ignore != 0 && IsClipboardFormatAvailable(formats.viewer_ignore) != 0 {
            return true;
        }
        if formats.can_include_history != 0
            && IsClipboardFormatAvailable(formats.can_include_history) != 0
        {
            let h = GetClipboardData(formats.can_include_history);
            if let Some(bytes) = read_global_bytes(h) {
                if bytes.len() >= 4 {
                    let flag = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
                    if flag == 0 {
                        return true;
                    }
                }
            }
        }
    }
    false
}

pub fn normalize_exe_name(name: &str) -> String {
    name.trim().to_lowercase()
}

fn is_app_excluded(excluded_list: &[String], exe_name: &str) -> bool {
    let target = normalize_exe_name(exe_name);
    if target.is_empty() {
        return false;
    }
    let target_stem = target.strip_suffix(".exe").unwrap_or(&target);

    excluded_list.iter().any(|entry| {
        let e = normalize_exe_name(entry);
        if e.is_empty() {
            return false;
        }
        let e_stem = e.strip_suffix(".exe").unwrap_or(&e);
        e == target || e_stem == target_stem
    })
}

fn is_browser_exe(exe_name: &str) -> bool {
    matches!(
        normalize_exe_name(exe_name).as_str(),
        "brave.exe"
            | "chrome.exe"
            | "msedge.exe"
            | "firefox.exe"
            | "opera.exe"
            | "vivaldi.exe"
            | "arc.exe"
            | "zen.exe"
            | "waterfox.exe"
            | "floorp.exe"
    )
}

fn extract_domain_from_url(raw_url: &str) -> Option<String> {
    let url = raw_url.trim();
    let lower = url.to_ascii_lowercase();
    let after_scheme = if lower.starts_with("https://") {
        &url[8..]
    } else if lower.starts_with("http://") {
        &url[7..]
    } else if lower.starts_with("www.") {
        url
    } else {
        return None;
    };

    let host_port = after_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or("")
        .trim();
    let host = host_port.split(':').next().unwrap_or("").to_lowercase();
    let clean_host = host.strip_prefix("www.").unwrap_or(&host);
    if clean_host.is_empty() || !clean_host.contains('.') {
        return None;
    }
    Some(format!("site:{clean_host}"))
}

fn read_window_title(hwnd_raw: isize) -> Option<String> {
    if hwnd_raw == 0 {
        return None;
    }
    unsafe {
        let mut buf = [0u16; 512];
        let len = GetWindowTextW(hwnd_raw as HWND, buf.as_mut_ptr(), buf.len() as i32);
        if len > 0 {
            let title = String::from_utf16_lossy(&buf[..len as usize])
                .trim()
                .to_string();
            if !title.is_empty() {
                return Some(title);
            }
        }
    }
    None
}

fn strip_leading_notification_count(s: &str) -> &str {
    let t = s.trim_start();
    if let Some(after_open) = t.strip_prefix('(') {
        if let Some(close_idx) = after_open.find(')') {
            let inner = &after_open[..close_idx];
            if !inner.is_empty() && inner.chars().all(|c| c.is_ascii_digit() || c == '+') {
                return after_open[close_idx + 1..].trim_start();
            }
        }
    }
    t
}

fn strip_browser_suffix(title: &str) -> String {
    let mut s = title.trim().to_string();
    let suffixes = [
        " - Brave",
        " - Google Chrome",
        " - Microsoft\u{200b} Edge",
        " - Microsoft Edge",
        " \u{2014} Mozilla Firefox",
        " - Mozilla Firefox",
        " - Opera",
        " - Vivaldi",
        " - Arc",
        " - Zen Browser",
    ];
    for sfx in suffixes {
        if s.len() > sfx.len() && s[s.len() - sfx.len()..].eq_ignore_ascii_case(sfx) {
            s.truncate(s.len() - sfx.len());
            break;
        }
    }
    strip_leading_notification_count(&s).trim().to_string()
}

fn infer_site_from_browser_title(title: &str) -> Option<String> {
    let cleaned = strip_browser_suffix(title);
    let lower = cleaned.to_lowercase();
    if lower.is_empty() {
        return None;
    }

    if lower.contains("facebook") {
        return Some("site:facebook.com".to_string());
    }
    if lower.contains("youtube") {
        return Some("site:youtube.com".to_string());
    }
    if lower.contains("github") {
        return Some("site:github.com".to_string());
    }
    if lower.contains("gmail") || lower.contains("mail.google") {
        return Some("site:mail.google.com".to_string());
    }
    if lower.contains("chatgpt") {
        return Some("site:chatgpt.com".to_string());
    }
    if lower.contains("claude") {
        return Some("site:claude.ai".to_string());
    }
    if lower.contains("whatsapp") {
        return Some("site:whatsapp.com".to_string());
    }
    if lower.contains("instagram") {
        return Some("site:instagram.com".to_string());
    }
    if lower.contains("todoist") {
        return Some("site:todoist.com".to_string());
    }
    if lower.contains("discord") {
        return Some("site:discord.com".to_string());
    }
    if lower.contains("notion") {
        return Some("site:notion.so".to_string());
    }
    if lower.contains("reddit") {
        return Some("site:reddit.com".to_string());
    }
    if lower.contains("linkedin") {
        return Some("site:linkedin.com".to_string());
    }
    if lower.contains("tracker.gg") || lower.contains("tracker network") {
        return Some("site:tracker.gg".to_string());
    }
    if lower.contains("torrentbd") {
        return Some("site:torrentbd.net".to_string());
    }
    if lower.contains("stack overflow") || lower.contains("stackoverflow") {
        return Some("site:stackoverflow.com".to_string());
    }
    if lower.contains("twitch") {
        return Some("site:twitch.tv".to_string());
    }
    if lower.contains("twitter") || lower.ends_with(" / x") || lower.starts_with("x / ") || lower == "x" {
        return Some("site:x.com".to_string());
    }
    if lower.contains("google") {
        return Some("site:google.com".to_string());
    }

    // Check if the cleaned tab title explicitly contains a domain (e.g. "Page - example.com")
    for token in lower.split(|c: char| c.is_whitespace() || c == '|' || c == '/') {
        let t = token.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '.' && c != '-');
        if t.contains('.') && !t.starts_with('.') && !t.ends_with('.') && t.len() <= 64 {
            let parts: Vec<&str> = t.split('.').collect();
            if parts.len() >= 2 && parts.last().map(|tld| tld.len() >= 2 && tld.chars().all(|c| c.is_ascii_alphabetic())).unwrap_or(false) {
                let host = t.strip_prefix("www.").unwrap_or(t);
                return Some(format!("site:{host}"));
            }
        }
    }

    None
}

fn extract_source_site(
    raw_html: Option<&str>,
    text_opt: Option<&str>,
    exe_name: Option<&str>,
    fg_hwnd: isize,
) -> Option<String> {
    let is_browser = exe_name.map(is_browser_exe).unwrap_or(false);

    if is_browser {
        if let Some(html) = raw_html {
            for line in html.lines().take(25) {
                let trimmed = line.trim();
                if trimmed.len() > 10 && trimmed[..10].eq_ignore_ascii_case("sourceurl:") {
                    if let Some(site) = extract_domain_from_url(&trimmed[10..]) {
                        return Some(site);
                    }
                }
            }
        }
    }

    if let Some(text) = text_opt {
        let trimmed = text.trim();
        if is_link(trimmed) {
            if let Some(site) = extract_domain_from_url(trimmed) {
                return Some(site);
            }
        }
    }

    if is_browser {
        if let Some(title) = read_window_title(fg_hwnd) {
            if let Some(site) = infer_site_from_browser_title(&title) {
                return Some(site);
            }
        }
    }

    None
}

fn read_le_u32(bytes: &[u8], off: usize) -> Option<u32> {
    let slice = bytes.get(off..off + 4)?;
    Some(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

fn read_le_i32(bytes: &[u8], off: usize) -> Option<i32> {
    let slice = bytes.get(off..off + 4)?;
    Some(i32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

fn read_le_u16(bytes: &[u8], off: usize) -> Option<u16> {
    let slice = bytes.get(off..off + 2)?;
    Some(u16::from_le_bytes([slice[0], slice[1]]))
}

fn mask_shift_and_scale(mask: u32, val: u32) -> u8 {
    if mask == 0 {
        return 0;
    }
    let shift = mask.trailing_zeros();
    let bits = (mask >> shift).count_ones();
    if bits == 0 {
        return 0;
    }
    let raw = (val & mask) >> shift;
    let max_val = (1u32 << bits) - 1;
    ((raw * 255 + max_val / 2) / max_val) as u8
}

fn dib_to_png(dib: &[u8]) -> Result<(Vec<u8>, u32, u32), String> {
    if dib.len() < 40 {
        return Err("DIB header too small".into());
    }
    let bi_size = read_le_u32(dib, 0).ok_or("Missing biSize")? as usize;
    if bi_size < 40 || dib.len() < bi_size {
        return Err("Invalid DIB biSize".into());
    }
    let width_i = read_le_i32(dib, 4).ok_or("Missing biWidth")?;
    let height_i = read_le_i32(dib, 8).ok_or("Missing biHeight")?;
    let bpp = read_le_u16(dib, 14).ok_or("Missing biBitCount")? as u32;
    let compression = read_le_u32(dib, 16).ok_or("Missing biCompression")?;
    let clr_used = read_le_u32(dib, 32).unwrap_or(0) as usize;

    if width_i <= 0 || height_i == 0 || width_i > 16384 || height_i.abs() > 16384 {
        return Err("Unsupported DIB dimensions".into());
    }
    let width = width_i as u32;
    let height = height_i.unsigned_abs();
    let top_down = height_i < 0;

    if (width as usize) * (height as usize) * 4 > 150 * 1024 * 1024 {
        return Err("Raw image exceeds 150 MB guard".into());
    }

    let mut r_mask: u32 = 0x00FF_0000;
    let mut g_mask: u32 = 0x0000_FF00;
    let mut b_mask: u32 = 0x0000_00FF;
    let mut a_mask: u32 = 0xFF00_0000;
    let mut extra_masks = 0usize;

    if compression == BI_BITFIELDS {
        if bi_size == 40 {
            r_mask = read_le_u32(dib, 40).unwrap_or(r_mask);
            g_mask = read_le_u32(dib, 44).unwrap_or(g_mask);
            b_mask = read_le_u32(dib, 48).unwrap_or(b_mask);
            a_mask = 0;
            extra_masks = 12;
        } else if bi_size >= 56 {
            r_mask = read_le_u32(dib, 40).unwrap_or(r_mask);
            g_mask = read_le_u32(dib, 44).unwrap_or(g_mask);
            b_mask = read_le_u32(dib, 48).unwrap_or(b_mask);
            a_mask = read_le_u32(dib, 52).unwrap_or(0);
        }
    } else if compression != BI_RGB {
        return Err(format!("Unsupported DIB compression: {compression}"));
    }

    let palette_entries = if bpp <= 8 {
        if clr_used > 0 {
            clr_used
        } else {
            1usize << bpp
        }
    } else {
        clr_used
    };
    let palette_offset = bi_size + extra_masks;
    let pixel_offset = palette_offset + palette_entries * 4;
    if dib.len() <= pixel_offset {
        return Err("DIB truncated before pixel data".into());
    }

    let row_stride = (((width * bpp + 31) / 32) * 4) as usize;
    let pixels = &dib[pixel_offset..];
    if pixels.len() < row_stride * (height as usize) {
        return Err("DIB pixel buffer truncated".into());
    }

    let mut rgba = vec![0u8; (width * height * 4) as usize];
    let mut any_non_zero_alpha = false;

    for y in 0..height as usize {
        let src_y = if top_down { y } else { (height as usize) - 1 - y };
        let row = &pixels[src_y * row_stride..(src_y + 1) * row_stride];
        let dst_row = &mut rgba[y * (width as usize) * 4..(y + 1) * (width as usize) * 4];

        match bpp {
            32 => {
                for x in 0..width as usize {
                    let off = x * 4;
                    let val = u32::from_le_bytes([row[off], row[off + 1], row[off + 2], row[off + 3]]);
                    let (r, g, b, a) = if compression == BI_BITFIELDS {
                        (
                            mask_shift_and_scale(r_mask, val),
                            mask_shift_and_scale(g_mask, val),
                            mask_shift_and_scale(b_mask, val),
                            if a_mask != 0 {
                                mask_shift_and_scale(a_mask, val)
                            } else {
                                255
                            },
                        )
                    } else {
                        (row[off + 2], row[off + 1], row[off], row[off + 3])
                    };
                    if a != 0 {
                        any_non_zero_alpha = true;
                    }
                    dst_row[off] = r;
                    dst_row[off + 1] = g;
                    dst_row[off + 2] = b;
                    dst_row[off + 3] = a;
                }
            }
            24 => {
                any_non_zero_alpha = true;
                for x in 0..width as usize {
                    let s = x * 3;
                    let d = x * 4;
                    dst_row[d] = row[s + 2];
                    dst_row[d + 1] = row[s + 1];
                    dst_row[d + 2] = row[s];
                    dst_row[d + 3] = 255;
                }
            }
            16 => {
                any_non_zero_alpha = true;
                let (rm, gm, bm) = if compression == BI_BITFIELDS {
                    (r_mask, g_mask, b_mask)
                } else {
                    (0x7C00, 0x03E0, 0x001F)
                };
                for x in 0..width as usize {
                    let s = x * 2;
                    let d = x * 4;
                    let val = u16::from_le_bytes([row[s], row[s + 1]]) as u32;
                    dst_row[d] = mask_shift_and_scale(rm, val);
                    dst_row[d + 1] = mask_shift_and_scale(gm, val);
                    dst_row[d + 2] = mask_shift_and_scale(bm, val);
                    dst_row[d + 3] = 255;
                }
            }
            8 => {
                any_non_zero_alpha = true;
                let palette = &dib[palette_offset..palette_offset + palette_entries * 4];
                for x in 0..width as usize {
                    let idx = row[x] as usize;
                    let d = x * 4;
                    if idx < palette_entries {
                        let p = idx * 4;
                        dst_row[d] = palette[p + 2];
                        dst_row[d + 1] = palette[p + 1];
                        dst_row[d + 2] = palette[p];
                        dst_row[d + 3] = 255;
                    }
                }
            }
            _ => return Err(format!("Unsupported DIB bit depth: {bpp}")),
        }
    }

    if !any_non_zero_alpha {
        for px in rgba.chunks_exact_mut(4) {
            px[3] = 255;
        }
    }

    let png_bytes = encode_rgba_png(width, height, &rgba)?;
    Ok((png_bytes, width, height))
}

fn read_png_dimensions(png_bytes: &[u8]) -> Option<(u32, u32)> {
    if png_bytes.len() < 24 || &png_bytes[0..8] != b"\x89PNG\r\n\x1a\n" {
        return None;
    }
    let w = u32::from_be_bytes([png_bytes[16], png_bytes[17], png_bytes[18], png_bytes[19]]);
    let h = u32::from_be_bytes([png_bytes[20], png_bytes[21], png_bytes[22], png_bytes[23]]);
    Some((w, h))
}

fn read_unicode_text() -> Option<String> {
    unsafe {
        if IsClipboardFormatAvailable(CF_UNICODETEXT) == 0 {
            return None;
        }
        let h = GetClipboardData(CF_UNICODETEXT);
        let bytes = read_global_bytes(h)?;
        if bytes.len() < 2 {
            return None;
        }
        let mut u16_words = Vec::with_capacity(bytes.len() / 2);
        for chunk in bytes.chunks_exact(2) {
            let word = u16::from_le_bytes([chunk[0], chunk[1]]);
            if word == 0 {
                break;
            }
            u16_words.push(word);
        }
        if u16_words.is_empty() {
            None
        } else {
            Some(String::from_utf16_lossy(&u16_words))
        }
    }
}

fn extract_cf_html_fragment(raw: &str) -> String {
    if let (Some(start), Some(end)) = (
        raw.find("<!--StartFragment-->"),
        raw.find("<!--EndFragment-->"),
    ) {
        let s = start + "<!--StartFragment-->".len();
        if s <= end {
            return raw[s..end].trim().to_string();
        }
    }

    let mut start_frag: Option<usize> = None;
    let mut end_frag: Option<usize> = None;
    for line in raw.lines().take(12) {
        if let Some(rest) = line.strip_prefix("StartFragment:") {
            start_frag = rest.trim().parse::<usize>().ok();
        } else if let Some(rest) = line.strip_prefix("EndFragment:") {
            end_frag = rest.trim().parse::<usize>().ok();
        }
    }

    if let (Some(s), Some(e)) = (start_frag, end_frag) {
        let bytes = raw.as_bytes();
        if s < e && e <= bytes.len() {
            return String::from_utf8_lossy(&bytes[s..e]).trim().to_string();
        }
    }

    raw.trim().to_string()
}

fn read_raw_html_format(html_format_id: u32) -> Option<String> {
    if html_format_id == 0 {
        return None;
    }
    unsafe {
        if IsClipboardFormatAvailable(html_format_id) == 0 {
            return None;
        }
        let h = GetClipboardData(html_format_id);
        let bytes = read_global_bytes(h)?;
        let nul_pos = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
        let raw = String::from_utf8_lossy(&bytes[..nul_pos]).into_owned();
        if raw.trim().is_empty() {
            None
        } else {
            Some(raw)
        }
    }
}

fn has_meaningful_html_formatting(frag: &str) -> bool {
    let lower = frag.to_ascii_lowercase();
    let rich_tags = [
        "<b>", "<b ", "<strong>", "<strong ", "<i>", "<i ", "<em>", "<em ", "<u>", "<u ",
        "<a ", "<h1", "<h2", "<h3", "<h4", "<ul", "<ol", "<li", "<table", "<blockquote",
        "<pre", "<code", "<img ", "font-weight: 700", "font-weight: bold", "font-weight:700",
        "font-weight:bold",
    ];
    rich_tags.iter().any(|tag| lower.contains(tag))
}

fn is_hex_color(s: &str) -> bool {
    if let Some(hex) = s.strip_prefix('#') {
        return matches!(hex.len(), 3 | 6 | 8) && hex.chars().all(|c| c.is_ascii_hexdigit());
    }
    false
}

fn is_functional_color(s: &str) -> bool {
    let lower = s.to_ascii_lowercase();
    (lower.starts_with("rgb(")
        || lower.starts_with("rgba(")
        || lower.starts_with("hsl(")
        || lower.starts_with("hsla("))
        && lower.ends_with(')')
        && lower.len() <= 48
}

fn is_email(s: &str) -> bool {
    if s.len() > 254 || s.contains(char::is_whitespace) {
        return false;
    }
    let parts: Vec<&str> = s.split('@').collect();
    if parts.len() != 2 {
        return false;
    }
    let (local, domain) = (parts[0], parts[1]);
    !local.is_empty()
        && !domain.is_empty()
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && domain.contains('.')
}

fn is_link(s: &str) -> bool {
    if s.contains(char::is_whitespace) || s.len() > 2048 {
        return false;
    }
    let lower = s.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") || lower.starts_with("ftp://") {
        return lower.len() > 8;
    }
    lower.starts_with("www.") && lower[4..].contains('.')
}

fn looks_like_code(s: &str) -> bool {
    let trimmed = s.trim();
    if trimmed.len() < 4 {
        return false;
    }

    let keywords = [
        "fn ", "pub fn ", "function ", "const ", "let ", "var ", "import ", "export ",
        "class ", "def ", "return ", "SELECT ", "INSERT ", "UPDATE ", "DELETE FROM ",
        "CREATE TABLE", "#include", "<?php", "=>", "::", "console.log(", "println!(",
    ];
    if keywords.iter().any(|k| trimmed.contains(k)) {
        return true;
    }

    if trimmed.contains('\n') {
        let has_braces = trimmed.contains('{') && trimmed.contains('}');
        let has_html_tags = trimmed.contains('<') && trimmed.contains("/>");
        let semicolon_lines = trimmed
            .lines()
            .filter(|l| l.trim_end().ends_with(';'))
            .count();
        if has_braces || has_html_tags || semicolon_lines >= 2 {
            return true;
        }
    }

    false
}

fn detect_text_clip_type(text: &str, meaningful_html: bool) -> &'static str {
    let trimmed = text.trim();
    if is_hex_color(trimmed) || is_functional_color(trimmed) {
        return "color";
    }
    if is_email(trimmed) {
        return "email";
    }
    if is_link(trimmed) {
        return "link";
    }
    if looks_like_code(trimmed) {
        return "code";
    }
    if meaningful_html {
        return "rich_text";
    }
    "text"
}

fn process_payload(
    app: &AppHandle,
    payload: RawClipboardPayload,
    cached_icons: &mut HashMap<String, Option<String>>,
) {
    let state = app.state::<AppState>();

    let (fg_hwnd, owner_hwnd, raw_html_opt, created_at) = match &payload {
        RawClipboardPayload::ImagePng {
            raw_html,
            fg_hwnd,
            owner_hwnd,
            created_at,
            ..
        } => (*fg_hwnd, *owner_hwnd, raw_html.as_deref(), *created_at),
        RawClipboardPayload::ImageDib {
            raw_html,
            fg_hwnd,
            owner_hwnd,
            created_at,
            ..
        } => (*fg_hwnd, *owner_hwnd, raw_html.as_deref(), *created_at),
        RawClipboardPayload::Text {
            raw_html,
            fg_hwnd,
            owner_hwnd,
            created_at,
            ..
        } => (*fg_hwnd, *owner_hwnd, raw_html.as_deref(), *created_at),
    };

    let source_proc = resolve_source_from_hwnds(fg_hwnd, owner_hwnd);
    if let Some((ref exe_name, _)) = source_proc {
        if let Ok(excluded) = state.excluded_apps_cache.read() {
            if is_app_excluded(&excluded, exe_name) {
                return;
            }
        }
    }

    let max_bytes: usize = {
        let Ok(conn) = state.conn.lock() else {
            return;
        };
        let max_mb = db::get_setting(&conn, "max_clip_size_mb")
            .ok()
            .flatten()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(10)
            .max(1);
        max_mb * 1024 * 1024
    };

    let text_preview_opt = match &payload {
        RawClipboardPayload::Text { text, .. } => text.as_deref(),
        _ => None,
    };

    let source_site = extract_source_site(
        raw_html_opt,
        text_preview_opt,
        source_proc.as_ref().map(|(name, _)| name.as_str()),
        fg_hwnd,
    );

    match payload {
        RawClipboardPayload::ImagePng { png_bytes, .. } => {
            if let Some((w, h)) = read_png_dimensions(&png_bytes) {
                save_image_clip(
                    app,
                    &state,
                    &png_bytes,
                    w,
                    h,
                    source_proc,
                    source_site.as_deref(),
                    created_at,
                    max_bytes,
                    cached_icons,
                );
            }
        }
        RawClipboardPayload::ImageDib { dib_bytes, .. } => {
            if let Ok((png_bytes, w, h)) = dib_to_png(&dib_bytes) {
                save_image_clip(
                    app,
                    &state,
                    &png_bytes,
                    w,
                    h,
                    source_proc,
                    source_site.as_deref(),
                    created_at,
                    max_bytes,
                    cached_icons,
                );
            }
        }
        RawClipboardPayload::Text { text, raw_html, .. } => {
            let html_frag = raw_html
                .as_deref()
                .map(extract_cf_html_fragment)
                .filter(|f| !f.is_empty());

            let meaningful_html = html_frag
                .as_deref()
                .map(has_meaningful_html_formatting)
                .unwrap_or(false);

            let text_bytes = text.as_ref().map(|s| s.len()).unwrap_or(0);
            let html_bytes = if meaningful_html {
                html_frag.as_ref().map(|s| s.len()).unwrap_or(0)
            } else {
                0
            };
            let stored_html = if meaningful_html { html_frag } else { None };
            let total_size = text_bytes + html_bytes;

            if total_size == 0 || total_size > max_bytes {
                return;
            }

            let clip_type = match text.as_deref() {
                Some(t) => detect_text_clip_type(t, meaningful_html),
                None => "rich_text",
            };

            let (source_app, source_icon_path) = match source_proc {
                Some((exe_name, exe_path)) => {
                    let icon =
                        ensure_app_icon(&exe_name, &exe_path, &state.icons_dir, cached_icons);
                    (Some(exe_name), icon)
                }
                None => (None, None),
            };

            let inserted: Result<ClipItem, String> = (|| {
                let conn = state.conn.lock().map_err(|_| "DB lock poisoned".to_string())?;
                let item = db::insert_clip(
                    &conn,
                    &NewClip {
                        clip_type,
                        text_content: text.as_deref(),
                        html_content: stored_html.as_deref(),
                        image_path: None,
                        content_size: total_size as i64,
                        source_app: source_app.as_deref(),
                        source_icon_path: source_icon_path.as_deref(),
                        source_site: source_site.as_deref(),
                        created_at,
                    },
                )?;
                let removed = db::enforce_retention_and_limits(&conn)?;
                if !removed.is_empty() {
                    let _ = app.emit("klip://clips-pruned", removed);
                }
                Ok(item)
            })();

            if let Ok(item) = inserted {
                let _ = app.emit("klip://clip-added", item);
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn save_image_clip(
    app: &AppHandle,
    state: &AppState,
    png_bytes: &[u8],
    w: u32,
    h: u32,
    source_proc: Option<(String, PathBuf)>,
    source_site: Option<&str>,
    created_at: i64,
    max_bytes: usize,
    cached_icons: &mut HashMap<String, Option<String>>,
) {
    if png_bytes.is_empty() || png_bytes.len() > max_bytes {
        return;
    }

    let (source_app, source_icon_path) = match source_proc {
        Some((exe_name, exe_path)) => {
            let icon = ensure_app_icon(&exe_name, &exe_path, &state.icons_dir, cached_icons);
            (Some(exe_name), icon)
        }
        None => (None, None),
    };

    let seq = IMAGE_SEQ.fetch_add(1, Ordering::Relaxed);
    let file_name = format!("{created_at}_{seq}.png");
    let full_path = state.images_dir.join(file_name);
    if fs::write(&full_path, png_bytes).is_err() {
        return;
    }

    let dims_text = format!("{w}x{h}");
    let path_str = full_path.to_string_lossy().into_owned();
    let inserted: Result<ClipItem, String> = (|| {
        let conn = state.conn.lock().map_err(|_| "DB lock poisoned".to_string())?;
        let item = db::insert_clip(
            &conn,
            &NewClip {
                clip_type: "image",
                text_content: Some(&dims_text),
                html_content: None,
                image_path: Some(&path_str),
                content_size: png_bytes.len() as i64,
                source_app: source_app.as_deref(),
                source_icon_path: source_icon_path.as_deref(),
                source_site,
                created_at,
            },
        )?;
        let removed = db::enforce_retention_and_limits(&conn)?;
        if !removed.is_empty() {
            let _ = app.emit("klip://clips-pruned", removed);
        }
        Ok(item)
    })();

    if let Ok(item) = inserted {
        let _ = app.emit("klip://clip-added", item);
    }
}

fn handle_clipboard_update(hwnd: HWND, ctx: &mut ListenerContext) {
    if IGNORE_SELF_COPY.swap(false, Ordering::SeqCst) {
        return;
    }

    let state = ctx.app.state::<AppState>();
    if state.paused_flag.load(Ordering::Relaxed) {
        return;
    }

    let (fg_hwnd, owner_hwnd) =
        unsafe { (GetForegroundWindow() as isize, GetClipboardOwner() as isize) };
    let created_at = db::now_unix_ms();

    let payload: Option<RawClipboardPayload> = {
        let _lock = match ClipboardLock::acquire(hwnd) {
            Some(l) => l,
            None => return,
        };

        unsafe {
            if IsClipboardFormatAvailable(CF_HDROP) != 0 {
                return;
            }
        }

        if is_privacy_blocked(&ctx.formats) {
            return;
        }

        let raw_html = read_raw_html_format(ctx.formats.html);

        unsafe {
            if ctx.formats.png != 0 && IsClipboardFormatAvailable(ctx.formats.png) != 0 {
                read_global_bytes(GetClipboardData(ctx.formats.png)).map(|png_bytes| {
                    RawClipboardPayload::ImagePng {
                        png_bytes,
                        raw_html,
                        fg_hwnd,
                        owner_hwnd,
                        created_at,
                    }
                })
            } else if IsClipboardFormatAvailable(CF_DIBV5) != 0 {
                read_global_bytes(GetClipboardData(CF_DIBV5)).map(|dib_bytes| {
                    RawClipboardPayload::ImageDib {
                        dib_bytes,
                        raw_html,
                        fg_hwnd,
                        owner_hwnd,
                        created_at,
                    }
                })
            } else if IsClipboardFormatAvailable(CF_DIB) != 0 {
                read_global_bytes(GetClipboardData(CF_DIB)).map(|dib_bytes| {
                    RawClipboardPayload::ImageDib {
                        dib_bytes,
                        raw_html,
                        fg_hwnd,
                        owner_hwnd,
                        created_at,
                    }
                })
            } else {
                let text = read_unicode_text();
                if text.is_some() || raw_html.is_some() {
                    Some(RawClipboardPayload::Text {
                        text,
                        raw_html,
                        fg_hwnd,
                        owner_hwnd,
                        created_at,
                    })
                } else {
                    None
                }
            }
        }
    };

    if let Some(p) = payload {
        if let Some(tx) = ctx.worker_tx.as_ref() {
            let _ = tx.send(p);
        }
    }
}

unsafe fn register_thread_hotkey(app: &AppHandle, spec: &str) {
    UnregisterHotKey(null_mut(), HOTKEY_POPUP_ID);
    let state = app.state::<AppState>();
    if let Some((mods, vk)) = parse_hotkey_string(spec) {
        let ok = RegisterHotKey(null_mut(), HOTKEY_POPUP_ID, mods, vk) != 0;
        state.hotkey_registered.store(ok, Ordering::SeqCst);
        let _ = app.emit("klip://hotkey-status", ok);
    } else {
        state.hotkey_registered.store(false, Ordering::SeqCst);
        let _ = app.emit("klip://hotkey-status", false);
    }
}

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_CLIPBOARDUPDATE => {
            LISTENER_CTX.with(|cell| {
                if let Some(ctx) = cell.borrow_mut().as_mut() {
                    handle_clipboard_update(hwnd, ctx);
                }
            });
            0
        }
        WM_APP_RELOAD_HOTKEY => {
            LISTENER_CTX.with(|cell| {
                if let Some(ctx) = cell.borrow().as_ref() {
                    let state = ctx.app.state::<AppState>();
                    let spec = state
                        .conn
                        .lock()
                        .ok()
                        .and_then(|conn| db::get_setting(&conn, "hotkey").ok().flatten())
                        .unwrap_or_else(|| "Ctrl+Shift+Backquote".to_string());
                    register_thread_hotkey(&ctx.app, &spec);
                }
            });
            0
        }
        WM_CLOSE => {
            UnregisterHotKey(null_mut(), HOTKEY_POPUP_ID);
            RemoveClipboardFormatListener(hwnd);
            DestroyWindow(hwnd);
            0
        }
        WM_DESTROY => {
            LISTENER_CTX.with(|cell| {
                *cell.borrow_mut() = None;
            });
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

pub fn start_clipboard_listener(app: AppHandle, initial_hotkey: String) -> ClipboardWatcherHandle {
    let hwnd_holder = Arc::new(AtomicIsize::new(0));
    let hwnd_for_thread = Arc::clone(&hwnd_holder);

    let (tx, rx) = mpsc::channel::<RawClipboardPayload>();
    let worker_app = app.clone();

    thread::Builder::new()
        .name("klip-capture-worker".to_string())
        .spawn(move || {
            unsafe {
                let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            }
            let mut cached_icons: HashMap<String, Option<String>> = HashMap::new();
            while let Ok(payload) = rx.recv() {
                process_payload(&worker_app, payload, &mut cached_icons);
            }
            unsafe {
                CoUninitialize();
            }
        })
        .expect("Failed to spawn Klip capture worker thread");

    thread::Builder::new()
        .name("klip-clipboard-listener".to_string())
        .spawn(move || unsafe {
            let hinstance = GetModuleHandleW(null());
            let class_name = to_wide_null("KlipClipboardListenerClass");
            let window_title = to_wide_null("KlipClipboardListener");

            let mut wc: WNDCLASSEXW = zeroed();
            wc.cbSize = size_of::<WNDCLASSEXW>() as u32;
            wc.lpfnWndProc = Some(wnd_proc);
            wc.hInstance = hinstance;
            wc.lpszClassName = class_name.as_ptr();

            RegisterClassExW(&wc);

            let hwnd = CreateWindowExW(
                WS_EX_NOACTIVATE_FLAG | WS_EX_TOOLWINDOW_FLAG,
                class_name.as_ptr(),
                window_title.as_ptr(),
                0,
                0,
                0,
                0,
                0,
                HWND_MESSAGE,
                null_mut(),
                hinstance,
                null(),
            );

            if hwnd.is_null() {
                return;
            }

            hwnd_for_thread.store(hwnd as isize, Ordering::SeqCst);

            let formats = FormatIds {
                html: register_format("HTML Format"),
                png: register_format("PNG"),
                exclude_monitor: register_format("ExcludeClipboardContentFromMonitorProcessing"),
                viewer_ignore: register_format("Clipboard Viewer Ignore"),
                can_include_history: register_format("CanIncludeInClipboardHistory"),
            };

            let app_for_ctx = app.clone();
            LISTENER_CTX.with(|cell| {
                *cell.borrow_mut() = Some(ListenerContext {
                    app: app_for_ctx,
                    formats,
                    worker_tx: Some(tx),
                });
            });

            if AddClipboardFormatListener(hwnd) == FALSE {
                DestroyWindow(hwnd);
                return;
            }

            // Register thread-level hotkey (HWND = NULL -> guaranteed WM_HOTKEY delivery)
            register_thread_hotkey(&app, &initial_hotkey);

            let mut msg = zeroed();
            while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
                if msg.message == WM_HOTKEY && msg.wParam as i32 == HOTKEY_POPUP_ID {
                    let fg_hwnd = GetForegroundWindow() as isize;
                    let app_c = app.clone();
                    let _ = app.run_on_main_thread(move || {
                        toggle_popup_window(&app_c, fg_hwnd);
                    });
                    continue;
                }
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }

            hwnd_for_thread.store(0, Ordering::SeqCst);
        })
        .expect("Failed to spawn Klip clipboard listener thread");

    ClipboardWatcherHandle { hwnd: hwnd_holder }
}