//! 一键更新：从 GitHub Releases 取最新版本，下载安装包、校验 SHA-256，然后
//! macOS 用新的 .app 替换当前 .app 并重启；Windows 静默运行安装程序后重启。
//! 只用系统自带的 curl 下载（macOS 自带，Windows 10 1803 起自带），不引入额外的网络库。

use crate::i18n::tr;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::path::Path;
use std::process::Command;

pub const REPO: &str = "y99854-ui/swap-your-icon";

#[derive(Debug, Clone, Serialize)]
pub struct UpdateInfo {
    pub current: String,
    pub latest: String,
    pub available: bool,
    pub notes: String,
    pub url: String,
}

pub enum Installed {
    /// 已交给后台脚本替换并重启，应用需要立即退出
    Restarting,
    /// 无法自动替换，已打开安装包让用户手动安装
    #[cfg_attr(windows, allow(dead_code))]
    Manual(String),
}

fn curl() -> Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let mut c = Command::new("curl.exe");
        c.creation_flags(0x0800_0000);
        c
    }
    #[cfg(not(windows))]
    {
        Command::new("/usr/bin/curl")
    }
}

fn get_json(url: &str) -> Result<serde_json::Value, String> {
    let out = curl()
        .args(["-fsSL", "--max-time", "20", "-H", "Accept: application/vnd.github+json", "-H", "User-Agent: SwapYourIcon-updater", url])
        .output()
        .map_err(|e| format!("curl: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "{}{}",
            tr("连接 GitHub 失败：", "Couldn't reach GitHub: "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    serde_json::from_slice(&out.stdout).map_err(|e| e.to_string())
}

/// "6704" / "v1.2.3" / "6703.0.0" → [6704, 0, 0]
fn parse_version(s: &str) -> Vec<u64> {
    let mut v: Vec<u64> = s
        .trim()
        .trim_start_matches(['v', 'V'])
        .split('.')
        .map(|p| p.chars().take_while(|c| c.is_ascii_digit()).collect::<String>().parse().unwrap_or(0))
        .collect();
    v.resize(3.max(v.len()), 0);
    v
}

fn asset_matches(name: &str) -> bool {
    if cfg!(target_os = "macos") {
        name.ends_with(".dmg")
    } else if cfg!(windows) {
        name.ends_with("-setup.exe")
    } else {
        false
    }
}

struct Release {
    info: UpdateInfo,
    download: Option<(String, String, Option<String>)>, // (url, file name, sha256)
}

fn latest(current: &str) -> Result<Release, String> {
    let r = get_json(&format!("https://api.github.com/repos/{REPO}/releases/latest"))?;
    let tag = r["tag_name"].as_str().unwrap_or_default().to_string();
    let download = r["assets"].as_array().and_then(|assets| {
        assets.iter().find_map(|a| {
            let name = a["name"].as_str()?;
            asset_matches(name).then(|| {
                (
                    a["browser_download_url"].as_str().unwrap_or_default().to_string(),
                    name.to_string(),
                    a["digest"].as_str().and_then(|d| d.strip_prefix("sha256:")).map(|d| d.to_lowercase()),
                )
            })
        })
    });
    let available = !tag.is_empty() && parse_version(&tag) > parse_version(current) && download.is_some();
    Ok(Release {
        info: UpdateInfo {
            current: current.to_string(),
            latest: tag,
            available,
            notes: r["body"].as_str().unwrap_or_default().chars().take(600).collect(),
            url: r["html_url"].as_str().unwrap_or_default().to_string(),
        },
        download,
    })
}

pub fn check(current: &str) -> Result<UpdateInfo, String> {
    latest(current).map(|r| r.info)
}

fn sha256_file(p: &Path) -> Result<String, String> {
    let bytes = std::fs::read(p).map_err(|e| e.to_string())?;
    Ok(Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect())
}

fn download(url: &str, dest: &Path) -> Result<(), String> {
    let out = curl()
        .args(["-fL", "--max-time", "600", "-H", "User-Agent: SwapYourIcon-updater", "-o"])
        .arg(dest)
        .arg(url)
        .output()
        .map_err(|e| format!("curl: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!("{}{}", tr("下载失败：", "Download failed: "), String::from_utf8_lossy(&out.stderr).trim()))
    }
}

pub fn install(current: &str) -> Result<Installed, String> {
    let rel = latest(current)?;
    if !rel.info.available {
        return Err(tr("已经是最新版本", "You're already on the latest version").into());
    }
    let (url, name, sha) = rel.download.ok_or_else(|| tr("这个版本没有适合本机的安装包", "No installer for this platform in the release").to_string())?;
    let dir = std::env::temp_dir().join(format!("swapyouricon-update-{}", crate::now_ts()));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let file = dir.join(&name);
    download(&url, &file)?;
    if let Some(expected) = sha {
        let got = sha256_file(&file)?;
        if got != expected {
            let _ = std::fs::remove_file(&file);
            return Err(tr("安装包校验失败（SHA-256 不一致），已放弃更新", "Installer checksum mismatch (SHA-256) — update cancelled").into());
        }
    }
    install_platform(&file, &dir)
}

#[cfg(target_os = "macos")]
fn install_platform(dmg: &Path, dir: &Path) -> Result<Installed, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    // .../Swap Your Icon.app/Contents/MacOS/<exe>
    let bundle: std::path::PathBuf = exe
        .ancestors()
        .nth(3)
        .filter(|p| p.extension().map(|e| e == "app").unwrap_or(false))
        .map(Path::to_path_buf)
        .ok_or_else(|| tr("开发模式下不能自动更新", "Updates aren't available when running in development mode").to_string())?;

    let mnt = dir.join("mnt");
    let st = Command::new("/usr/bin/hdiutil")
        .args(["attach", "-nobrowse", "-noautoopen", "-readonly", "-mountpoint"])
        .arg(&mnt)
        .arg(dmg)
        .output()
        .map_err(|e| e.to_string())?;
    if !st.status.success() {
        return Err(format!("hdiutil: {}", String::from_utf8_lossy(&st.stderr).trim()));
    }
    let new_app = std::fs::read_dir(&mnt)
        .ok()
        .and_then(|rd| rd.flatten().map(|e| e.path()).find(|p| p.extension().map(|e| e == "app").unwrap_or(false)));

    let parent_writable = bundle
        .parent()
        .map(|p| {
            let probe = p.join(".swapyouricon-update-probe");
            let ok = std::fs::write(&probe, b"").is_ok();
            let _ = std::fs::remove_file(&probe);
            ok
        })
        .unwrap_or(false);

    let Some(new_app) = new_app.filter(|_| parent_writable) else {
        // 自动替换不了（比如 /Applications 没有写权限）：打开安装包让用户拖进去
        let _ = Command::new("/usr/bin/hdiutil").args(["detach", "-quiet"]).arg(&mnt).status();
        let _ = Command::new("/usr/bin/open").arg(dmg).status();
        return Ok(Installed::Manual(
            tr(
                "无法自动替换，已打开新版安装包：把 Swap Your Icon 拖进「应用程序」覆盖旧版即可",
                "Couldn't replace the app automatically. The new installer is open — drag Swap Your Icon into Applications to replace the old version",
            )
            .into(),
        ));
    };

    // 等当前进程退出后再替换，失败就回滚，最后打开新版本
    let q = |p: &Path| format!("'{}'", p.to_string_lossy().replace('\'', "'\\''"));
    let old = bundle.with_extension("app.old");
    let script = format!(
        r#"#!/bin/sh
while kill -0 {pid} 2>/dev/null; do sleep 0.3; done
rm -rf {old}
if mv {target} {old} && /usr/bin/ditto {src} {target}; then
  rm -rf {old}
else
  rm -rf {target}; mv {old} {target}
fi
/usr/bin/xattr -cr {target} 2>/dev/null
/usr/bin/hdiutil detach -quiet {mnt}
/usr/bin/open {target}
"#,
        pid = std::process::id(),
        old = q(&old),
        target = q(&bundle),
        src = q(&new_app),
        mnt = q(&mnt),
    );
    let sh = dir.join("update.sh");
    std::fs::write(&sh, script).map_err(|e| e.to_string())?;
    Command::new("/bin/sh").arg(&sh).spawn().map_err(|e| e.to_string())?;
    Ok(Installed::Restarting)
}

#[cfg(windows)]
fn install_platform(setup: &Path, dir: &Path) -> Result<Installed, String> {
    use std::os::windows::process::CommandExt;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    // 等当前进程退出 → 静默安装 → 重新打开
    let script = format!(
        "@echo off\r\ntimeout /t 2 /nobreak >nul\r\n\"{setup}\" /S\r\nstart \"\" \"{exe}\"\r\n",
        setup = setup.display(),
        exe = exe.display()
    );
    let bat = dir.join("update.cmd");
    std::fs::write(&bat, script).map_err(|e| e.to_string())?;
    Command::new("cmd.exe")
        .arg("/C")
        .arg(&bat)
        .creation_flags(0x0800_0000 | 0x0000_0008) // CREATE_NO_WINDOW | DETACHED_PROCESS
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(Installed::Restarting)
}

#[cfg(not(any(target_os = "macos", windows)))]
fn install_platform(_file: &Path, _dir: &Path) -> Result<Installed, String> {
    Err("unsupported platform".into())
}

#[cfg(test)]
mod tests {
    use super::parse_version;
    #[test]
    fn versions() {
        assert!(parse_version("6704") > parse_version("6703.0.0"));
        assert!(parse_version("v1.2.10") > parse_version("1.2.9"));
        assert_eq!(parse_version("6703"), parse_version("6703.0.0"));
    }
}
