use std::collections::HashMap;
use std::ffi::c_void;
use std::fs;
use std::io::BufWriter;
use std::mem::{size_of, zeroed};
use std::path::{Path, PathBuf};
use std::ptr::null_mut;
use windows::core::HSTRING;
use windows::Win32::Foundation::SIZE;
use windows::Win32::UI::Shell::{
    IShellItemImageFactory, SHCreateItemFromParsingName, SIIGBF_ICONONLY, SIIGBF_RESIZETOFIT,
};
use windows_sys::Win32::Foundation::{BOOL, CloseHandle, FALSE, HWND, LPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    CreateCompatibleDC, DeleteDC, DeleteObject, GetDIBits, GetObjectW, BITMAP, BITMAPINFO,
    BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HBITMAP,
};
use windows_sys::Win32::System::DataExchange::GetClipboardOwner;
use windows_sys::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows_sys::Win32::UI::Shell::ExtractIconExW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumChildWindows, GetClassNameW, GetIconInfo, GetWindowThreadProcessId, DestroyIcon, HICON,
    ICONINFO,
};

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

pub fn resolve_source_from_hwnds(fg_raw: isize, owner_raw: isize) -> Option<(String, PathBuf)> {
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

        let fallback_owner = if owner_raw != 0 {
            owner_raw as HWND
        } else {
            GetClipboardOwner()
        };
        if !fallback_owner.is_null() && fallback_owner != primary {
            let mut owner_pid: u32 = 0;
            GetWindowThreadProcessId(fallback_owner, &mut owner_pid);
            return query_process_image(owner_pid);
        }

        None
    }
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

pub fn encode_rgba_png(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>, String> {
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

unsafe fn extract_with_shell_factory(exe_path: &Path) -> Option<Vec<u8>> {
    let path_str = exe_path.to_string_lossy();
    let hstr = HSTRING::from(path_str.as_ref());
    let factory: IShellItemImageFactory = SHCreateItemFromParsingName(&hstr, None).ok()?;
    let hbmp = factory
        .GetImage(
            SIZE { cx: 32, cy: 32 },
            SIIGBF_ICONONLY | SIIGBF_RESIZETOFIT,
        )
        .ok()?;
    if hbmp.0.is_null() {
        return None;
    }

    let sys_hbmp = hbmp.0 as HBITMAP;
    let res = hbitmap_to_png(sys_hbmp, true).ok();
    DeleteObject(sys_hbmp as _);
    res
}

unsafe fn extract_with_extract_icon_ex(exe_path: &Path) -> Option<Vec<u8>> {
    let wide_path: Vec<u16> = exe_path
        .to_string_lossy()
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let mut large_icon: HICON = null_mut();
    let count = ExtractIconExW(wide_path.as_ptr(), 0, &mut large_icon, null_mut(), 1);
    if count == 0 || large_icon.is_null() {
        return None;
    }
    let res = hicon_to_png(large_icon).ok();
    DestroyIcon(large_icon);
    res
}

unsafe fn hbitmap_to_png(hbitmap: HBITMAP, unpremultiply: bool) -> Result<Vec<u8>, String> {
    let mut bmp: BITMAP = zeroed();
    if GetObjectW(
        hbitmap as _,
        size_of::<BITMAP>() as i32,
        &mut bmp as *mut _ as *mut c_void,
    ) == 0
    {
        return Err("GetObjectW failed".into());
    }

    let w = bmp.bmWidth;
    let h = bmp.bmHeight.abs();
    if w <= 0 || h <= 0 || w > 512 || h > 512 {
        return Err("Invalid bitmap dimensions".into());
    }

    let hdc = CreateCompatibleDC(null_mut());
    if hdc.is_null() {
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
        hbitmap,
        0,
        h as u32,
        bgra.as_mut_ptr() as *mut c_void,
        &mut bmi,
        DIB_RGB_COLORS,
    );
    DeleteDC(hdc);

    if lines == 0 {
        return Err("GetDIBits failed".into());
    }

    let mut has_alpha = false;
    for px in bgra.chunks_exact_mut(4) {
        px.swap(0, 2); // BGRA -> RGBA
        let a = px[3];
        if a != 0 {
            has_alpha = true;
            if unpremultiply && a < 255 {
                px[0] = ((px[0] as u16 * 255) / a as u16).min(255) as u8;
                px[1] = ((px[1] as u16 * 255) / a as u16).min(255) as u8;
                px[2] = ((px[2] as u16 * 255) / a as u16).min(255) as u8;
            }
        }
    }

    if !has_alpha {
        for px in bgra.chunks_exact_mut(4) {
            px[3] = 255;
        }
    }

    encode_rgba_png(w as u32, h as u32, &bgra)
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

pub fn ensure_app_icon(
    exe_name: &str,
    exe_path: &Path,
    icons_dir: &Path,
    cache: &mut HashMap<String, Option<String>>,
) -> Option<String> {
    let key = exe_name.trim().to_lowercase();
    if let Some(cached) = cache.get(&key) {
        if let Some(p) = cached {
            if Path::new(p).exists() {
                return Some(p.clone());
            }
        } else {
            return None;
        }
    }

    let stem = key.strip_suffix(".exe").unwrap_or(&key);
    let file_name = format!("{}.png", sanitize_filename(stem));
    let dest_path = icons_dir.join(&file_name);
    let dest_str = dest_path.to_string_lossy().into_owned();

    if dest_path.exists() {
        cache.insert(key, Some(dest_str.clone()));
        return Some(dest_str);
    }

    let png_opt = unsafe {
        extract_with_shell_factory(exe_path).or_else(|| extract_with_extract_icon_ex(exe_path))
    };

    let result = match png_opt {
        Some(ref bytes) if fs::write(&dest_path, bytes).is_ok() => Some(dest_str),
        _ => None,
    };

    cache.insert(key, result.clone());
    result
}