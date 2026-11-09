//! Windows：应用在开始菜单、桌面、任务栏上显示的都是快捷方式（.lnk / Steam 等用的 .url）的图标。
//! 这里只改快捷方式的 IconLocation，不碰 exe 本身，所以不会破坏程序签名，随时可还原。

use super::{stable_id, AppInfo, Op, OpError, ELEVATED};
use image::RgbaImage;
use std::collections::BTreeMap;
use std::ffi::c_void;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::Ordering;
use windows::core::{Interface, HSTRING, PCWSTR};
use windows::Win32::Foundation::{CloseHandle, E_ACCESSDENIED, SIZE};
use windows::Win32::Graphics::Gdi::{
    DeleteObject, GetDC, GetDIBits, GetObjectW, ReleaseDC, BITMAP, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS,
    HBITMAP,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, IPersistFile, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
    STGM_READ, STGM_READWRITE,
};
use windows::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject, INFINITE};
use windows::Win32::UI::Shell::{
    FOLDERID_CommonPrograms, FOLDERID_Desktop, FOLDERID_Programs, FOLDERID_PublicDesktop, IShellItemImageFactory,
    IShellLinkW, SHChangeNotify, SHCreateItemFromParsingName, SHGetKnownFolderPath, ShellExecuteExW, ShellLink,
    KF_FLAG_DEFAULT, SEE_MASK_NOCLOSEPROCESS, SHCNE_ASSOCCHANGED, SHCNE_UPDATEITEM, SHCNF_IDLIST, SHCNF_PATHW,
    SHELLEXECUTEINFOW, SIIGBF_BIGGERSIZEOK, SIIGBF_ICONONLY,
};
use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub fn platform_init() {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }
}

fn known_folder(id: &windows::core::GUID) -> Option<PathBuf> {
    unsafe {
        let p = SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const c_void));
        s.map(PathBuf::from)
    }
}

/// (目录, 是否开始菜单, 递归深度)
fn shortcut_roots() -> Vec<(PathBuf, bool, u32)> {
    let mut v = Vec::new();
    if let Some(p) = known_folder(&FOLDERID_Programs) {
        v.push((p, true, 4));
    }
    if let Some(p) = known_folder(&FOLDERID_CommonPrograms) {
        v.push((p, true, 4));
    }
    if let Some(p) = known_folder(&FOLDERID_Desktop) {
        v.push((p, false, 0));
    }
    if let Some(p) = known_folder(&FOLDERID_PublicDesktop) {
        v.push((p, false, 0));
    }
    if let Some(appdata) = std::env::var_os("APPDATA") {
        v.push((
            PathBuf::from(appdata).join(r"Microsoft\Internet Explorer\Quick Launch\User Pinned\TaskBar"),
            false,
            0,
        ));
    }
    v
}

fn collect(dir: &Path, depth: u32, start_menu: bool, out: &mut Vec<(PathBuf, bool)>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            if depth > 0 {
                collect(&p, depth - 1, start_menu, out);
            }
            continue;
        }
        match ext(&p).as_str() {
            "lnk" | "url" => out.push((p, start_menu)),
            _ => {}
        }
    }
}

fn ext(p: &Path) -> String {
    p.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase()
}

fn wide_buf_to_string(buf: &[u16]) -> String {
    let n = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..n])
}

fn load_link(path: &Path, mode: windows::Win32::System::Com::STGM) -> windows::core::Result<(IShellLinkW, IPersistFile)> {
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
        let pf: IPersistFile = link.cast()?;
        pf.Load(&HSTRING::from(path.as_os_str()), mode)?;
        Ok((link, pf))
    }
}

fn lnk_target(path: &Path) -> Option<String> {
    let (link, _) = load_link(path, STGM_READ).ok()?;
    let mut buf = [0u16; 1024];
    unsafe { link.GetPath(&mut buf, std::ptr::null_mut(), 0).ok()? };
    let s = wide_buf_to_string(&buf);
    (!s.is_empty()).then_some(s)
}

fn lnk_icon(path: &Path) -> (String, i32) {
    let Ok((link, _)) = load_link(path, STGM_READ) else { return (String::new(), 0) };
    let mut buf = [0u16; 1024];
    let mut idx = 0i32;
    unsafe {
        if link.GetIconLocation(&mut buf, &mut idx).is_err() {
            return (String::new(), 0);
        }
    }
    (wide_buf_to_string(&buf), idx)
}

fn url_fields(path: &Path) -> (Option<String>, String, i32) {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let (mut url, mut icon, mut idx) = (None, String::new(), 0);
    for line in text.lines() {
        if let Some((k, v)) = line.split_once('=') {
            match k.trim().to_ascii_lowercase().as_str() {
                "url" => url = Some(v.trim().to_string()),
                "iconfile" => icon = v.trim().to_string(),
                "iconindex" => idx = v.trim().parse().unwrap_or(0),
                _ => {}
            }
        }
    }
    (url, icon, idx)
}

fn read_icon(file: &Path) -> (String, i32) {
    if ext(file) == "url" {
        let (_, icon, idx) = url_fields(file);
        (icon, idx)
    } else {
        lnk_icon(file)
    }
}

fn is_noise(name: &str) -> bool {
    let n = name.to_lowercase();
    ["uninstall", "卸载", "uninst", "readme", "release notes", "license"].iter().any(|k| n.contains(k))
}

pub fn list_apps() -> Vec<AppInfo> {
    platform_init();
    let mut files = Vec::new();
    for (dir, sm, depth) in shortcut_roots() {
        collect(&dir, depth, sm, &mut files);
    }
    // key -> (name, name_from_start_menu, display_path, files)
    let mut groups: BTreeMap<String, (String, bool, String, Vec<String>)> = BTreeMap::new();
    for (file, start_menu) in files {
        let stem = file.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
        if stem.is_empty() || is_noise(&stem) {
            continue;
        }
        let (key, display) = if ext(&file) == "url" {
            match url_fields(&file).0 {
                Some(u) => (format!("url:{}", u.to_lowercase()), u),
                None => continue,
            }
        } else {
            match lnk_target(&file) {
                Some(t) => {
                    // 只要指向程序的快捷方式，跳过指向文档、文件夹的
                    if !t.to_ascii_lowercase().ends_with(".exe") {
                        continue;
                    }
                    (format!("exe:{}", t.to_lowercase()), t)
                }
                // MSI 广告快捷方式、商店应用等拿不到目标路径，按名称归组
                None => (format!("lnk:{}", stem.to_lowercase()), file.to_string_lossy().to_string()),
            }
        };
        let entry = groups.entry(key).or_insert_with(|| (stem.clone(), start_menu, display, Vec::new()));
        if start_menu && !entry.1 {
            entry.0 = stem.clone();
            entry.1 = true;
        }
        entry.3.push(file.to_string_lossy().to_string());
    }
    let mut out: Vec<AppInfo> = groups
        .into_iter()
        .map(|(key, (name, _, path, files))| AppInfo { id: stable_id(&key), name, path, protected: false, files })
        .collect();
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out
}

pub fn current_icon_png(app: &AppInfo, size: u32) -> Option<Vec<u8>> {
    platform_init();
    let file = app.files.first()?;
    unsafe {
        let factory: IShellItemImageFactory = SHCreateItemFromParsingName(&HSTRING::from(file.as_str()), None).ok()?;
        let hbm = factory
            .GetImage(SIZE { cx: size as i32, cy: size as i32 }, SIIGBF_ICONONLY | SIIGBF_BIGGERSIZEOK)
            .ok()?;
        let img = hbitmap_to_rgba(hbm);
        let _ = DeleteObject(hbm.into());
        img.map(|i| crate::imaging::to_png(&i))
    }
}

unsafe fn hbitmap_to_rgba(hbm: HBITMAP) -> Option<RgbaImage> {
    let mut bm = BITMAP::default();
    if GetObjectW(hbm.into(), std::mem::size_of::<BITMAP>() as i32, Some(&mut bm as *mut _ as *mut c_void)) == 0 {
        return None;
    }
    let (w, h) = (bm.bmWidth, bm.bmHeight.abs());
    if w <= 0 || h <= 0 {
        return None;
    }
    let mut bi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            biHeight: -h, // 自上而下
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut buf = vec![0u8; (w * h * 4) as usize];
    let hdc = GetDC(None);
    let lines = GetDIBits(hdc, hbm, 0, h as u32, Some(buf.as_mut_ptr() as *mut c_void), &mut bi, DIB_RGB_COLORS);
    ReleaseDC(None, hdc);
    if lines == 0 {
        return None;
    }
    let no_alpha = buf.chunks_exact(4).all(|p| p[3] == 0);
    for p in buf.chunks_exact_mut(4) {
        let (b, g, r, a) = (p[0], p[1], p[2], if no_alpha { 255 } else { p[3] });
        // 预乘 alpha 还原
        let un = |c: u8| if a == 0 || a == 255 { c } else { ((c as u32 * 255 + a as u32 / 2) / a as u32).min(255) as u8 };
        p[0] = un(r);
        p[1] = un(g);
        p[2] = un(b);
        p[3] = a;
    }
    RgbaImage::from_raw(w as u32, h as u32, buf)
}

pub fn write_icon_file(id: &str, img: &RgbaImage, dir: &Path) -> Result<String, String> {
    let d = dir.join("icons");
    std::fs::create_dir_all(&d).map_err(|e| e.to_string())?;
    // 文件名带时间戳：Windows 按路径缓存图标，换新路径才能立刻刷新
    let name = format!("{id}-{}.ico", crate::now_ts());
    let f = d.join(&name);
    std::fs::write(&f, crate::imaging::to_ico(img)?).map_err(|e| e.to_string())?;
    if let Ok(rd) = std::fs::read_dir(&d) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            if n.starts_with(&format!("{id}-")) && n != name {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    Ok(f.to_string_lossy().to_string())
}

pub fn apply_ops(app: &AppInfo, icon_file: &str) -> Vec<Op> {
    app.files
        .iter()
        .map(|f| Op::WinSetIcon { file: f.clone(), location: icon_file.to_string(), index: 0 })
        .collect()
}

pub fn capture_restore_ops(_app: &AppInfo, _id: &str, _dir: &Path, files: &[String]) -> Vec<Op> {
    files
        .iter()
        .map(|f| {
            let (location, index) = read_icon(Path::new(f));
            Op::WinSetIcon { file: f.clone(), location, index }
        })
        .collect()
}

/// 应用更新常会重建快捷方式；任何一个快捷方式不再指向我们的图标就算"被还原"
pub fn is_applied(app: &AppInfo, icon_file: &str) -> bool {
    app.files.iter().all(|f| read_icon(Path::new(f)).0.eq_ignore_ascii_case(icon_file))
}

fn map_io(e: std::io::Error, file: &str) -> OpError {
    if e.kind() == std::io::ErrorKind::PermissionDenied && !ELEVATED.load(Ordering::Relaxed) {
        OpError::NeedsAdmin(file.to_string())
    } else {
        OpError::Other(format!("{file}{}{e}", crate::i18n::colon()))
    }
}

fn set_url_icon(file: &str, location: &str, index: i32) -> Result<(), OpError> {
    let text = std::fs::read_to_string(file).map_err(|e| map_io(e, file))?;
    let mut lines: Vec<String> = Vec::new();
    let mut in_section = false;
    let mut inserted = false;
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            if in_section && !inserted && !location.is_empty() {
                lines.push(format!("IconFile={location}"));
                lines.push(format!("IconIndex={index}"));
                inserted = true;
            }
            in_section = t.eq_ignore_ascii_case("[InternetShortcut]");
        }
        let key = t.split_once('=').map(|(k, _)| k.trim().to_ascii_lowercase());
        if in_section && matches!(key.as_deref(), Some("iconfile") | Some("iconindex")) {
            continue;
        }
        lines.push(line.to_string());
    }
    if in_section && !inserted && !location.is_empty() {
        lines.push(format!("IconFile={location}"));
        lines.push(format!("IconIndex={index}"));
    }
    std::fs::write(file, lines.join("\r\n") + "\r\n").map_err(|e| map_io(e, file))
}

fn set_lnk_icon(file: &str, location: &str, index: i32) -> Result<(), OpError> {
    let map = |e: windows::core::Error| {
        if e.code() == E_ACCESSDENIED && !ELEVATED.load(Ordering::Relaxed) {
            OpError::NeedsAdmin(file.to_string())
        } else {
            OpError::Other(format!("{file}{}{}", crate::i18n::colon(), e.message()))
        }
    };
    let (link, pf) = load_link(Path::new(file), STGM_READWRITE).map_err(map)?;
    unsafe {
        link.SetIconLocation(&HSTRING::from(location), index).map_err(map)?;
        pf.Save(&HSTRING::from(file), true).map_err(map)?;
    }
    Ok(())
}

pub fn exec_op(op: &Op) -> Result<(), OpError> {
    let Op::WinSetIcon { file, location, index } = op else {
        return Err(OpError::Other("unsupported operation".into()));
    };
    platform_init();
    if ext(Path::new(file)) == "url" {
        set_url_icon(file, location, *index)?;
    } else {
        set_lnk_icon(file, location, *index)?;
    }
    let w = HSTRING::from(file.as_str());
    unsafe { SHChangeNotify(SHCNE_UPDATEITEM, SHCNF_PATHW, Some(w.as_ptr() as *const c_void), None) };
    Ok(())
}

/// 用 UAC 提权（runas）启动自身执行这些操作
pub fn run_elevated(ops: &[Op], dir: &Path) -> Result<(), String> {
    let _ = std::fs::create_dir_all(dir);
    let job = dir.join(format!("job-{}.json", crate::now_ts()));
    std::fs::write(&job, serde_json::to_string(ops).unwrap()).map_err(|e| e.to_string())?;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let exe_w = HSTRING::from(exe.as_os_str());
    let params = HSTRING::from(format!("--elevated \"{}\"", job.display()));
    let verb = HSTRING::from("runas");
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS,
        lpVerb: PCWSTR(verb.as_ptr()),
        lpFile: PCWSTR(exe_w.as_ptr()),
        lpParameters: PCWSTR(params.as_ptr()),
        nShow: SW_HIDE.0,
        ..Default::default()
    };
    let result = unsafe { ShellExecuteExW(&mut info) };
    let out_file = format!("{}.out", job.display());
    if result.is_err() {
        let _ = std::fs::remove_file(&job);
        return Err(crate::i18n::tr("已取消管理员授权", "Administrator authorization was cancelled").into());
    }
    let mut code = 1u32;
    unsafe {
        WaitForSingleObject(info.hProcess, INFINITE);
        let _ = GetExitCodeProcess(info.hProcess, &mut code);
        let _ = CloseHandle(info.hProcess);
    }
    let report = std::fs::read_to_string(&out_file).unwrap_or_default();
    let _ = std::fs::remove_file(&job);
    let _ = std::fs::remove_file(&out_file);
    if code == 0 {
        Ok(())
    } else {
        Err(if report.is_empty() { format!("{} {code}", crate::i18n::tr("提权进程退出码", "Elevated process exit code")) } else { report })
    }
}

/// 通知资源管理器刷新图标缓存；hard 时重启资源管理器（任务栏固定图标有时只有这样才会更新）
pub fn refresh_shell(hard: bool) {
    unsafe { SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None) };
    let _ = Command::new("ie4uinit.exe").arg("-show").creation_flags(CREATE_NO_WINDOW).status();
    if hard {
        let _ = Command::new("taskkill").args(["/f", "/im", "explorer.exe"]).creation_flags(CREATE_NO_WINDOW).status();
        let _ = Command::new("explorer.exe").spawn();
    }
}
