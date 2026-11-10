//! 界面语言：跟随系统（默认）/ English / 中文，可在设置里切换。
//! 跟随系统时，系统首选语言是中文就用中文，其他一律英文。

use std::sync::atomic::{AtomicU8, Ordering};

const AUTO: u8 = 0;
const EN: u8 = 1;
const ZH: u8 = 2;

static CHOICE: AtomicU8 = AtomicU8::new(AUTO);

fn system_is_zh() -> bool {
    sys_locale::get_locale()
        .map(|l| l.to_ascii_lowercase().starts_with("zh"))
        .unwrap_or(false)
}

/// 设置语言偏好："system" / "en" / "zh"
pub fn set_choice(choice: &str) {
    let v = match choice {
        "en" => EN,
        "zh" => ZH,
        _ => AUTO,
    };
    CHOICE.store(v, Ordering::Relaxed);
}

pub fn is_zh() -> bool {
    match CHOICE.load(Ordering::Relaxed) {
        EN => false,
        ZH => true,
        _ => system_is_zh(),
    }
}

/// 实际生效的语言
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
