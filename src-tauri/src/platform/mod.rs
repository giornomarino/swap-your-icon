//! 平台层：列出应用、读取当前图标、把"换图标"拆成一条条可提权执行的原子操作。

use serde::{Deserialize, Serialize};

#[cfg(target_os = "macos")]
mod mac;
#[cfg(target_os = "macos")]
pub use mac::*;

#[cfg(windows)]
mod win;
#[cfg(windows)]
pub use win::*;

#[cfg(not(any(target_os = "macos", windows)))]
mod other;
#[cfg(not(any(target_os = "macos", windows)))]
pub use other::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppInfo {
    /// 稳定 ID（由 key 哈希得到）
    pub id: String,
    pub name: String,
    /// 展示给用户的路径（Mac: .app 路径；Windows: 目标 exe 或快捷方式）
    pub path: String,
    /// 系统保护、改不了的应用
    pub protected: bool,
    /// 实际被修改的文件（Mac: .app；Windows: 所有指向它的 .lnk / .url）
    pub files: Vec<String>,
}

/// 一条原子操作。普通权限执行失败时，会被打包交给提权后的自身进程执行。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "op")]
pub enum Op {
    /// macOS：给 .app 设置自定义图标；png 为 None 表示移除自定义图标
    MacSetIcon { app: String, png: Option<String> },
    /// Windows：设置快捷方式（.lnk / .url）的图标位置；location 为空表示恢复默认
    WinSetIcon { file: String, location: String, index: i32 },
}

impl Op {
    pub fn target(&self) -> &str {
        match self {
            Op::MacSetIcon { app, .. } => app,
            Op::WinSetIcon { file, .. } => file,
        }
    }
}

#[derive(Debug)]
pub enum OpError {
    NeedsAdmin(String),
    Other(String),
}

impl std::fmt::Display for OpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OpError::NeedsAdmin(s) => write!(f, "{}{}{s}", crate::i18n::tr("需要管理员权限", "Needs admin rights"), crate::i18n::colon()),
            OpError::Other(s) => write!(f, "{s}"),
        }
    }
}

/// 当前进程是否是提权后的子进程
pub static ELEVATED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// FNV-1a，保证跨版本稳定
pub fn stable_id(key: &str) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in key.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}

/// 被 `--elevated <job.json>` 调起时执行（已经是管理员 / root）
pub fn run_elevated_job(job_path: &str) -> i32 {
    let ops: Vec<Op> = match std::fs::read_to_string(job_path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
    {
        Some(o) => o,
        None => return 2,
    };
    ELEVATED.store(true, std::sync::atomic::Ordering::Relaxed);
    platform_init();
    let mut failed = Vec::new();
    for op in &ops {
        if let Err(e) = exec_op(op) {
            failed.push(format!("{}: {e}", op.target()));
        }
    }
    let _ = std::fs::write(format!("{job_path}.out"), failed.join("\n"));
    if failed.is_empty() {
        0
    } else {
        1
    }
}
