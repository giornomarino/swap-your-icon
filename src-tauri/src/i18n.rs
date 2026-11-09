//! 跟随系统语言：系统首选语言是中文就用中文，其他一律英文。

use std::sync::OnceLock;

pub fn is_zh() -> bool {
    static ZH: OnceLock<bool> = OnceLock::new();
    *ZH.get_or_init(|| {
        sys_locale::get_locale()
            .map(|l| l.to_ascii_lowercase().starts_with("zh"))
            .unwrap_or(false)
    })
}

pub fn lang() -> &'static str {
    if is_zh() {
        "zh"
    } else {
        "en"
    }
}

/// 二选一：中文 / 英文
pub fn tr(zh: &'static str, en: &'static str) -> &'static str {
    if is_zh() {
        zh
    } else {
        en
    }
}

/// 冒号分隔符
pub fn colon() -> &'static str {
    tr("：", ": ")
}
