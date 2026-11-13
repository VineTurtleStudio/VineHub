use std::path::Path;
use std::process::{Command, Stdio};

fn hide_window(cmd: &mut Command) {
    use std::os::windows::process::CommandExt;
    cmd.creation_flags(0x08000000);
}

fn ensure_exists(path: &str) -> Result<(), String> {
    if !Path::new(path).exists() {
        return Err(format!("路径不存在: {path}"));
    }
    Ok(())
}

fn win_join_normalize(path: &str) -> String {
    let s = path.replace('/', "\\");
    let is_unc = s.starts_with("\\\\");
    let mut out = String::with_capacity(s.len());
    let mut prev_sep = false;
    for ch in s.chars() {
        if ch == '\\' {
            if prev_sep && !(is_unc && out.len() < 2) {
                continue;
            }
            prev_sep = true;
        } else {
            prev_sep = false;
        }
        out.push(ch);
    }
    out
}

pub fn open_path(path: &str) -> Result<(), String> {
    let path = win_join_normalize(path);
    ensure_exists(&path)?;
    let mut cmd = Command::new("cmd");
    cmd.args(["/C", "start", ""])
        .arg(&path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    hide_window(&mut cmd);
    cmd.spawn().map_err(|e| format!("打开失败: {e}"))?;
    Ok(())
}

pub fn reveal_path(path: &str) -> Result<(), String> {
    let path = win_join_normalize(path);
    ensure_exists(&path)?;
    if reveal_select_api(&path).is_ok() {
        return Ok(());
    }
    use std::os::windows::process::CommandExt;
    let mut cmd = Command::new("explorer");
    cmd.raw_arg(format!("/select,\"{path}\""))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    hide_window(&mut cmd);
    cmd.spawn().map_err(|e| format!("定位失败: {e}"))?;
    Ok(())
}

fn reveal_select_api(path: &str) -> Result<(), String> {
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;

    #[link(name = "ole32")]
    unsafe extern "system" {
        fn CoInitializeEx(reserved: *mut c_void, coinit: u32) -> i32;
        fn CoUninitialize();
    }
    #[link(name = "shell32")]
    unsafe extern "system" {
        fn ILCreateFromPathW(psz_path: *const u16) -> *mut c_void;
        fn ILFree(pidl: *mut c_void);
        fn SHOpenFolderAndSelectItems(
            pidl_folder: *const c_void,
            cidl: u32,
            apidl: *const *const c_void,
            flags: u32,
        ) -> i32;
    }

    const COINIT_APARTMENTTHREADED: u32 = 0x2;
    const RPC_E_CHANGED_MODE: i32 = 0x8001_0106u32 as i32;

    let wide = |s: &str| -> Vec<u16> {
        std::ffi::OsStr::new(s)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    };

    let p = Path::new(path);
    let dir = p
        .parent()
        .ok_or_else(|| format!("定位失败: 无法取父目录 {path}"))?
        .to_string_lossy()
        .into_owned();

    unsafe {
        let hr = CoInitializeEx(std::ptr::null_mut(), COINIT_APARTMENTTHREADED);
        if hr < 0 && hr != RPC_E_CHANGED_MODE {
            return Err(format!("定位失败: CoInitializeEx 0x{:08X}", hr as u32));
        }
        let need_uninit = hr >= 0;

        let dir_w = wide(&dir);
        let file_w = wide(path);
        let dir_pidl = ILCreateFromPathW(dir_w.as_ptr());
        let file_pidl = ILCreateFromPathW(file_w.as_ptr());
        let mut ok = false;
        if !dir_pidl.is_null() && !file_pidl.is_null() {
            let items: [*const c_void; 1] = [file_pidl];
            let hr2 = SHOpenFolderAndSelectItems(dir_pidl, 1, items.as_ptr(), 0);
            ok = hr2 >= 0;
        }
        if !dir_pidl.is_null() {
            ILFree(dir_pidl);
        }
        if !file_pidl.is_null() {
            ILFree(file_pidl);
        }
        if need_uninit {
            CoUninitialize();
        }
        if ok {
            Ok(())
        } else {
            Err(format!("定位失败: ILCreateFromPathW {path}"))
        }
    }
}
