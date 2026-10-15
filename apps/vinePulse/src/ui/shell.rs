//! 只读 shell 交互：用系统默认程序打开 / 在资源管理器中定位。
//! 移植自 `apps/vineCurator/src-tauri/src/shell.rs`（去掉 tauri 依赖，保留 Windows Shell API 定位链路）。

use std::path::Path;
use std::process::{Command, Stdio};

fn hide_window(cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }
}

fn ensure_exists(path: &str) -> Result<(), String> {
    if !Path::new(path).exists() {
        return Err(format!("路径不存在: {path}"));
    }
    Ok(())
}

/// 统一成 Windows 分隔符并折叠连续 `\`（保留 UNC 开头 `\\server`），否则
/// `explorer /select,F:\\...` 认不出路径回落打开「此电脑」。
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

/// 用系统默认程序打开文件/目录（只读：不修改媒体文件）
pub fn open_path(path: &str) -> Result<(), String> {
    let path = win_join_normalize(path);
    ensure_exists(&path)?;
    #[cfg(windows)]
    {
        let mut cmd = Command::new("cmd");
        cmd.args(["/C", "start", ""])
            .arg(&path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        hide_window(&mut cmd);
        cmd.spawn().map_err(|e| format!("打开失败: {e}"))?;
    }
    #[cfg(not(windows))]
    {
        let mut cmd = Command::new("xdg-open");
        cmd.arg(&path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        hide_window(&mut cmd);
        cmd.spawn().map_err(|e| format!("打开失败: {e}"))?;
    }
    Ok(())
}

/// 在资源管理器中定位（选中文件）。
/// 主路径走 `SHOpenFolderAndSelectItems`（确定性选中）：`explorer /select,` 命令行
/// 对空格路径行为不稳定——实测同一路径带空格时好时坏、整体引号形式甚至开不出窗口。
/// API 失败时回落 `explorer /select,<path>`（引号包路径）。
pub fn reveal_path(path: &str) -> Result<(), String> {
    let path = win_join_normalize(path);
    ensure_exists(&path)?;
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        if reveal_select_api(&path).is_ok() {
            return Ok(());
        }
        let mut cmd = Command::new("explorer");
        cmd.raw_arg(format!("/select,\"{path}\""))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        hide_window(&mut cmd);
        // explorer 返回码不可靠，spawn 成功即视为成功
        cmd.spawn().map_err(|e| format!("定位失败: {e}"))?;
    }
    #[cfg(not(windows))]
    {
        let dir = Path::new(&path)
            .parent()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.clone());
        let mut cmd = Command::new("xdg-open");
        cmd.arg(&dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        hide_window(&mut cmd);
        cmd.spawn().map_err(|e| format!("定位失败: {e}"))?;
    }
    Ok(())
}

/// 打开父文件夹并选中 `path`（Windows Shell API，不走 explorer 命令行解析）
#[cfg(windows)]
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
            if !ok {
                ILFree(dir_pidl);
                ILFree(file_pidl);
                if need_uninit {
                    CoUninitialize();
                }
                return Err(format!(
                    "定位失败: SHOpenFolderAndSelectItems 0x{:08X}",
                    hr2 as u32
                ));
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_rejects_missing_path() {
        assert!(open_path("Z:\\definitely\\not\\here.mkv").is_err());
        assert!(reveal_path("Z:\\definitely\\not\\here.mkv").is_err());
    }

    #[test]
    fn win_join_normalize_collapses_separators() {
        assert_eq!(win_join_normalize("F:/18138/a.txt"), "F:\\18138\\a.txt");
        // root 以 \ 结尾 + 前端 / 拼接 → 盘符后双分隔符
        assert_eq!(
            win_join_normalize("F:\\/18138/Key小说图册/a.png"),
            "F:\\18138\\Key小说图册\\a.png"
        );
        assert_eq!(win_join_normalize("D:/a//b///c.txt"), "D:\\a\\b\\c.txt");
        // UNC 开头保留双反斜杠
        assert_eq!(
            win_join_normalize("//server/share/a.txt"),
            "\\\\server\\share\\a.txt"
        );
    }

    #[test]
    fn reveal_accepts_forward_slash_paths() {
        let dir = std::env::temp_dir().join(format!("vp_shell_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("临时目录");
        // 真实媒体库文件名普遍含空格/中文——用同类命名走完整链路
        let f = dir.join("reveal me 测试.txt");
        std::fs::write(&f, b"x").expect("写文件");
        let mixed = f.to_string_lossy().replace('\\', "/");
        assert!(
            reveal_path(&mixed).is_ok(),
            "正斜杠/空格/中文路径应定位成功"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
