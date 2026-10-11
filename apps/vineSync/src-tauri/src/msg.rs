//! 后端消息中英双语。语言为全局状态：启动时与保存设置时由配置同步。
//!
//! 简单字面量/带格式化的内容通过 [`msg!`] 宏选择（`format!` 内联捕获变量）；
//! 需要拼接逻辑的消息用本模块的函数。

use crate::model::{Counts, Language};
use std::sync::atomic::{AtomicU8, Ordering};

static LANG: AtomicU8 = AtomicU8::new(0);

pub fn set(lang: Language) {
    LANG.store(
        match lang {
            Language::Zh => 0,
            Language::En => 1,
        },
        Ordering::Relaxed,
    );
}

pub fn current() -> Language {
    if LANG.load(Ordering::Relaxed) == 1 {
        Language::En
    } else {
        Language::Zh
    }
}

pub fn en() -> bool {
    matches!(current(), Language::En)
}

/// 双语选择宏：`msg!("中文 {}", "English {}", arg)`
#[macro_export]
macro_rules! msg {
    ($zh:literal, $en:literal $(, $arg:expr)* $(,)?) => {{
        if $crate::msg::en() {
            ::std::format!($en $(, $arg)*)
        } else {
            ::std::format!($zh $(, $arg)*)
        }
    }};
}

// ---------- 汇总与状态 ----------

/// 变更计数摘要，如 `新增 2 · 更新 1` / `added 2 · updated 1`
pub fn summary(c: &Counts) -> String {
    let mut parts: Vec<String> = Vec::new();
    if c.added > 0 {
        parts.push(if en() {
            format!("added {}", c.added)
        } else {
            format!("新增 {}", c.added)
        });
    }
    if c.updated > 0 {
        parts.push(if en() {
            format!("updated {}", c.updated)
        } else {
            format!("更新 {}", c.updated)
        });
    }
    if c.deleted > 0 {
        parts.push(if en() {
            format!("deleted {}", c.deleted)
        } else {
            format!("删除 {}", c.deleted)
        });
    }
    if c.versioned > 0 {
        parts.push(if en() {
            format!("versioned {}", c.versioned)
        } else {
            format!("版本备份 {}", c.versioned)
        });
    }
    if c.skipped > 0 {
        parts.push(if en() {
            format!("kept deletes {}", c.skipped)
        } else {
            format!("保留删除 {}", c.skipped)
        });
    }
    if c.failed > 0 {
        parts.push(if en() {
            format!("failed {}", c.failed)
        } else {
            format!("失败 {}", c.failed)
        });
    }
    if parts.is_empty() {
        if en() {
            "no changes".to_string()
        } else {
            "无变更".to_string()
        }
    } else {
        parts.join(" · ")
    }
}

pub fn err_pair_target_unavailable(name: &str) -> String {
    if en() {
        format!("{name}: target unavailable")
    } else {
        format!("{name}：目标盘不可用")
    }
}

pub fn err_pair_source_missing(name: &str) -> String {
    if en() {
        format!("{name}: source directory not found")
    } else {
        format!("{name}：源目录不存在")
    }
}

pub fn err_pair_dest_root(name: &str) -> String {
    if en() {
        format!(
            "{name}: destination cannot be a drive root; edit the pair and choose a subfolder (e.g. F:\\Backup)"
        )
    } else {
        format!("{name}：目标目录不能是盘符根目录，请修改文件夹对，选择具体的备份子文件夹（如 F:\\备份）")
    }
}

pub fn pair_err_sep() -> &'static str {
    if en() {
        ": "
    } else {
        "："
    }
}

pub fn err_list_sep() -> &'static str {
    if en() {
        "; "
    } else {
        "；"
    }
}

pub fn err_list_more() -> &'static str {
    if en() {
        ", etc."
    } else {
        " 等"
    }
}

// ---------- 校验提示 ----------

pub fn unnamed_pair() -> &'static str {
    if en() {
        "pair"
    } else {
        "文件夹对"
    }
}

// ---------- 托盘 ----------

pub fn tray_show() -> &'static str {
    if en() {
        "Show main window"
    } else {
        "显示主界面"
    }
}

pub fn tray_sync() -> &'static str {
    if en() {
        "Sync now"
    } else {
        "立即同步"
    }
}

pub fn tray_quit() -> &'static str {
    if en() {
        "Quit vineSync"
    } else {
        "退出 vineSync"
    }
}

// ---------- 单元测试 ----------

#[cfg(test)]
mod tests {
    use super::*;

    /// 语言是全局状态，串行化相关测试避免互相干扰
    fn lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn language_default_and_switch() {
        let _g = lock();
        set(Language::Zh);
        assert!(!en());
        set(Language::En);
        assert!(en());
        set(Language::Zh);
        assert!(!en());
    }

    #[test]
    fn summary_localized() {
        let _g = lock();
        let c = Counts {
            added: 2,
            updated: 1,
            deleted: 0,
            versioned: 0,
            skipped: 0,
            failed: 0,
        };
        set(Language::Zh);
        assert_eq!(summary(&c), "新增 2 · 更新 1");
        set(Language::En);
        assert_eq!(summary(&c), "added 2 · updated 1");
        let empty = Counts::default();
        assert_eq!(summary(&empty), "no changes");
        set(Language::Zh);
        assert_eq!(summary(&empty), "无变更");
    }

    #[test]
    fn macro_selects_language() {
        let _g = lock();
        set(Language::Zh);
        let s = msg!("复制失败: {}", "Copy failed: {}", "x");
        assert_eq!(s, "复制失败: x");
        set(Language::En);
        let s = msg!("复制失败: {}", "Copy failed: {}", "x");
        assert_eq!(s, "Copy failed: x");
        let plain = msg!("源目录不存在", "Source directory missing");
        assert_eq!(plain, "Source directory missing");
        set(Language::Zh);
    }
}
