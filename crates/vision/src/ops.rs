//! 本机图像操作：所有几何都在原始像素坐标上做，不经过模型。

use image::{DynamicImage, ImageFormat, Rgba, RgbaImage};
use std::io::Cursor;

/// 解码图片（png/jpeg/webp 由 image 特性决定）。
pub fn decode(bytes: &[u8]) -> Result<RgbaImage, String> {
    let image = image::load_from_memory(bytes).map_err(|error| format!("图片解码失败：{error}"))?;
    Ok(image.to_rgba8())
}

pub fn encode_png(image: &RgbaImage) -> Result<Vec<u8>, String> {
    let mut out = Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(image.clone())
        .write_to(&mut out, ImageFormat::Png)
        .map_err(|error| format!("图片编码失败：{error}"))?;
    Ok(out.into_inner())
}

/// 裁剪；越界自动夹回原图，宽高至少 1。
pub fn crop(image: &RgbaImage, x: i64, y: i64, width: i64, height: i64) -> RgbaImage {
    let (image_width, image_height) = image.dimensions();
    let x = x.clamp(0, i64::from(image_width.saturating_sub(1))) as u32;
    let y = y.clamp(0, i64::from(image_height.saturating_sub(1))) as u32;
    let width = width.max(1).min(i64::from(image_width - x)) as u32;
    let height = height.max(1).min(i64::from(image_height - y)) as u32;
    image::imageops::crop_imm(image, x, y, width, height).to_image()
}

/// 在原图副本上画一个醒目方框；不改变原图。
pub fn annotate(image: &RgbaImage, x: i64, y: i64, width: i64, height: i64) -> RgbaImage {
    let mut out = image.clone();
    let (image_width, image_height) = out.dimensions();
    let x1 = x.clamp(0, i64::from(image_width.saturating_sub(1))) as u32;
    let y1 = y.clamp(0, i64::from(image_height.saturating_sub(1))) as u32;
    let x2 = (x + width.max(1) - 1).clamp(0, i64::from(image_width.saturating_sub(1))) as u32;
    let y2 = (y + height.max(1) - 1).clamp(0, i64::from(image_height.saturating_sub(1))) as u32;
    let color = Rgba([230, 30, 30, 255]);
    let thickness = 3_u32;
    for offset in 0..thickness {
        if y1 + offset < image_height {
            for x in x1..=x2 {
                out.put_pixel(x, y1 + offset, color);
            }
        }
        if y2 >= offset {
            for x in x1..=x2 {
                out.put_pixel(x, y2.saturating_sub(offset), color);
            }
        }
        if x1 + offset < image_width {
            for y in y1..=y2 {
                out.put_pixel(x1 + offset, y, color);
            }
        }
        if x2 >= offset {
            for y in y1..=y2 {
                out.put_pixel(x2.saturating_sub(offset), y, color);
            }
        }
    }
    out
}

/// 主要颜色：按 32 级量化后取前 n 个，带占比。
pub fn dominant_colors(image: &RgbaImage, count: usize) -> Vec<(String, f64)> {
    let mut buckets: std::collections::HashMap<(u8, u8, u8), u64> =
        std::collections::HashMap::new();
    let mut total = 0_u64;
    for pixel in image.pixels() {
        if pixel[3] < 16 {
            continue;
        }
        let key = (pixel[0] & 0xe0, pixel[1] & 0xe0, pixel[2] & 0xe0);
        *buckets.entry(key).or_default() += 1;
        total += 1;
    }
    if total == 0 {
        return Vec::new();
    }
    let mut rows: Vec<((u8, u8, u8), u64)> = buckets.into_iter().collect();
    rows.sort_by(|left, right| right.1.cmp(&left.1));
    rows.into_iter()
        .take(count)
        .map(|((red, green, blue), hits)| {
            (
                format!("#{red:02x}{green:02x}{blue:02x}"),
                hits as f64 / total as f64,
            )
        })
        .collect()
}

/// 两图像素差异：同一坐标颜色差超过阈值即计一个像素；尺寸不同按重叠区算。
pub fn pixel_diff(left: &RgbaImage, right: &RgbaImage, threshold: u8) -> (u64, f64, u32, u32) {
    let width = left.width().min(right.width());
    let height = left.height().min(right.height());
    let mut changed = 0_u64;
    let total = u64::from(width) * u64::from(height);
    for y in 0..height {
        for x in 0..width {
            let a = left.get_pixel(x, y);
            let b = right.get_pixel(x, y);
            let different = [0, 1, 2]
                .iter()
                .any(|index| a[*index].abs_diff(b[*index]) > threshold);
            if different {
                changed += 1;
            }
        }
    }
    let ratio = if total == 0 {
        0.0
    } else {
        changed as f64 / total as f64
    };
    (changed, ratio, width, height)
}

/// 等比缩到最长边 limit 像素（不放大）。
pub fn fit_long_edge(image: &RgbaImage, limit: u32) -> RgbaImage {
    let (width, height) = image.dimensions();
    if width.max(height) <= limit {
        return image.clone();
    }
    let scale = limit as f64 / width.max(height) as f64;
    let new_width = (f64::from(width) * scale).round().max(1.0) as u32;
    let new_height = (f64::from(height) * scale).round().max(1.0) as u32;
    image::imageops::resize(
        image,
        new_width,
        new_height,
        image::imageops::FilterType::Lanczos3,
    )
}

/// 内置测试图：16×16 纯红，给「测试看图」用；不读用户文件。
pub fn test_image() -> Result<Vec<u8>, String> {
    let image = RgbaImage::from_pixel(16, 16, Rgba([220, 20, 20, 255]));
    encode_png(&image)
}
