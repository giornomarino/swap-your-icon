# Swap Your Icon

English · [中文](README.zh-CN.md)

Change the icons of apps on macOS and Windows: pick an app → drop in an image → click **Apply**.

## Features

- **Drop an image to swap**: PNG / JPG / WebP / BMP / GIF / ICO / TIFF. Choose *Original*, *Rounded* or *Circle* — *Rounded* crops to Apple's icon grid automatically.
- **One-click restore**: the original state is recorded before every change (existing custom icons are backed up too).
- **Auto-reapply after updates**: app updates wipe custom icons; Swap Your Icon checks in the background and puts them back (every 10 min by default; can launch at login and stay in the menu bar / tray).
- **Batch & themes**: pick a folder of images named after apps (`Google Chrome.png`) and they're matched and applied in one go. Save all your current custom icons as a theme — a theme is just such a folder, so you can share it or import it on another computer.
- Multi-select with ⌘/Ctrl-click, Shift-click, ⌘/Ctrl+A to apply one image to several apps.
- Interface language: follow the system, English or 中文 (Settings → Language).
- Built-in macOS apps that can't be changed are tucked into a collapsible “System apps” group.
- One-click updates: new releases are detected at launch and installed from Settings (downloaded from GitHub Releases and verified with SHA-256).

## How it works

| | What changes | Limits |
|---|---|---|
| macOS | Sets a custom icon on the `.app` with `NSWorkspace setIcon:forFile:` (the same mechanism as pasting an icon in Finder's Get Info) | Built-in apps (Safari, anything in /System/Applications) are SIP-protected; root-owned apps (e.g. App Store apps) ask for your admin password; macOS may require allowing Swap Your Icon under Privacy & Security → App Management |
| Windows | Every `.lnk` in the Start menu, desktop and pinned taskbar items pointing at the program, plus `.url` shortcuts (e.g. Steam games) | The exe is never modified (signatures stay intact); shortcuts shared by all users trigger UAC; the taskbar sometimes needs “Restart Explorer” to refresh |

When admin rights are needed, the app relaunches itself elevated with `--elevated <job file>` and runs only those changes — one prompt per operation.

## Download

Grab the latest `.dmg` (macOS, universal) or `setup.exe` (Windows) from [Releases](../../releases).

The macOS build isn't notarized. On first launch, right-click the app → Open, or run `xattr -cr "/Applications/Swap Your Icon.app"`.

## Build from source

Requires stable Rust and the Tauri CLI 2:

```bash
cargo install tauri-cli --version "^2" --locked
cd src-tauri
cargo tauri dev      # run in development
cargo tauri build    # package: .dmg on macOS, setup.exe on Windows
```

The front end is plain HTML/CSS/JS in `ui/` — no npm needed. Pushing a tag (e.g. `6703`) builds both platforms on GitHub Actions and publishes a release.

## Project layout

```
ui/                     interface (index.html / style.css / app.js / i18n.js)
src-tauri/src/
  main.rs               entry point; handles the --elevated child process
  lib.rs                records, restore, auto-reapply, batch matching, themes, tray, commands
  i18n.rs               system-language detection
  imaging.rs            cropping, rounded/circle masks, PNG / multi-size ICO output
  platform/mac.rs       macOS implementation
  platform/win.rs       Windows implementation
```
