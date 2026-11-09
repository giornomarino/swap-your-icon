pub mod i18n;
mod imaging;
pub mod platform;

use imaging::Shape;
use platform::{AppInfo, Op, OpError};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, State, WindowEvent};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};

pub fn now_ts() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ---------------------------------------------------------------- 持久化状态

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Record {
    id: String,
    name: String,
    path: String,
    /// 实际设置给系统的图标文件（Mac: png；Windows: ico）
    icon_file: String,
    /// 处理后的 1024 PNG，用于重套和保存主题
    image: String,
    /// 还原用的操作（修改前的状态）
    restore: Vec<Op>,
    applied_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
struct Settings {
    auto_reapply: bool,
    interval_min: u32,
    refresh_shell: bool,
    keep_in_tray: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { auto_reapply: true, interval_min: 10, refresh_shell: true, keep_in_tray: true }
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct Store {
    records: BTreeMap<String, Record>,
    settings: Settings,
}

struct Core {
    dir: PathBuf,
    store: Mutex<Store>,
    apps: Mutex<Vec<AppInfo>>,
}

type CoreState<'a> = State<'a, Arc<Core>>;

impl Core {
    fn load(dir: PathBuf) -> Self {
        let _ = std::fs::create_dir_all(&dir);
        let store = std::fs::read_to_string(dir.join("state.json"))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        Core { dir, store: Mutex::new(store), apps: Mutex::new(Vec::new()) }
    }

    fn save(&self) {
        let s = self.store.lock().unwrap();
        if let Ok(json) = serde_json::to_string_pretty(&*s) {
            let tmp = self.dir.join("state.json.tmp");
            if std::fs::write(&tmp, json).is_ok() {
                let _ = std::fs::rename(&tmp, self.dir.join("state.json"));
            }
        }
    }

    fn refresh_apps(&self) -> Vec<AppInfo> {
        let apps = platform::list_apps();
        *self.apps.lock().unwrap() = apps.clone();
        apps
    }

    fn apps_by_ids(&self, ids: &[String]) -> Vec<AppInfo> {
        let mut apps = self.apps.lock().unwrap().clone();
        if apps.is_empty() {
            apps = self.refresh_apps();
        }
        let want: HashSet<&String> = ids.iter().collect();
        apps.into_iter().filter(|a| want.contains(&a.id)).collect()
    }

    fn settings(&self) -> Settings {
        self.store.lock().unwrap().settings.clone()
    }
}

// ---------------------------------------------------------------- 执行

#[derive(Debug, Default, Serialize)]
struct Report {
    ok: Vec<String>,
    failed: Vec<Failure>,
}

#[derive(Debug, Serialize)]
struct Failure {
    name: String,
    msg: String,
}

impl Report {
    fn fail(&mut self, name: &str, msg: impl Into<String>) {
        self.failed.push(Failure { name: name.to_string(), msg: msg.into() });
    }
}

/// 执行多组操作；普通权限不够的操作会一起提权执行，只弹一次授权框。
fn execute(groups: &[Vec<Op>], dir: &Path, allow_elevate: bool) -> Vec<Result<(), String>> {
    platform::platform_init();
    let mut results: Vec<Result<(), String>> = vec![Ok(()); groups.len()];
    let mut pending: Vec<(usize, Op)> = Vec::new();
    for (i, ops) in groups.iter().enumerate() {
        for op in ops {
            match platform::exec_op(op) {
                Ok(()) => {}
                Err(OpError::NeedsAdmin(_)) => pending.push((i, op.clone())),
                Err(OpError::Other(e)) => {
                    if results[i].is_ok() {
                        results[i] = Err(e);
                    }
                }
            }
        }
    }
    if !pending.is_empty() {
        let outcome = if allow_elevate {
            let ops: Vec<Op> = pending.iter().map(|(_, o)| o.clone()).collect();
            platform::run_elevated(&ops, &dir.join("jobs"))
        } else {
            Err(i18n::tr(
                "需要管理员权限，请打开 Swap Your Icon 点「检查并重套」",
                "Needs admin rights — open Swap Your Icon and click “Check & Reapply”",
            )
            .to_string())
        };
        if let Err(e) = outcome {
            for (i, _) in &pending {
                if results[*i].is_ok() {
                    results[*i] = Err(e.clone());
                }
            }
        }
    }
    results
}

/// 捕获到的"原始状态"如果其实是我们自己之前设置的图标，视为默认图标
fn sanitize_restore(ops: &mut [Op], dir: &Path) {
    let ours = dir.join("icons");
    for op in ops.iter_mut() {
        match op {
            Op::MacSetIcon { png, .. } => {
                if png.as_deref().map(|p| Path::new(p).starts_with(&ours)).unwrap_or(false) {
                    *png = None;
                }
            }
            Op::WinSetIcon { location, index, .. } => {
                if Path::new(location.as_str()).starts_with(&ours) {
                    location.clear();
                    *index = 0;
                }
            }
        }
    }
}

fn apply_many(core: &Core, items: Vec<(AppInfo, image::RgbaImage)>, allow_elevate: bool) -> Report {
    let dir = &core.dir;
    let mut report = Report::default();
    let mut staged: Vec<(AppInfo, Record, Vec<Op>)> = Vec::new();
    {
        let store = core.store.lock().unwrap();
        for (app, img) in items {
            if app.protected {
                report.fail(
                    &app.name,
                    i18n::tr(
                        "系统自带应用受系统完整性保护（SIP），无法修改",
                        "Built-in system app, protected by System Integrity Protection (SIP)",
                    ),
                );
                continue;
            }
            let id = app.id.clone();
            let staged_one = (|| -> Result<(Record, Vec<Op>), String> {
                let images = dir.join("images");
                std::fs::create_dir_all(&images).map_err(|e| e.to_string())?;
                let image_path = images.join(format!("{id}.png"));
                std::fs::write(&image_path, imaging::to_png(&img)).map_err(|e| e.to_string())?;
                let icon_file = platform::write_icon_file(&id, &img, dir)?;
                let mut restore = store.records.get(&id).map(|r| r.restore.clone()).unwrap_or_default();
                let known: HashSet<String> = restore.iter().map(|o| o.target().to_string()).collect();
                let new_files: Vec<String> = app.files.iter().filter(|f| !known.contains(*f)).cloned().collect();
                let mut captured = platform::capture_restore_ops(&app, &id, dir, &new_files);
                sanitize_restore(&mut captured, dir);
                restore.extend(captured);
                let ops = platform::apply_ops(&app, &icon_file);
                Ok((
                    Record {
                        id: id.clone(),
                        name: app.name.clone(),
                        path: app.path.clone(),
                        icon_file,
                        image: image_path.to_string_lossy().to_string(),
                        restore,
                        applied_at: now_ts(),
                    },
                    ops,
                ))
            })();
            match staged_one {
                Ok((rec, ops)) => staged.push((app, rec, ops)),
                Err(e) => report.fail(&app.name, e),
            }
        }
    }
    if staged.is_empty() {
        return report;
    }
    let groups: Vec<Vec<Op>> = staged.iter().map(|(_, _, ops)| ops.clone()).collect();
    let results = execute(&groups, dir, allow_elevate);
    {
        let mut store = core.store.lock().unwrap();
        for ((app, rec, _), res) in staged.into_iter().zip(results) {
            match res {
                Ok(()) => {
                    report.ok.push(app.name.clone());
                    store.records.insert(rec.id.clone(), rec);
                }
                Err(e) => report.fail(&app.name, e),
            }
        }
    }
    core.save();
    if !report.ok.is_empty() && core.settings().refresh_shell {
        platform::refresh_shell(false);
    }
    report
}

fn restore_many(core: &Core, ids: &[String]) -> Report {
    let mut report = Report::default();
    let recs: Vec<Record> = {
        let store = core.store.lock().unwrap();
        ids.iter().filter_map(|id| store.records.get(id).cloned()).collect()
    };
    let groups: Vec<Vec<Op>> = recs.iter().map(|r| r.restore.clone()).collect();
    let results = execute(&groups, &core.dir, true);
    {
        let mut store = core.store.lock().unwrap();
        for (rec, res) in recs.into_iter().zip(results) {
            match res {
                Ok(()) => {
                    report.ok.push(rec.name.clone());
                    store.records.remove(&rec.id);
                    let _ = std::fs::remove_file(&rec.image);
                    let _ = std::fs::remove_file(&rec.icon_file);
                    let _ = std::fs::remove_file(core.dir.join("backup").join(format!("{}.png", rec.id)));
                }
                Err(e) => report.fail(&rec.name, e),
            }
        }
    }
    core.save();
    if !report.ok.is_empty() && core.settings().refresh_shell {
        platform::refresh_shell(false);
    }
    report
}

/// 检查所有已定制的应用，图标被更新还原了（或多出新快捷方式）就重新套上
fn reapply_all(core: &Core, allow_elevate: bool) -> Report {
    let apps = core.refresh_apps();
    let by_id: HashMap<&str, &AppInfo> = apps.iter().map(|a| (a.id.as_str(), a)).collect();
    let recs: Vec<Record> = core.store.lock().unwrap().records.values().cloned().collect();
    let mut items = Vec::new();
    for rec in recs {
        let Some(app) = by_id.get(rec.id.as_str()) else { continue };
        if platform::is_applied(app, &rec.icon_file) {
            continue;
        }
        if let Ok(img) = imaging::load(Path::new(&rec.image)) {
            items.push(((*app).clone(), img));
        }
    }
    if items.is_empty() {
        return Report::default();
    }
    apply_many(core, items, allow_elevate)
}

// ---------------------------------------------------------------- 按文件名批量匹配

fn normalize(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(|c| c.to_lowercase()).collect()
}

fn path_stem(p: &str) -> String {
    Path::new(p).file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string()
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct Match {
    id: String,
    name: String,
    image: String,
    image_name: String,
}

fn match_folder(apps: &[AppInfo], folder: &Path) -> Vec<Match> {
    let mut images: Vec<(String, PathBuf)> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(folder) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_file() && imaging::is_image(&p) {
                let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
                images.push((normalize(&stem), p));
            }
        }
    }
    let mut out = Vec::new();
    for app in apps {
        let names = [normalize(&app.name), normalize(&path_stem(&app.path))];
        let mut best: Option<(u32, &PathBuf)> = None;
        for (stem, p) in &images {
            if stem.is_empty() {
                continue;
            }
            let score = if *stem == names[0] {
                3
            } else if *stem == names[1] {
                2
            } else if stem.chars().count() >= 4
                && names.iter().any(|n| !n.is_empty() && (n.contains(stem.as_str()) || stem.contains(n.as_str())))
            {
                1
            } else {
                0
            };
            if score > 0 && best.map(|(s, _)| score > s).unwrap_or(true) {
                best = Some((score, p));
            }
        }
        if let Some((_, p)) = best {
            out.push(Match {
                id: app.id.clone(),
                name: app.name.clone(),
                image: p.to_string_lossy().to_string(),
                image_name: p.file_name().and_then(|s| s.to_str()).unwrap_or("").to_string(),
            });
        }
    }
    out
}

fn sanitize_filename(s: &str) -> String {
    let t: String = s
        .chars()
        .map(|c| if "<>:\"/\\|?*".contains(c) || c.is_control() { '_' } else { c })
        .collect();
    let t = t.trim().trim_matches('.').to_string();
    if t.is_empty() {
        "untitled".into()
    } else {
        t
    }
}

// ---------------------------------------------------------------- 前端命令

#[derive(Serialize)]
struct AppView {
    id: String,
    name: String,
    path: String,
    protected: bool,
    customized: bool,
    files: usize,
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn list_apps(core: CoreState<'_>) -> Result<Vec<AppView>, String> {
    let core = core.inner().clone();
    blocking(move || {
        let apps = core.refresh_apps();
        let store = core.store.lock().unwrap();
        apps.into_iter()
            .map(|a| AppView {
                customized: store.records.contains_key(&a.id),
                files: a.files.len(),
                id: a.id,
                name: a.name,
                path: a.path,
                protected: a.protected,
            })
            .collect()
    })
    .await
}

#[tauri::command]
async fn app_icons(core: CoreState<'_>, ids: Vec<String>, size: Option<u32>) -> Result<HashMap<String, String>, String> {
    let core = core.inner().clone();
    blocking(move || {
        platform::platform_init();
        core.apps_by_ids(&ids)
            .into_iter()
            .filter_map(|a| platform::current_icon_png(&a, size.unwrap_or(128)).map(|png| (a.id, imaging::data_url(&png))))
            .collect()
    })
    .await
}

#[tauri::command]
async fn preview(path: String, shape: Shape) -> Result<String, String> {
    blocking(move || imaging::preview(Path::new(&path), shape)).await?
}

#[tauri::command]
fn default_shape() -> Shape {
    Shape::default()
}

#[tauri::command]
async fn apply_icon(core: CoreState<'_>, ids: Vec<String>, image: String, shape: Shape) -> Result<Report, String> {
    let core = core.inner().clone();
    blocking(move || {
        let src = imaging::load(Path::new(&image))?;
        let img = imaging::process(&src, shape);
        let items = core.apps_by_ids(&ids).into_iter().map(|a| (a, img.clone())).collect();
        Ok(apply_many(&core, items, true))
    })
    .await?
}

#[derive(Deserialize)]
struct Pair {
    id: String,
    image: String,
}

#[tauri::command]
async fn apply_matches(core: CoreState<'_>, pairs: Vec<Pair>, shape: Shape) -> Result<Report, String> {
    let core = core.inner().clone();
    blocking(move || {
        let ids: Vec<String> = pairs.iter().map(|p| p.id.clone()).collect();
        let apps: HashMap<String, AppInfo> = core.apps_by_ids(&ids).into_iter().map(|a| (a.id.clone(), a)).collect();
        let mut report = Report::default();
        let mut items = Vec::new();
        for p in pairs {
            let Some(app) = apps.get(&p.id) else { continue };
            match imaging::load(Path::new(&p.image)) {
                Ok(src) => items.push((app.clone(), imaging::process(&src, shape))),
                Err(e) => report.fail(&app.name, e),
            }
        }
        let r = apply_many(&core, items, true);
        report.ok.extend(r.ok);
        report.failed.extend(r.failed);
        report
    })
    .await
}

#[tauri::command]
async fn restore_icon(core: CoreState<'_>, ids: Vec<String>) -> Result<Report, String> {
    let core = core.inner().clone();
    blocking(move || restore_many(&core, &ids)).await
}

#[tauri::command]
async fn reapply_now(core: CoreState<'_>) -> Result<Report, String> {
    let core = core.inner().clone();
    blocking(move || reapply_all(&core, true)).await
}

#[tauri::command]
async fn refresh_shell(hard: bool) -> Result<(), String> {
    blocking(move || platform::refresh_shell(hard)).await
}

#[tauri::command]
async fn match_folder_cmd(core: CoreState<'_>, folder: String) -> Result<Vec<Match>, String> {
    let core = core.inner().clone();
    blocking(move || {
        let mut apps = core.apps.lock().unwrap().clone();
        if apps.is_empty() {
            apps = core.refresh_apps();
        }
        match_folder(&apps, Path::new(&folder))
    })
    .await
}

#[derive(Serialize)]
struct ThemeView {
    name: String,
    path: String,
    count: usize,
    previews: Vec<String>,
}

fn themes_dir(core: &Core) -> PathBuf {
    core.dir.join("themes")
}

#[tauri::command]
async fn list_themes(core: CoreState<'_>) -> Result<Vec<ThemeView>, String> {
    let core = core.inner().clone();
    blocking(move || {
        let mut out = Vec::new();
        let Ok(rd) = std::fs::read_dir(themes_dir(&core)) else { return out };
        for e in rd.flatten() {
            let p = e.path();
            if !p.is_dir() {
                continue;
            }
            let mut imgs: Vec<PathBuf> = std::fs::read_dir(&p)
                .map(|r| r.flatten().map(|e| e.path()).filter(|p| imaging::is_image(p)).collect())
                .unwrap_or_default();
            imgs.sort();
            let previews = imgs
                .iter()
                .take(4)
                .filter_map(|i| imaging::load(i).ok())
                .map(|i| imaging::data_url(&imaging::to_png(&image::imageops::thumbnail(&i, 64, 64))))
                .collect();
            out.push(ThemeView {
                name: e.file_name().to_string_lossy().to_string(),
                path: p.to_string_lossy().to_string(),
                count: imgs.len(),
                previews,
            });
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    })
    .await
}

/// 主题就是一个"以应用名命名的图片"文件夹，和批量导入用的是同一种格式
#[tauri::command]
async fn save_theme(core: CoreState<'_>, name: String) -> Result<usize, String> {
    let core = core.inner().clone();
    blocking(move || {
        let dir = themes_dir(&core).join(sanitize_filename(&name));
        if dir.exists() {
            std::fs::remove_dir_all(&dir).map_err(|e| e.to_string())?;
        }
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let recs: Vec<Record> = core.store.lock().unwrap().records.values().cloned().collect();
        let mut n = 0;
        for r in recs {
            if std::fs::copy(&r.image, dir.join(format!("{}.png", sanitize_filename(&r.name)))).is_ok() {
                n += 1;
            }
        }
        Ok(n)
    })
    .await?
}

#[tauri::command]
async fn delete_theme(core: CoreState<'_>, name: String) -> Result<(), String> {
    let dir = themes_dir(&core).join(sanitize_filename(&name));
    std::fs::remove_dir_all(dir).map_err(|e| e.to_string())
}

#[tauri::command]
fn reveal(path: String) {
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("/usr/bin/open").arg("-R").arg(&path).spawn();
    #[cfg(windows)]
    let _ = std::process::Command::new("explorer.exe").arg(format!("/select,{path}")).spawn();
    #[cfg(not(any(target_os = "macos", windows)))]
    let _ = path;
}

#[derive(Serialize, Deserialize)]
struct SettingsView {
    auto_reapply: bool,
    interval_min: u32,
    refresh_shell: bool,
    keep_in_tray: bool,
    autostart: bool,
    platform: String,
    data_dir: String,
    #[serde(default)]
    lang: String,
}

#[tauri::command]
fn get_settings(app: AppHandle, core: CoreState<'_>) -> SettingsView {
    let s = core.settings();
    SettingsView {
        auto_reapply: s.auto_reapply,
        interval_min: s.interval_min,
        refresh_shell: s.refresh_shell,
        keep_in_tray: s.keep_in_tray,
        autostart: app.autolaunch().is_enabled().unwrap_or(false),
        platform: std::env::consts::OS.to_string(),
        data_dir: core.dir.to_string_lossy().to_string(),
        lang: i18n::lang().to_string(),
    }
}

#[tauri::command]
fn set_settings(app: AppHandle, core: CoreState<'_>, settings: SettingsView) -> Result<(), String> {
    {
        let mut store = core.store.lock().unwrap();
        store.settings = Settings {
            auto_reapply: settings.auto_reapply,
            interval_min: settings.interval_min.clamp(1, 1440),
            refresh_shell: settings.refresh_shell,
            keep_in_tray: settings.keep_in_tray,
        };
    }
    core.save();
    let al = app.autolaunch();
    if settings.autostart != al.is_enabled().unwrap_or(false) {
        if settings.autostart {
            al.enable().map_err(|e| e.to_string())?;
        } else {
            al.disable().map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------- 启动

fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

fn spawn_watcher(app: AppHandle, core: Arc<Core>) {
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(8));
        loop {
            let s = core.settings();
            if s.auto_reapply {
                let r = reapply_all(&core, false);
                if !r.ok.is_empty() || !r.failed.is_empty() {
                    let _ = app.emit("auto-reapplied", &r);
                }
            }
            std::thread::sleep(Duration::from_secs(s.interval_min.max(1) as u64 * 60));
        }
    });
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let start_hidden = std::env::args().any(|a| a == "--hidden");
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_autostart::init(MacosLauncher::LaunchAgent, Some(vec!["--hidden"])))
        .setup(move |app| {
            let dir = app.path().app_data_dir().expect("app data dir");
            let core = Arc::new(Core::load(dir));
            app.manage(core.clone());

            let show = MenuItem::with_id(app, "show", i18n::tr("打开 Swap Your Icon", "Open Swap Your Icon"), true, None::<&str>)?;
            let check = MenuItem::with_id(app, "check", i18n::tr("检查并重套图标", "Check & Reapply Icons"), true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", i18n::tr("退出", "Quit"), true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show, &check, &quit])?;
            let tray_core = core.clone();
            TrayIconBuilder::new()
                .icon(tauri::image::Image::from_bytes(include_bytes!("../icons/tray.png"))?)
                .icon_as_template(true)
                .tooltip("Swap Your Icon")
                .menu(&menu)
                .show_menu_on_left_click(cfg!(target_os = "macos"))
                .on_menu_event(move |app, ev| match ev.id().as_ref() {
                    "show" => show_main(app),
                    "check" => {
                        let core = tray_core.clone();
                        let app = app.clone();
                        std::thread::spawn(move || {
                            let r = reapply_all(&core, true);
                            let _ = app.emit("auto-reapplied", &r);
                        });
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, ev| {
                    if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = ev {
                        if !cfg!(target_os = "macos") {
                            show_main(tray.app_handle());
                        }
                    }
                })
                .build(app)?;

            if start_hidden {
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.hide();
                }
            }
            spawn_watcher(app.handle().clone(), core);
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                let core = window.state::<Arc<Core>>();
                if core.settings().keep_in_tray {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            list_apps,
            app_icons,
            preview,
            default_shape,
            apply_icon,
            apply_matches,
            restore_icon,
            reapply_now,
            refresh_shell,
            match_folder_cmd,
            list_themes,
            save_theme,
            delete_theme,
            reveal,
            get_settings,
            set_settings
        ])
        .build(tauri::generate_context!())
        .expect("error while building Swap Your Icon")
        .run(|app, event| {
            // macOS：点击 Dock 图标时重新显示窗口
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Reopen { .. } = event {
                show_main(app);
            }
            let _ = (app, &event);
        });
}
