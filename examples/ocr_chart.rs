//! OCR 调参标尺图：用系统字体渲染一张“终端式”密排中英文测试图。
//!
//! - 背景用户 temp 目录文件随时会被覆盖 / 删除，标尺图可随时重建，保证调参可复现；
//! - 用法：`cargo run --example ocr_chart`，输出 `temp/ocr-chart.png`（1200x800）；
//! - 内容：黑底等宽式终端行（中英混排、大小字号、次要灰字），专治检测框偏大与行间打架。

use ab_glyph::{Font, FontVec, PxScale, ScaleFont as _, point};
use image::{Rgb, RgbImage};

/// 标尺行：文本 + 字号 + 颜色。
const ROWS: &[(&str, f32, [u8; 3])] = &[
    (
        "PS C:\\code\\rupix> cargo run --profile size",
        20.0,
        [220, 220, 220],
    ),
    (
        "Compiling rupix v0.1.0 (C:\\code\\rupix)",
        20.0,
        [220, 220, 220],
    ),
    (
        "warning: Linker stdout: LINK : warning LNK4098",
        20.0,
        [255, 200, 100],
    ),
    (
        "Finished `size` profile [optimized] target(s) in 3m 07s",
        20.0,
        [220, 220, 220],
    ),
    ("拾景 Rupix 截图 OCR 调参标尺", 28.0, [255, 255, 255]),
    (
        "托盘图标（正常） 托盘图标（悬停） 托盘图标（激活）",
        20.0,
        [180, 180, 180],
    ),
    (
        "主图标（深色背景） 主图标（浅色背景） 单色图标",
        16.0,
        [140, 140, 140],
    ),
    (
        "0123456789 abcdefghijklmnopqrstuvwxyz",
        14.0,
        [120, 120, 120],
    ),
];

/// 单行文字光栅化（与烧录同算法：字形位图逐字拼排，覆盖度调 alpha）。
fn draw_text(
    img: &mut RgbImage,
    font: &FontVec,
    x: f32,
    y: f32,
    content: &str,
    size: f32,
    rgb: [u8; 3],
) {
    let scale = PxScale::from(size);
    let scaled = font.as_scaled(scale);
    let mut caret_x = x;
    let baseline = y + scaled.ascent();
    for ch in content.chars() {
        let glyph_id = scaled.glyph_id(ch);
        let glyph = glyph_id.with_scale_and_position(scale, point(caret_x, baseline));
        if let Some(outlined) = scaled.outline_glyph(glyph) {
            let bounds = outlined.px_bounds();
            outlined.draw(|gx, gy, v| {
                if v <= 0.0 {
                    return;
                }
                let px = bounds.min.x as i32 + gx as i32;
                let py = bounds.min.y as i32 + gy as i32;
                if px < 0 || py < 0 || px >= img.width() as i32 || img.height() as i32 <= py {
                    return;
                }
                // 深底浅字：按覆盖度混合底色。
                let dst = img.get_pixel(px as u32, py as u32);
                let mix = |s: u8, d: u8| (s as f32 * v + d as f32 * (1.0 - v)) as u8;
                img.put_pixel(
                    px as u32,
                    py as u32,
                    Rgb([
                        mix(rgb[0], dst[0]),
                        mix(rgb[1], dst[1]),
                        mix(rgb[2], dst[2]),
                    ]),
                );
            });
        }
        caret_x += scaled.h_advance(glyph_id);
    }
}

fn main() -> anyhow::Result<()> {
    // 微软雅黑首选（中文覆盖齐全），与烧录侧字体一致，标尺才有代表性。
    let font_data = std::fs::read("C:\\Windows\\Fonts\\msyh.ttc")?;
    let font = FontVec::try_from_vec_and_index(font_data, 0)?;
    let mut img = RgbImage::from_pixel(1200, 800, Rgb([15, 15, 20]));
    // 行距 1.6 倍字号：模拟终端行高（含行距 padding，专测框偏大）。
    let mut y = 40.0;
    for (text, size, color) in ROWS {
        draw_text(&mut img, &font, 40.0, y, text, *size, *color);
        y += size * 1.6;
    }
    img.save("temp/ocr-chart.png")?;
    println!("标尺图已写入 temp/ocr-chart.png");
    Ok(())
}
