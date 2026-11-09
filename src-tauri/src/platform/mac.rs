//! macOS：用 NSWorkspace setIcon:forFile:options: 给 .app 设置自定义图标
//! （和 Finder「显示简介」里粘贴图标是同一个机制：在 bundle 里写入 `Icon\r` 并打上自定义图标标记）。

use super::{stable_id, AppInfo, Op, OpError, ELEVATED};
use crate::i18n::tr;
use image::RgbaImage;
use objc2::rc::Retained;
use objc2::AnyThread;
use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep, NSImage, NSWorkspace, NSWorkspaceIconCreationOptions};
use objc2_foundation::{NSDictionary, NSFileManager, NSPoint, NSRect, NSSize, NSString};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::Ordering;

fn ns(s: &str) -> Retained<NSString> {
    NSString::from_str(s)
}

pub fn platform_init() {}

pub fn list_apps() -> Vec<AppInfo> {
    let mut roots: Vec<(PathBuf, bool)> = vec![
        (PathBuf::from("/Applications"), false),
        (PathBuf::from("/System/Applications"), true),
    ];
    if let Some(home) = std::env::var_os("HOME") {
        roots.push((PathBuf::from(home).join("Applications"), false));
    }
    let mut found = Vec::new();
    for (dir, protected) in roots {
        scan(&dir, 2, protected, &mut found);
    }

    let fm = NSFileManager::defaultManager();
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for (path, protected) in found {
        let canon = std::fs::canonicalize(&path).unwrap_or(path.clone());
        if !seen.insert(canon.clone()) {
            continue;
        }
        let p = path.to_string_lossy().to_string();
        let mut name = fm.displayNameAtPath(&ns(&p)).to_string();
        if let Some(stripped) = name.strip_suffix(".app") {
            name = stripped.to_string();
        }
        // /Applications/Safari.app 等实际位于只读系统卷
        let protected = protected
            || canon.starts_with("/System")
            || matches!(path.file_name().and_then(|n| n.to_str()), Some("Safari.app"));
        out.push(AppInfo { id: stable_id(&p), name, path: p.clone(), protected, files: vec![p] });
    }
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out
}

fn scan(dir: &Path, depth: u32, protected: bool, out: &mut Vec<(PathBuf, bool)>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        if name.ends_with(".app") {
            out.push((p, protected));
        } else if depth > 0 && p.is_dir() {
            scan(&p, depth - 1, protected, out);
        }
    }
}

pub fn current_icon_png(app: &AppInfo, size: u32) -> Option<Vec<u8>> {
    icon_png_for_path(&app.files[0], size)
}

fn icon_png_for_path(path: &str, size: u32) -> Option<Vec<u8>> {
    let ws = NSWorkspace::sharedWorkspace();
    let img = ws.iconForFile(&ns(path));
    let s = size as f64;
    img.setSize(NSSize::new(s, s));
    let mut rect = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(s, s));
    let cg = unsafe { img.CGImageForProposedRect_context_hints(&mut rect, None, None) }?;
    let rep = NSBitmapImageRep::initWithCGImage(NSBitmapImageRep::alloc(), &cg);
    let data = unsafe { rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new()) }?;
    Some(data.to_vec())
}

fn has_custom_icon(app: &str) -> bool {
    Path::new(app).join("Icon\r").exists()
}

pub fn write_icon_file(id: &str, img: &RgbaImage, dir: &Path) -> Result<String, String> {
    let d = dir.join("icons");
    std::fs::create_dir_all(&d).map_err(|e| e.to_string())?;
    let f = d.join(format!("{id}.png"));
    std::fs::write(&f, crate::imaging::to_png(img)).map_err(|e| e.to_string())?;
    Ok(f.to_string_lossy().to_string())
}

pub fn apply_ops(app: &AppInfo, icon_file: &str) -> Vec<Op> {
    app.files
        .iter()
        .map(|f| Op::MacSetIcon { app: f.clone(), png: Some(icon_file.to_string()) })
        .collect()
}

/// 记录修改前的状态：原本就有自定义图标的，先把它备份成 PNG
pub fn capture_restore_ops(_app: &AppInfo, id: &str, dir: &Path, files: &[String]) -> Vec<Op> {
    files
        .iter()
        .map(|f| {
            let mut png = None;
            if has_custom_icon(f) {
                if let Some(bytes) = icon_png_for_path(f, 1024) {
                    let d = dir.join("backup");
                    let _ = std::fs::create_dir_all(&d);
                    let p = d.join(format!("{id}.png"));
                    if std::fs::write(&p, bytes).is_ok() {
                        png = Some(p.to_string_lossy().to_string());
                    }
                }
            }
            Op::MacSetIcon { app: f.clone(), png }
        })
        .collect()
}

/// 应用更新会整个替换 .app，自定义图标随之消失
pub fn is_applied(app: &AppInfo, _icon_file: &str) -> bool {
    app.files.iter().all(|f| has_custom_icon(f))
}

fn writable(dir: &str) -> std::io::Result<()> {
    let probe = Path::new(dir).join(".iconswap-probe");
    std::fs::write(&probe, b"")?;
    std::fs::remove_file(&probe)
}

pub fn exec_op(op: &Op) -> Result<(), OpError> {
    let Op::MacSetIcon { app, png } = op else {
        return Err(OpError::Other("unsupported operation".into()));
    };
    if let Err(e) = writable(app) {
        let elevated = ELEVATED.load(Ordering::Relaxed);
        return match e.raw_os_error() {
            // EACCES：文件归 root 所有，提权就能改
            Some(13) if !elevated => Err(OpError::NeedsAdmin(app.clone())),
            // EPERM：被 macOS「App 管理」隐私权限拦下，提权也没用，得去系统设置里打开
            Some(1) => {
                if !elevated {
                    open_app_management_settings();
                }
                Err(OpError::Other(
                    tr(
                        "macOS 拦截了对其他应用的修改。已打开「系统设置 → 隐私与安全性 → App 管理」：\
                         打开 Swap Your Icon 的开关后，从菜单栏图标选「退出」彻底退出，再重新打开（只关窗口不够）。\
                         如果列表里已经开着，先关掉再打开一次。用 cargo tauri dev 运行时，要打开的是你运行命令的终端",
                        "macOS blocked changes to other apps. System Settings → Privacy & Security → App Management \
                         has been opened: turn on Swap Your Icon, then fully quit it from the menu bar icon (closing the \
                         window isn't enough) and reopen it. If it's already on, toggle it off and on again. When running \
                         via cargo tauri dev, turn on the terminal you ran it from instead",
                    )
                    .into(),
                ))
            }
            _ => Err(OpError::Other(if crate::i18n::is_zh() {
                format!("无法写入（{e}）。若是系统应用，受 SIP 保护无法修改")
            } else {
                format!("Can't write ({e}). Built-in system apps are protected by SIP and can't be changed")
            })),
        };
    }
    let ws = NSWorkspace::sharedWorkspace();
    let ok = match png {
        Some(p) => {
            let img = NSImage::initWithContentsOfFile(NSImage::alloc(), &ns(p))
                .ok_or_else(|| OpError::Other(format!("{}{}{p}", tr("读取图标文件失败", "Failed to read icon file"), crate::i18n::colon())))?;
            ws.setIcon_forFile_options(Some(&img), &ns(app), NSWorkspaceIconCreationOptions(0))
        }
        None => ws.setIcon_forFile_options(None, &ns(app), NSWorkspaceIconCreationOptions(0)),
    };
    if !ok {
        return Err(OpError::Other(
            tr(
                "系统拒绝修改。请在「系统设置 → 隐私与安全性 → App 管理」里允许 Swap Your Icon，然后从菜单栏图标选「退出」再重新打开",
                "macOS refused the change. Allow Swap Your Icon in System Settings → Privacy & Security → App Management, then quit it from the menu bar icon and reopen it",
            )
            .into(),
        ));
    }
    // 更新修改时间，让 Finder / Dock 重新读取
    let _ = Command::new("/usr/bin/touch").arg(app).status();
    Ok(())
}

/// 打开「隐私与安全性 → App 管理」设置页（每次运行只打开一次）
fn open_app_management_settings() {
    static OPENED: std::sync::Once = std::sync::Once::new();
    OPENED.call_once(|| {
        let _ = Command::new("/usr/bin/open")
            .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_AppBundles")
            .spawn();
    });
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn applescript_string(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// 通过 osascript 弹出系统管理员密码框，以 root 身份运行自身执行这些操作
pub fn run_elevated(ops: &[Op], dir: &Path) -> Result<(), String> {
    let _ = std::fs::create_dir_all(dir);
    let job = dir.join(format!("job-{}.json", crate::now_ts()));
    std::fs::write(&job, serde_json::to_string(ops).unwrap()).map_err(|e| e.to_string())?;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let cmd = format!(
        "{} --elevated {}",
        shell_quote(&exe.to_string_lossy()),
        shell_quote(&job.to_string_lossy())
    );
    let script = format!(
        "do shell script \"{}\" with administrator privileges with prompt \"{}\"",
        applescript_string(&cmd),
        tr(
            "Swap Your Icon 需要管理员权限来修改这些应用的图标",
            "Swap Your Icon needs administrator rights to change these apps' icons"
        )
    );
    let out = Command::new("/usr/bin/osascript").arg("-e").arg(&script).output().map_err(|e| e.to_string())?;
    let report = std::fs::read_to_string(format!("{}.out", job.display())).unwrap_or_default();
    let _ = std::fs::remove_file(&job);
    let _ = std::fs::remove_file(format!("{}.out", job.display()));
    if out.status.success() {
        Ok(())
    } else if String::from_utf8_lossy(&out.stderr).contains("-128") {
        Err(tr("已取消管理员授权", "Administrator authorization was cancelled").into())
    } else if !report.is_empty() {
        Err(report)
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// Dock 会缓存图标，需要重启它才会显示新图标；hard 时连 Finder 一起重启
pub fn refresh_shell(hard: bool) {
    let _ = Command::new("/usr/bin/killall").arg("Dock").status();
    if hard {
        let _ = Command::new("/usr/bin/killall").arg("Finder").status();
    }
}
