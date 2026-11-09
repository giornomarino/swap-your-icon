//! 其他平台（仅用于开发时能编译）
use super::{AppInfo, Op, OpError};
use image::RgbaImage;
use std::path::Path;

pub fn platform_init() {}
pub fn list_apps() -> Vec<AppInfo> {
    Vec::new()
}
pub fn current_icon_png(_app: &AppInfo, _size: u32) -> Option<Vec<u8>> {
    None
}
pub fn write_icon_file(_id: &str, _img: &RgbaImage, _dir: &Path) -> Result<String, String> {
    Err("unsupported platform".into())
}
pub fn apply_ops(_app: &AppInfo, _icon_file: &str) -> Vec<Op> {
    Vec::new()
}
pub fn capture_restore_ops(_app: &AppInfo, _id: &str, _dir: &Path, _files: &[String]) -> Vec<Op> {
    Vec::new()
}
pub fn is_applied(_app: &AppInfo, _icon_file: &str) -> bool {
    true
}
pub fn exec_op(_op: &Op) -> Result<(), OpError> {
    Err(OpError::Other("unsupported platform".into()))
}
pub fn run_elevated(_ops: &[Op], _dir: &Path) -> Result<(), String> {
    Err("unsupported platform".into())
}
pub fn refresh_shell(_hard: bool) {}
