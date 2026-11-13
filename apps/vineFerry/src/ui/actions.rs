use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AfterAction {
    None,
    Shutdown,
    Sleep,
    Hibernate,
    Lock,
}

impl AfterAction {
    pub fn label(self) -> &'static str {
        match self {
            AfterAction::None => crate::ui::i18n::t("无", "None"),
            AfterAction::Shutdown => crate::ui::i18n::t("关机", "Shutdown"),
            AfterAction::Sleep => crate::ui::i18n::t("睡眠", "Sleep"),
            AfterAction::Hibernate => crate::ui::i18n::t("休眠", "Hibernate"),
            AfterAction::Lock => crate::ui::i18n::t("锁定", "Lock"),
        }
    }

    pub const ALL: [AfterAction; 5] = [
        AfterAction::None,
        AfterAction::Shutdown,
        AfterAction::Sleep,
        AfterAction::Hibernate,
        AfterAction::Lock,
    ];

    pub fn trigger(self) -> Result<(), String> {
        match self {
            AfterAction::None => Ok(()),
            AfterAction::Shutdown => spawn("shutdown", &["/s", "/t", "30"]),
            AfterAction::Sleep => spawn(
                "rundll32",
                &["powrprof.dll,SetSuspendState", "0,1,0"],
            ),
            AfterAction::Hibernate => spawn("shutdown", &["/h"]),
            AfterAction::Lock => spawn("rundll32", &["user32.dll,LockWorkStation"]),
        }
    }
}

fn spawn(program: &str, args: &[&str]) -> Result<(), String> {
    Command::new(program)
        .args(args)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("{program} 执行失败：{e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn none_is_noop() {
        assert_eq!(AfterAction::None.trigger(), Ok(()));
    }

    #[test]
    fn labels_and_all_cover_variants() {
        assert_eq!(AfterAction::ALL.len(), 5);
        assert_eq!(AfterAction::Shutdown.label(), "关机");
        assert_eq!(AfterAction::Lock.label(), "锁定");
    }

    #[test]
    fn serde_lowercases() {
        let json = serde_json::to_string(&AfterAction::Sleep).unwrap();
        assert_eq!(json, "\"sleep\"");
    }
}
