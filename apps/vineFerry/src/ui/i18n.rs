use std::sync::atomic::{AtomicU8, Ordering};

static LANG: AtomicU8 = AtomicU8::new(0);

pub fn toggle() {
    let cur = LANG.load(Ordering::Relaxed);
    LANG.store(1 - cur, Ordering::Relaxed);
}

pub fn en() -> bool {
    LANG.load(Ordering::Relaxed) == 1
}

pub fn t(zh: &'static str, en_text: &'static str) -> &'static str {
    if en() {
        en_text
    } else {
        zh
    }
}

pub fn lang_label() -> &'static str {
    if en() {
        "EN"
    } else {
        "中"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn t_selects_by_language() {
        let before = en();
        toggle();
        assert_ne!(en(), before);
        let picked = t("转移", "Transfer");
        if en() {
            assert_eq!(picked, "Transfer");
        } else {
            assert_eq!(picked, "转移");
        }
        toggle();
        assert_eq!(en(), before);
    }

    #[test]
    fn labels_nonempty() {
        assert!(!lang_label().is_empty());
    }
}
