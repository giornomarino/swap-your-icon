//! 图片处理：读取任意图片，裁成方形，按需套上 macOS 圆角方形 / 圆形遮罩，
//! 输出 1024×1024 PNG 或多尺寸 ICO。

use base64::Engine;
use image::codecs::ico::{IcoEncoder, IcoFrame};
use image::imageops::FilterType;
use image::{ExtendedColorType, ImageEncoder, Rgba, RgbaImage};
use serde::{Deserialize, Serialize};
use std::io::Cursor;
use std::path::Path;

pub const CANVAS: u32 = 1024;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Shape {
    /// 保持原图（等比缩放进方形画布，透明留白）
    Original,
    /// 苹果风格圆角方形（连续曲率，近似超椭圆）
    Squircle,
    /// 圆形
    Circle,
}

impl Default for Shape {
    fn default() -> Self {
        if cfg!(target_os = "macos") {
            Shape::Squircle
        } else {
            Shape::Original
        }
    }
}

/// macOS 图标规范：1024 画布里，主体是 824×824，四周留 100px。
/// Windows 上不需要这么大的留白。
fn shape_margin() -> u32 {
    if cfg!(target_os = "macos") {
        100
    } else {
        24
    }
}

pub fn load(path: &Path) -> Result<RgbaImage, String> {
    let img = image::open(path).map_err(|e| format!("{} {}{}{e}", crate::i18n::tr("无法读取图片", "Can't read image"), path.display(), crate::i18n::colon()))?;
    Ok(img.to_rgba8())
}

/// 处理成 1024×1024 的最终图标
pub fn process(src: &RgbaImage, shape: Shape) -> RgbaImage {
    match shape {
        Shape::Original => fit_contain(src, CANVAS),
        Shape::Squircle | Shape::Circle => {
            let margin = shape_margin();
            let inner = CANVAS - margin * 2;
            let filled = fill_cover(src, inner);
            let masked = apply_mask(&filled, shape);
            let mut out = RgbaImage::from_pixel(CANVAS, CANVAS, Rgba([0, 0, 0, 0]));
            image::imageops::overlay(&mut out, &masked, margin as i64, margin as i64);
            out
        }
    }
}

/// 等比缩放并居中放进 size×size 的透明画布
fn fit_contain(src: &RgbaImage, size: u32) -> RgbaImage {
    let (w, h) = src.dimensions();
    let scale = size as f32 / w.max(h) as f32;
    let nw = ((w as f32 * scale).round() as u32).max(1);
    let nh = ((h as f32 * scale).round() as u32).max(1);
    let resized = image::imageops::resize(src, nw, nh, FilterType::Lanczos3);
    let mut out = RgbaImage::from_pixel(size, size, Rgba([0, 0, 0, 0]));
    image::imageops::overlay(&mut out, &resized, ((size - nw) / 2) as i64, ((size - nh) / 2) as i64);
    out
}

/// 等比缩放到覆盖 size×size，再居中裁切
fn fill_cover(src: &RgbaImage, size: u32) -> RgbaImage {
    let (w, h) = src.dimensions();
    let scale = size as f32 / w.min(h) as f32;
    let nw = ((w as f32 * scale).ceil() as u32).max(size);
    let nh = ((h as f32 * scale).ceil() as u32).max(size);
    let resized = image::imageops::resize(src, nw, nh, FilterType::Lanczos3);
    image::imageops::crop_imm(&resized, (nw - size) / 2, (nh - size) / 2, size, size).to_image()
}

/// 4×4 超采样抗锯齿遮罩
fn apply_mask(img: &RgbaImage, shape: Shape) -> RgbaImage {
    let size = img.width();
    let r = size as f64 / 2.0;
    let n = 5.0_f64; // 超椭圆指数，≈ Apple 连续圆角
    let inside = |x: f64, y: f64| -> bool {
        let dx = ((x - r) / r).abs();
        let dy = ((y - r) / r).abs();
        match shape {
            Shape::Circle => dx * dx + dy * dy <= 1.0,
            _ => dx.powf(n) + dy.powf(n) <= 1.0,
        }
    };
    const SS: u32 = 4;
    let mut out = img.clone();
    for y in 0..size {
        for x in 0..size {
            // 快速判断：中心区域一定在内部
            let cx = (x as f64 + 0.5 - r).abs() / r;
            let cy = (y as f64 + 0.5 - r).abs() / r;
            if cx < 0.7 && cy < 0.7 {
                continue;
            }
            let mut hit = 0u32;
            for sy in 0..SS {
                for sx in 0..SS {
                    let px = x as f64 + (sx as f64 + 0.5) / SS as f64;
                    let py = y as f64 + (sy as f64 + 0.5) / SS as f64;
                    if inside(px, py) {
                        hit += 1;
                    }
                }
            }
            let p = out.get_pixel_mut(x, y);
            p[3] = ((p[3] as u32 * hit) / (SS * SS)) as u8;
        }
    }
    out
}

pub fn to_png(img: &RgbaImage) -> Vec<u8> {
    let mut buf = Vec::new();
    image::codecs::png::PngEncoder::new(&mut buf)
        .write_image(img.as_raw(), img.width(), img.height(), ExtendedColorType::Rgba8)
        .expect("png encode");
    buf
}

#[cfg_attr(not(windows), allow(dead_code))]
/// Windows 用的多尺寸 ICO（每帧都是 PNG 压缩，Vista 以后都支持）
pub fn to_ico(img: &RgbaImage) -> Result<Vec<u8>, String> {
    let mut frames = Vec::new();
    for s in [16u32, 20, 24, 32, 40, 48, 64, 96, 128, 256] {
        let small = image::imageops::resize(img, s, s, FilterType::Lanczos3);
        let png = to_png(&small);
        frames.push(IcoFrame::with_encoded(png, s, s, ExtendedColorType::Rgba8).map_err(|e| e.to_string())?);
    }
    let mut buf = Vec::new();
    IcoEncoder::new(Cursor::new(&mut buf))
        .encode_images(&frames)
        .map_err(|e| e.to_string())?;
    Ok(buf)
}

pub fn data_url(png: &[u8]) -> String {
    format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(png))
}

/// 预览用：处理后缩到 256
pub fn preview(path: &Path, shape: Shape) -> Result<String, String> {
    let src = load(path)?;
    let out = process(&src, shape);
    let small = image::imageops::resize(&out, 256, 256, FilterType::Triangle);
    Ok(data_url(&to_png(&small)))
}

pub const IMAGE_EXTS: &[&str] = &["png", "jpg", "jpeg", "webp", "bmp", "gif", "ico", "tif", "tiff"];

pub fn is_image(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| IMAGE_EXTS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}
