use std::ffi::c_void;
use std::os::windows::ffi::OsStrExt;
use std::path::PathBuf;
use std::time::Duration;

use windows_sys::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, OpenClipboard, RegisterClipboardFormatW,
    SetClipboardData,
};
use windows_sys::Win32::System::Memory::{
    GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE,
};
use windows_sys::Win32::System::Ole::{CF_HDROP, CF_UNICODETEXT};

const CF_HDROP_U32: u32 = CF_HDROP as u32;
const CF_UNICODETEXT_U32: u32 = CF_UNICODETEXT as u32;

const EXCLUDE_FORMAT: &str = "ExcludeClipboardFromMonitorProcessing";

fn wide(s: &std::ffi::OsStr) -> Vec<u16> {
    s.encode_wide().chain(std::iter::once(0)).collect()
}

fn open_clipboard() -> bool {
    for _ in 0..3 {
        if unsafe { OpenClipboard(std::ptr::null_mut()) } != 0 {
            return true;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    false
}

fn hglobal_from_u16(data: &[u16]) -> Option<*mut c_void> {
    let bytes: Vec<u8> = data.iter().flat_map(|w| w.to_le_bytes()).collect();
    unsafe {
        let h = GlobalAlloc(GMEM_MOVEABLE, bytes.len());
        if h.is_null() {
            return None;
        }
        let p = GlobalLock(h);
        if p.is_null() {
            return None;
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), p as *mut u8, bytes.len());
        GlobalUnlock(h);
        Some(h)
    }
}

fn add_exclude_marker() {
    let name = wide(std::ffi::OsStr::new(EXCLUDE_FORMAT));
    let fmt = unsafe { RegisterClipboardFormatW(name.as_ptr()) };
    if fmt == 0 {
        return;
    }
    let val: Vec<u16> = wide(std::ffi::OsStr::new("1"));
    if let Some(h) = hglobal_from_u16(&val) {
        unsafe { SetClipboardData(fmt, h) };
    }
}

pub fn set_text(text: &str) {
    let data: Vec<u16> = wide(std::ffi::OsStr::new(text));
    if !open_clipboard() {
        return;
    }
    unsafe {
        EmptyClipboard();
        if let Some(h) = hglobal_from_u16(&data) {
            SetClipboardData(CF_UNICODETEXT_U32, h);
        }
        add_exclude_marker();
        CloseClipboard();
    }
}

pub fn encode_hdrop(paths: &[PathBuf]) -> Vec<u16> {
    let mut data: Vec<u16> = vec![0u16; 10];
    data[0] = 20;
    data[8] = 1;
    for p in paths {
        data.extend(wide(p.as_os_str()));
    }
    data.push(0);
    data
}

pub unsafe fn decode_hdrop(base: *const u8, size: usize) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if base.is_null() || size < 20 {
        return out;
    }
    unsafe {
        let p_files = *(base as *const u32) as usize;
        if p_files >= size {
            return out;
        }
        let mut cur = base.add(p_files) as *const u16;
        let end = base.add(size) as *const u16;
        while cur < end {
            let mut w: Vec<u16> = Vec::new();
            while cur < end && *cur != 0 {
                w.push(*cur);
                cur = cur.add(1);
            }
            if cur >= end {
                break;
            }
            cur = cur.add(1);
            if w.is_empty() {
                break;
            }
            out.push(PathBuf::from(String::from_utf16_lossy(&w)));
        }
    }
    out
}

pub fn set_paths(paths: &[PathBuf]) {
    if paths.is_empty() {
        return;
    }
    let data = encode_hdrop(paths);
    if !open_clipboard() {
        return;
    }
    unsafe {
        EmptyClipboard();
        if let Some(h) = hglobal_from_u16(&data) {
            SetClipboardData(CF_HDROP_U32, h);
        }
        add_exclude_marker();
        CloseClipboard();
    }
}

pub fn get_paths() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if !open_clipboard() {
        return out;
    }
    unsafe {
        let h = GetClipboardData(CF_HDROP_U32);
        if !h.is_null() {
            let base = GlobalLock(h) as *const u8;
            if !base.is_null() {
                let size = GlobalSize(h);
                out = decode_hdrop(base, size);
            }
            GlobalUnlock(h);
        }
        CloseClipboard();
    }
    out
}

pub fn take_copy_text(commands: &mut Vec<egui::OutputCommand>) -> Vec<String> {
    let mut texts = Vec::new();
    let taken = std::mem::take(commands);
    for c in taken {
        match c {
            egui::OutputCommand::CopyText(t) => texts.push(t),
            other => commands.push(other),
        }
    }
    texts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hdrop_roundtrip() {
        let paths = vec![
            PathBuf::from(r"C:\Temp\alpha.txt"),
            PathBuf::from(r"C:\Temp\目录 B\beta.bin"),
        ];
        let data = encode_hdrop(&paths);
        let bytes: Vec<u8> = data.iter().flat_map(|w| w.to_le_bytes()).collect();
        let got = unsafe { decode_hdrop(bytes.as_ptr(), bytes.len()) };
        assert_eq!(got, paths);
    }

    #[test]
    fn decode_rejects_short_buffer() {
        let got = unsafe { decode_hdrop([0u8; 4].as_ptr(), 4) };
        assert!(got.is_empty());
    }
}
