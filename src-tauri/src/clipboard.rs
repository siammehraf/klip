use crate::db::{self, ClipItem, NewClip};
use crate::AppState;
use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::c_void;
use std::fs;
use std::io::{BufWriter, Cursor};
use std::mem::{size_of, zeroed};
use std::path::{Path, PathBuf};
use std::ptr::{copy_nonoverlapping, null, null_mut};
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};
use windows_sys::Win32::Foundation::{BOOL, CloseHandle, FALSE, HANDLE, HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    CreateCompatibleDC, DeleteDC, DeleteObject, GetDIBits, GetObjectW, BITMAP, BITMAPINFO,
    BITMAPINFOHEADER, BI_BITFIELDS, BI_RGB, DIB_RGB_COLORS,
};
use windows_sys::Win32::System::DataExchange::{
    AddClipboardFormatListener, CloseClipboard, EmptyClipboard, GetClipboardData,
    GetClipboardOwner, IsClipboardFormatAvailable, OpenClipboard, RegisterClipboardFormatW,
    RemoveClipboardFormatListener, SetClipboardData,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Memory::{
    GlobalAlloc, GlobalFree, GlobalLock, GlobalSize, GlobalUnlock,
};
use windows_sys::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows_sys::Win32::UI::Shell::{
    ExtractIconExW, SHGetFileInfoW, SHFILEINFOW, SHGFI_ICON, SHGFI_LARGEICON,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyIcon, DestroyWindow, DispatchMessageW,
    EnumChildWindows, GetClassNameW, GetForegroundWindow, GetIconInfo, GetMessageW,
    GetWindowThreadProcessId, PostMessageW, PostQuitMessage, RegisterClassExW, TranslateMessage,
    HICON, HWND_MESSAGE, ICONINFO, WM_CLOSE, WM_DESTROY, WNDCLASSEXW,
};

const CF_DIB: u32 = 8;
const CF_UNICODETEXT: u32 = 13;
const CF_HDROP: u32 = 15;
const CF_DIBV5: u32 = 17;
const WM_CLIPBOARDUPDATE: u32 = 0x031D;
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
    let start_frag = full.find("<!--StartFragment-->").map(|i| i + 20).unwrap_or(start_html);
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

    // BITMAPINFOHEADER (40 bytes, bottom-up BGRA)
    dib.extend_from_slice(&40u32.to_le_bytes());
    dib.extend_from_slice(&w.to_le_bytes());
    dib.extend_from_slice(&h.to_le_bytes());
    dib.extend_from_slice(&1u16.to_le_bytes());
    dib.extend_from_slice(&32u16.to_le_bytes());
    dib.extend_from_slice(&0u32.to_le_bytes()); // BI_RGB
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
        let row = &samples[y * (info.width as usize) * src_bpp..(y + 1) * (info.width as usize) * src_bpp];
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

struct ChildAppSearch {
    host_pid: u32,
    core_pid: u32,
    any_other_pid: u32,
}

unsafe extern "system" fn enum_frame_child_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let search = &mut *(lparam as *mut ChildAppSearch);
    let mut pid: u32 = 0;
    GetWindowThreadProcessId(hwnd, &mut pid);
    if pid != 0 && pid != search.host_pid {
        let mut cls = [0u16; 128];
        let len = GetClassNameW(hwnd, cls.as_mut_ptr(), cls.len() as i32);
        if len > 0 {
            let name = String::from_utf16_lossy(&cls[..len as usize]);
            if name == "Windows.UI.Core.CoreWindow" {
                search.core_pid = pid;
                return 0;
            }
        }
        if search.any_other_pid == 0 {
            search.any_other_pid = pid;
        }
    }
    1
}

unsafe fn query_process_image(pid: u32) -> Option<(String, PathBuf)> {
    if pid == 0 {
        return None;
    }
    let proc_handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, FALSE, pid);
    if proc_handle.is_null() {
        return None;
    }

    let mut buf = vec![0u16; 1024];
    let mut len = buf.len() as u32;
    let ok = QueryFullProcessImageNameW(proc_handle, 0, buf.as_mut_ptr(), &mut len);
    CloseHandle(proc_handle);

    if ok == 0 || len == 0 {
        return None;
    }

    let full_path = String::from_utf16_lossy(&buf[..len as usize]);
    let path = PathBuf::from(&full_path);
    let exe_name = path.file_name()?.to_str()?.trim().to_string();
    if exe_name.is_empty() {
        return None;
    }

    Some((exe_name, path))
}

fn resolve_source_from_hwnds(fg_raw: isize, owner_raw: isize) -> Option<(String, PathBuf)> {
    unsafe {
        let primary = if fg_raw != 0 {
            fg_raw as HWND
        } else {
            owner_raw as HWND
        };
        if primary.is_null() {
            return None;
        }

        let mut pid: u32 = 0;
        GetWindowThreadProcessId(primary, &mut pid);
        if pid != 0 {
            let mut cls = [0u16; 128];
            let cls_len = GetClassNameW(primary, cls.as_mut_ptr(), cls.len() as i32);
            let cls_name = if cls_len > 0 {
                String::from_utf16_lossy(&cls[..cls_len as usize])
            } else {
                String::new()
            };

            if cls_name == "ApplicationFrameWindow" {
                let mut search = ChildAppSearch {
                    host_pid: pid,
                    core_pid: 0,
                    any_other_pid: 0,
                };
                EnumChildWindows(
                    primary,
                    Some(enum_frame_child_proc),
                    &mut search as *mut _ as LPARAM,
                );
                let real_pid = if search.core_pid != 0 {
                    search.core_pid
                } else if search.any_other_pid != 0 {
                    search.any_other_pid
                } else {
                    pid
                };
                if let Some(info) = query_process_image(real_pid) {
                    return Some(info);
                }
            }

            if let Some(info) = query_process_image(pid) {
                return Some(info);
            }
        }

        if owner_raw != 0 && owner_raw != fg_raw {
            let mut owner_pid: u32 = 0;
            GetWindowThreadProcessId(owner_raw as HWND, &mut owner_pid);
            return query_process_image(owner_pid);
        }

        None
    }
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

fn extract_source_site(raw_html: &str, exe_name: Option<&str>) -> Option<String> {
    if let Some(exe) = exe_name {
        if !is_browser_exe(exe) {
            return None;
        }
    }

    for line in raw_html.lines().take(20) {
        let trimmed = line.trim();
        if let Some(url) = trimmed
            .strip_prefix("SourceURL:")
            .or_else(|| trimmed.strip_prefix("sourceurl:"))
        {
            let url = url.trim();
            let after_scheme = url
                .strip_prefix("https://")
                .or_else(|| url.strip_prefix("http://"))?;
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
            return Some(format!("site:{clean_host}"));
        }
    }
    None
}

fn sanitize_filename(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn encode_rgba_png(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    {
        let bw = BufWriter::new(&mut out);
        let mut encoder = png::Encoder::new(bw, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder
            .write_header()
            .map_err(|e| format!("PNG header error: {e}"))?;
        writer
            .write_image_data(rgba)
            .map_err(|e| format!("PNG write error: {e}"))?;
    }
    Ok(out)
}

fn ensure_app_icon(
    exe_name: &str,
    exe_path: &Path,
    icons_dir: &Path,
    cache: &mut HashMap<String, Option<String>>,
) -> Option<String> {
    let key = normalize_exe_name(exe_name);
    if let Some(cached) = cache.get(&key) {
        return cached.clone();
    }

    let file_name = format!("{}.png", sanitize_filename(&key));
    let dest_path = icons_dir.join(&file_name);
    let dest_str = dest_path.to_string_lossy().into_owned();

    if dest_path.exists() {
        cache.insert(key, Some(dest_str.clone()));
        return Some(dest_str);
    }

    let wide_path = to_wide_null(&exe_path.to_string_lossy());
    let mut extracted_path: Option<String> = None;

    unsafe {
        let mut shfi: SHFILEINFOW = zeroed();
        let res = SHGetFileInfoW(
            wide_path.as_ptr(),
            0,
            &mut shfi,
            size_of::<SHFILEINFOW>() as u32,
            SHGFI_ICON | SHGFI_LARGEICON,
        );
        let mut hicon = if res != 0 && !shfi.hIcon.is_null() {
            shfi.hIcon
        } else {
            null_mut()
        };

        if hicon.is_null() {
            let mut large_icon: HICON = null_mut();
            let count = ExtractIconExW(wide_path.as_ptr(), 0, &mut large_icon, null_mut(), 1);
            if count > 0 && !large_icon.is_null() {
                hicon = large_icon;
            }
        }

        if !hicon.is_null() {
            let png_bytes = hicon_to_png(hicon);
            DestroyIcon(hicon);

            if let Ok(bytes) = png_bytes {
                if fs::write(&dest_path, bytes).is_ok() {
                    extracted_path = Some(dest_str);
                }
            }
        }
    }

    cache.insert(key, extracted_path.clone());
    extracted_path
}

unsafe fn hicon_to_png(hicon: HICON) -> Result<Vec<u8>, String> {
    let mut icon_info: ICONINFO = zeroed();
    if GetIconInfo(hicon, &mut icon_info) == 0 {
        return Err("GetIconInfo failed".into());
    }

    let cleanup = |info: &ICONINFO| {
        if !info.hbmColor.is_null() {
            DeleteObject(info.hbmColor as _);
        }
        if !info.hbmMask.is_null() {
            DeleteObject(info.hbmMask as _);
        }
    };

    if icon_info.hbmColor.is_null() {
        cleanup(&icon_info);
        return Err("No color bitmap in HICON".into());
    }

    let mut bmp: BITMAP = zeroed();
    if GetObjectW(
        icon_info.hbmColor as _,
        size_of::<BITMAP>() as i32,
        &mut bmp as *mut _ as *mut c_void,
    ) == 0
    {
        cleanup(&icon_info);
        return Err("GetObjectW failed".into());
    }

    let w = bmp.bmWidth;
    let h = bmp.bmHeight.abs();
    if w <= 0 || h <= 0 || w > 512 || h > 512 {
        cleanup(&icon_info);
        return Err("Invalid icon dimensions".into());
    }

    let hdc = CreateCompatibleDC(null_mut());
    if hdc.is_null() {
        cleanup(&icon_info);
        return Err("CreateCompatibleDC failed".into());
    }

    let mut bmi: BITMAPINFO = zeroed();
    bmi.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
    bmi.bmiHeader.biWidth = w;
    bmi.bmiHeader.biHeight = -h;
    bmi.bmiHeader.biPlanes = 1;
    bmi.bmiHeader.biBitCount = 32;
    bmi.bmiHeader.biCompression = BI_RGB;

    let mut bgra = vec![0u8; (w * h * 4) as usize];
    let lines = GetDIBits(
        hdc,
        icon_info.hbmColor,
        0,
        h as u32,
        bgra.as_mut_ptr() as *mut c_void,
        &mut bmi,
        DIB_RGB_COLORS,
    );

    let mut has_alpha = false;
    for px in bgra.chunks_exact_mut(4) {
        px.swap(0, 2);
        if px[3] != 0 {
            has_alpha = true;
        }
    }

    if !has_alpha && !icon_info.hbmMask.is_null() {
        let mut mask_bgra = vec![0u8; (w * h * 4) as usize];
        let mask_lines = GetDIBits(
            hdc,
            icon_info.hbmMask,
            0,
            h as u32,
            mask_bgra.as_mut_ptr() as *mut c_void,
            &mut bmi,
            DIB_RGB_COLORS,
        );
        if mask_lines > 0 {
            for (px, m) in bgra.chunks_exact_mut(4).zip(mask_bgra.chunks_exact(4)) {
                px[3] = if m[0] == 0 { 255 } else { 0 };
            }
        } else {
            for px in bgra.chunks_exact_mut(4) {
                px[3] = 255;
            }
        }
    }

    DeleteDC(hdc);
    cleanup(&icon_info);

    if lines == 0 {
        return Err("GetDIBits failed".into());
    }

    encode_rgba_png(w as u32, h as u32, &bgra)
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

fn detect_text_clip_type(text: &str, has_html: bool) -> &'static str {
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
    if has_html {
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

    // Resolve process info & check exclusions completely off the Win32 listener thread
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

    let source_site = raw_html_opt.and_then(|raw| {
        extract_source_site(raw, source_proc.as_ref().map(|(name, _)| name.as_str()))
    });

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

            let text_bytes = text.as_ref().map(|s| s.len()).unwrap_or(0);
            let html_bytes = html_frag.as_ref().map(|s| s.len()).unwrap_or(0);
            let total_size = text_bytes + html_bytes;

            if total_size == 0 || total_size > max_bytes {
                return;
            }

            let clip_type = match text.as_deref() {
                Some(t) => detect_text_clip_type(t, html_frag.is_some()),
                None => "rich_text",
            };

            let (source_app, source_icon_path) = match source_proc {
                Some((exe_name, exe_path)) => {
                    let icon = ensure_app_icon(
                        &exe_name,
                        &exe_path,
                        &state.app_icons_dir,
                        cached_icons,
                    );
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
                        html_content: html_frag.as_deref(),
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
            let icon = ensure_app_icon(
                &exe_name,
                &exe_path,
                &state.app_icons_dir,
                cached_icons,
            );
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
    // Ignore copies triggered programmatically by Klip itself
    if IGNORE_SELF_COPY.swap(false, Ordering::SeqCst) {
        return;
    }

    let state = ctx.app.state::<AppState>();
    if state.paused_flag.load(Ordering::Relaxed) {
        return;
    }

    // Grab lightweight window handles only; process/icon/domain work happens in worker thread
    let (fg_hwnd, owner_hwnd) = unsafe { (GetForegroundWindow() as isize, GetClipboardOwner() as isize) };
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
    }; // _lock drops here -> CloseClipboard() immediately

    if let Some(p) = payload {
        if let Some(tx) = ctx.worker_tx.as_ref() {
            let _ = tx.send(p);
        }
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
        WM_CLOSE => {
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

pub fn start_clipboard_listener(app: AppHandle) -> ClipboardWatcherHandle {
    let hwnd_holder = Arc::new(AtomicIsize::new(0));
    let hwnd_for_thread = Arc::clone(&hwnd_holder);

    let (tx, rx) = mpsc::channel::<RawClipboardPayload>();
    let worker_app = app.clone();

    thread::Builder::new()
        .name("klip-capture-worker".to_string())
        .spawn(move || {
            let mut cached_icons: HashMap<String, Option<String>> = HashMap::new();
            while let Ok(payload) = rx.recv() {
                process_payload(&worker_app, payload, &mut cached_icons);
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

            LISTENER_CTX.with(|cell| {
                *cell.borrow_mut() = Some(ListenerContext {
                    app,
                    formats,
                    worker_tx: Some(tx),
                });
            });

            if AddClipboardFormatListener(hwnd) == FALSE {
                DestroyWindow(hwnd);
                return;
            }

            // Strictly blocking GetMessageW loop (0.00% idle CPU)
            let mut msg = zeroed();
            while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }

            hwnd_for_thread.store(0, Ordering::SeqCst);
        })
        .expect("Failed to spawn Klip clipboard listener thread");

    ClipboardWatcherHandle { hwnd: hwnd_holder }
}