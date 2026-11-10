# Swap Your Icon

[English](README.md) · 中文

给 Mac 和 Windows 上的应用换图标：选应用 → 拖入图片 → 点「应用」。

## 功能

- **拖入图片即换**：支持 PNG / JPG / WebP / BMP / GIF / ICO / TIFF。可选「原样 / 圆角方形 / 圆形」，圆角方形按 Apple 图标网格自动裁切留边。
- **一键还原**：修改前的状态会被记录下来（原本就有自定义图标的也会备份），随时恢复。
- **更新后自动重套**：应用更新会把图标冲掉，Swap Your Icon 在后台定时检查并重新套上（默认每 10 分钟，可开机自启、关窗口后留在菜单栏/托盘）。
- **批量 / 主题**：选一个文件夹，图片按应用名命名（`微信.png`、`Google Chrome.png`），自动匹配后一次换完。当前所有自定义图标可以存成一个主题，主题就是同样格式的文件夹，可以直接拷给别人或另一台电脑。
- 界面语言可选：跟随系统 / English / 中文（设置 → 语言）。
- macOS 上无法修改的系统应用统一收进可折叠的「系统应用」分组。
- 一键更新：启动时自动检查 GitHub Releases 上的新版本，在设置里一键下载（SHA-256 校验）、安装并重启。
- 多选：⌘/Ctrl 点击、Shift 连选、⌘/Ctrl+A 全选，可以把同一张图片套到多个应用上。

## 各平台原理

| | 改的是什么 | 限制 |
|---|---|---|
| macOS | 用 `NSWorkspace setIcon:forFile:` 给 `.app` 设置自定义图标（和 Finder「显示简介」里粘贴图标同一机制） | 系统自带应用（Safari、/System/Applications 下的）受 SIP 保护改不了；root 拥有的应用（如 App Store 应用）会弹管理员密码框；首次可能需要在「隐私与安全性 → App 管理」里允许 Swap Your Icon |
| Windows | 开始菜单、桌面、任务栏固定项里指向该程序的所有 `.lnk`，以及 Steam 游戏等用的 `.url` 的图标 | 不改 exe（不破坏签名）；所有用户共用的快捷方式会弹 UAC；任务栏图标偶尔要「重启资源管理器」才更新 |

需要管理员权限时，Swap Your Icon 会以 `--elevated <任务文件>` 参数提权启动自身，只执行那几条修改，一次操作只弹一次授权框。

数据保存在应用数据目录（设置里有「打开数据文件夹」）：`state.json` 记录、`images/` 处理后的图标、`backup/` 原有自定义图标备份、`themes/` 主题。

## 构建

需要 Rust（stable）和 Tauri CLI 2：

```bash
cargo install tauri-cli --version "^2" --locked
cd src-tauri
cargo tauri dev          # 开发运行
cargo tauri build        # 打包：Mac 出 .dmg，Windows 出 setup.exe 安装程序
```

前端是纯 HTML/CSS/JS（`ui/`），不需要 npm。

**不想在本地装环境**：把项目推到 GitHub，在 Actions 里手动运行 `build`（或推一个 `v0.1.0` 这样的 tag），会同时产出 Mac 通用版 .dmg 和 Windows 安装包。

未签名的 Mac 版第一次打开：右键 → 打开；或者 `xattr -cr "/Applications/Swap Your Icon.app"`。

## 项目结构

```
ui/                     界面（index.html / style.css / app.js）
src-tauri/src/
  main.rs               入口；处理 --elevated 提权子进程
  lib.rs                记录、还原、自动重套、批量匹配、主题、托盘、命令
  imaging.rs            裁切、圆角/圆形遮罩、PNG / 多尺寸 ICO 输出
  platform/mac.rs       macOS 实现
  platform/win.rs       Windows 实现
```
