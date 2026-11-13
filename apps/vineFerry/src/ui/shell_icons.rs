use std::collections::HashMap;
use std::ffi::c_void;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use windows_sys::Win32::Foundation::FALSE;
use windows_sys::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, CreateDIBSection, DeleteDC,
    DeleteObject, DIB_RGB_COLORS, SelectObject,
};
use windows_sys::Win32::Storage::FileSystem::{FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL};
use windows_sys::Win32::UI::Shell::{
    SHGetFileInfoW, SHFILEINFOW, SHGFI_ICON, SHGFI_LARGEICON, SHGFI_SMALLICON, SHGFI_USEFILEATTRIBUTES,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DestroyIcon, DrawIconEx, DI_NORMAL, GetIconInfo, HICON, ICONINFO,
};

const CACHE_MAX: usize = 256;

pub struct ShellIcons {
    map: HashMap<String, Option<egui::TextureHandle>>,
    order: Vec<String>,
}

impl ShellIcons {
    pub fn new() -> Self {
        ShellIcons {
            map: HashMap::new(),
            order: Vec::new(),
        }
    }

    pub fn icon_for(
        &mut self,
        ctx: &egui::Context,
        path: &Path,
        is_dir: bool,
        reparse: bool,
        size: u32,
    ) -> Option<egui::TextureHandle> {
        let key = if is_dir {
            format!("d{size}")
        } else if reparse {
            format!(
                "r{}",
                path.to_string_lossy().to_lowercase()
            )
        } else {
            format!(
                "f{size}{}",
                path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default()
            )
        };
        if let Some(v) = self.map.get(&key) {
            return v.clone();
        }
        let tex = load_icon(ctx, path, is_dir, reparse, size);
        if self.order.len() >= CACHE_MAX
            && let Some(old) = self.order.first().cloned()
        {
            self.order.remove(0);
            self.map.remove(&old);
        }
        self.map.insert(key.clone(), tex.clone());
        self.order.push(key);
        tex
    }
}

fn load_icon(
    ctx: &egui::Context,
    path: &Path,
    is_dir: bool,
    reparse: bool,
    size: u32,
) -> Option<egui::TextureHandle> {
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    let attrs = if is_dir {
        FILE_ATTRIBUTE_DIRECTORY
    } else {
        FILE_ATTRIBUTE_NORMAL
    };
    let flags = if reparse {
        SHGFI_ICON
    } else {
        SHGFI_ICON | SHGFI_USEFILEATTRIBUTES
    } | if size <= 16 {
        SHGFI_SMALLICON
    } else {
        SHGFI_LARGEICON
    };
    let mut shfi: SHFILEINFOW = unsafe { std::mem::zeroed() };
    let ok = unsafe {
        SHGetFileInfoW(
            wide.as_ptr(),
            attrs,
            &mut shfi,
            std::mem::size_of::<SHFILEINFOW>() as u32,
            flags,
        )
    };
    if ok == 0 || shfi.hIcon.is_null() {
        return None;
    }
    let img = unsafe { icon_to_image(shfi.hIcon, size as i32) };
    unsafe { DestroyIcon(shfi.hIcon) };
    let color = img?;
    static ICON_SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let seq = ICON_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    Some(ctx.load_texture(
        format!("shell_icon_{seq}"),
        color,
        egui::TextureOptions::LINEAR,
    ))
}

unsafe fn icon_to_image(hicon: HICON, size: i32) -> Option<egui::ColorImage> {
    unsafe {
        let mut icon_info: ICONINFO = std::mem::zeroed();
        if GetIconInfo(hicon, &mut icon_info) == FALSE {
            return None;
        }
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: size,
                biHeight: -size,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB,
                biSizeImage: 0,
                biXPelsPerMeter: 0,
                biYPelsPerMeter: 0,
                biClrUsed: 0,
                biClrImportant: 0,
            },
            bmiColors: [std::mem::zeroed(); 1],
        };
        let mut bits: *mut c_void = std::ptr::null_mut();
        let hdc = CreateCompatibleDC(std::ptr::null_mut());
        if hdc.is_null() {
            cleanup_icon_info(&icon_info);
            return None;
        }
        let hbm =
            CreateDIBSection(hdc, &bmi, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
        if hbm.is_null() || bits.is_null() {
            DeleteDC(hdc);
            cleanup_icon_info(&icon_info);
            return None;
        }
        let old = SelectObject(hdc, hbm);
        DrawIconEx(hdc, 0, 0, hicon, size, size, 0, std::ptr::null_mut(), DI_NORMAL);
        let n = (size * size * 4) as usize;
        let raw = std::slice::from_raw_parts(bits as *const u8, n);
        let mut rgba = Vec::with_capacity(n);
        for px in raw.as_chunks::<4>().0 {
            let b = px[0];
            let g = px[1];
            let r = px[2];
            let a = px[3];
            rgba.extend_from_slice(&[r, g, b, a]);
        }
        SelectObject(hdc, old);
        DeleteObject(hbm);
        DeleteDC(hdc);
        cleanup_icon_info(&icon_info);
        Some(egui::ColorImage::from_rgba_unmultiplied(
            [size as usize, size as usize],
            &rgba,
        ))
    }
}

unsafe fn cleanup_icon_info(ii: &ICONINFO) {
    unsafe {
        if !ii.hbmColor.is_null() {
            DeleteObject(ii.hbmColor);
        }
        if !ii.hbmMask.is_null() {
            DeleteObject(ii.hbmMask);
        }
    }
}
