//! 导出模块：把选区底图与矢量标注合成为最终 PNG，并负责保存 / 复制。
//!
//! 屏幕看到的标注是矢量绘制的，落盘时必须逐像素栅格化：
//! 直线 / 矩形 / 椭圆 / 箭头用笔刷印章法绘制，马赛克用分块取色，
//! 文字用系统字体的字形位图逐字拼排（依赖 `ab_glyph`，不引入重量级排版库）。

use std::{io::Cursor, path::Path};

use ab_glyph::{Font, FontVec, PxScale, ScaleFont as _, point};
use image::{DynamicImage, ImageFormat as ImgFormat, Rgba, RgbaImage};

use crate::shapes::{AnnoColor, PixelRect, Shape};

/// 文字导出时的默认字号（图片像素）。
pub const DEFAULT_TEXT_SIZE: f32 = 22.0;

/// 把底图按选区裁剪，并把落在选区内的标注全部烧录，返回最终图片。
///
/// # 参数
/// - `base`: 截屏首帧全图。
/// - `sel`: 选区（全图图片像素坐标），为空时视为全图。
/// - `shapes`: 全部矢量标注（全图图片像素坐标）。
/// - `font`: 系统字体（文字烧录用），缺失时跳过文字。
pub fn render_export(
    base: &RgbaImage,
    sel: PixelRect,
    shapes: &[Shape],
    font: &Option<FontVec>,
) -> RgbaImage {
    // 选区钳制到图片范围内，避免越界 panic。
    let (bw, bh) = (base.width() as f32, base.height() as f32);
    let x0 = sel.x.clamp(0.0, bw.max(1.0)) as u32;
    let y0 = sel.y.clamp(0.0, bh.max(1.0)) as u32;
    let x1 = (sel.x + sel.w).clamp(1.0, bw) as u32;
    let y1 = (sel.y + sel.h).clamp(1.0, bh) as u32;
    let (x1, y1) = (x1.max(x0 + 1), y1.max(y0 + 1));

    // 先裁出底图，后续绘制全部换算到裁剪后的局部坐标。
    let cropped = image::imageops::crop_imm(base, x0, y0, x1 - x0, y1 - y0).to_image();
    let mut out = cropped;
    let clip = PixelRect::new(0.0, 0.0, (x1 - x0) as f32, (y1 - y0) as f32);

    for shape in shapes {
        draw_shape(&mut out, shape, x0 as f32, y0 as f32, &clip, font);
    }
    out
}

/// 把图片编码为 PNG 字节（保存文件与复制剪贴板共用）。
pub fn png_bytes(img: &RgbaImage) -> Option<Vec<u8>> {
    let mut buf = Vec::new();
    DynamicImage::ImageRgba8(img.clone())
        .write_to(&mut Cursor::new(&mut buf), ImgFormat::Png)
        .ok()?;
    Some(buf)
}

/// 把 PNG 字节写入剪贴板图片项。
///
/// # 参数
/// - `cx`: 应用上下文（`Context` 可解引用为 `App`）。
/// - `png`: PNG 文件字节。
pub fn copy_png_to_clipboard(cx: &mut rgpui::App, png: &[u8]) -> Result<(), String> {
    let image = rgpui::Image::from_bytes(rgpui::ImageFormat::Png, png.to_vec());
    cx.write_to_clipboard(rgpui::ClipboardItem::new_image(&image));
    Ok(())
}

/// 把图片文件作为“文件项”写入系统剪贴板，便于在资源管理器目录中直接粘贴。
///
/// # 参数
/// - `path`: 已存在的 PNG 文件路径。
#[cfg(target_os = "windows")]
pub fn copy_file_to_clipboard(path: &Path) -> Result<(), String> {
    if !path.is_file() {
        return Err("剪贴板文件不存在".to_string());
    }

    // Windows 资源管理器需要 CF_HDROP 文件列表；PowerShell 的 Set-Clipboard -LiteralPath
    // 会按系统格式写入文件项，比普通文本路径更适合直接粘贴成新文件。
    let output = std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            "Set-Clipboard -LiteralPath $args[0]",
        ])
        .arg(path.as_os_str())
        .output()
        .map_err(|err| format!("调用系统剪贴板失败：{err}"))?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if stderr.is_empty() {
            Err("写入文件剪贴板失败".to_string())
        } else {
            Err(stderr)
        }
    }
}

/// 把图片文件作为“文件项”写入系统剪贴板，非 Windows 平台暂时回退为不支持。
///
/// # 参数
/// - `path`: 已存在的 PNG 文件路径。
#[cfg(not(target_os = "windows"))]
pub fn copy_file_to_clipboard(_path: &Path) -> Result<(), String> {
    Err("当前平台暂不支持复制为文件".to_string())
}

/// 加载系统字体（供文字烧录），按常见 Windows 中文字体依次尝试。
///
/// # 返回
/// 成功返回字体，全部缺失返回 `None`（此时导出跳过文字标注）。
pub fn load_system_font() -> Option<FontVec> {
    // 微软雅黑优先（Win10/11 自带，中文覆盖齐全），宋黑备用，英文保底。
    const CANDIDATES: &[&str] = &[
        "C:\\Windows\\Fonts\\msyh.ttc",
        "C:\\Windows\\Fonts\\simsun.ttc",
        "C:\\Windows\\Fonts\\simhei.ttf",
        "C:\\Windows\\Fonts\\segoeui.ttf",
    ];
    for path in CANDIDATES {
        if let Ok(data) = std::fs::read(path) {
            // ttc 是字体集合，按索引 0 取第一个；ttf 同样兼容该接口。
            if let Ok(font) = FontVec::try_from_vec_and_index(data, 0) {
                return Some(font);
            }
        }
    }
    None
}

/// 绘制单个标注（坐标从全图换算到裁剪局部坐标）。
fn draw_shape(
    img: &mut RgbaImage,
    shape: &Shape,
    off_x: f32,
    off_y: f32,
    clip: &PixelRect,
    font: &Option<FontVec>,
) {
    // 局部换算小工具：全图坐标减去裁剪原点。
    let local = |x: f32, y: f32| (x - off_x, y - off_y);
    match shape {
        Shape::Pen { pts, color, width } => {
            let local_pts: Vec<(f32, f32)> = pts.iter().map(|(x, y)| local(*x, *y)).collect();
            stroke_polyline(img, &local_pts, *width, *color, clip);
        }
        Shape::Highlighter { pts, color, width } => {
            // 荧光笔：加粗 + 强制半透明，模拟记号笔覆盖效果。
            let translucent = AnnoColor(color.0, color.1, color.2, 110);
            let local_pts: Vec<(f32, f32)> = pts.iter().map(|(x, y)| local(*x, *y)).collect();
            stroke_polyline(img, &local_pts, width * 3.0, translucent, clip);
        }
        Shape::Rect { rect, color, width } => {
            let (x, y) = local(rect.x, rect.y);
            let local_rect = PixelRect::new(x, y, rect.w, rect.h);
            rect_border(img, local_rect, *width, *color, clip);
        }
        Shape::Ellipse { rect, color, width } => {
            let (x, y) = local(rect.x, rect.y);
            let local_rect = PixelRect::new(x, y, rect.w, rect.h);
            ellipse_border(img, local_rect, *width, *color, clip);
        }
        Shape::Arrow {
            from,
            to,
            color,
            width,
        } => {
            let a = local(from.0, from.1);
            let b = local(to.0, to.1);
            draw_arrow(img, a, b, *width, *color, clip);
        }
        Shape::Mosaic { rect } => {
            let (x, y) = local(rect.x, rect.y);
            pixelate(img, x, y, rect.w, rect.h, clip);
        }
        Shape::Text {
            pos,
            content,
            color,
            size,
        } => {
            // 无字体时跳过，保证“至少能导出底图 + 图形”。
            if let Some(font) = font {
                draw_text(img, font, local(pos.0, pos.1), content, *color, *size, clip);
            }
        }
    }
}

/// 带裁剪的 alpha 混合写像素（越界直接丢弃）。
fn blend_px(img: &mut RgbaImage, x: i32, y: i32, src: AnnoColor, clip: &PixelRect) {
    if x < clip.x as i32
        || y < clip.y as i32
        || x >= (clip.x + clip.w) as i32
        || y >= (clip.y + clip.h) as i32
    {
        return;
    }
    if x < 0 || y < 0 || x >= img.width() as i32 || y >= img.height() as i32 {
        return;
    }
    // 标准的 source-over 混合。
    let dst = img.get_pixel(x as u32, y as u32);
    let sa = src.3 as f32 / 255.0;
    let da = dst[3] as f32 / 255.0;
    let out_a = sa + da * (1.0 - sa);
    if out_a <= 0.0 {
        return;
    }
    let mix = |s: u8, d: u8| ((s as f32 * sa + d as f32 * da * (1.0 - sa)) / out_a) as u8;
    img.put_pixel(
        x as u32,
        y as u32,
        Rgba([
            mix(src.0, dst[0]),
            mix(src.1, dst[1]),
            mix(src.2, dst[2]),
            (out_a * 255.0) as u8,
        ]),
    );
}

/// 实心圆印章（粗线条的笔触基本单元）。
fn stamp(img: &mut RgbaImage, cx: f32, cy: f32, r: f32, color: AnnoColor, clip: &PixelRect) {
    // 半径小于 1 时退化为单像素，避免细线断裂。
    if r < 1.0 {
        blend_px(img, cx.round() as i32, cy.round() as i32, color, clip);
        return;
    }
    let r2 = r * r;
    let x0 = (cx - r).floor() as i32;
    let x1 = (cx + r).ceil() as i32;
    let y0 = (cy - r).floor() as i32;
    let y1 = (cy + r).ceil() as i32;
    for y in y0..=y1 {
        for x in x0..=x1 {
            let dx = x as f32 - cx;
            let dy = y as f32 - cy;
            if dx * dx + dy * dy <= r2 {
                blend_px(img, x, y, color, clip);
            }
        }
    }
}

/// 折线描边：按弧长均匀取点盖印章，保证拐弯不断线。
fn stroke_polyline(
    img: &mut RgbaImage,
    pts: &[(f32, f32)],
    width: f32,
    color: AnnoColor,
    clip: &PixelRect,
) {
    if pts.is_empty() {
        return;
    }
    if pts.len() == 1 {
        stamp(img, pts[0].0, pts[0].1, width / 2.0, color, clip);
        return;
    }
    for pair in pts.windows(2) {
        let (ax, ay) = pair[0];
        let (bx, by) = pair[1];
        let dist = ((bx - ax).powi(2) + (by - ay).powi(2)).sqrt();
        // 步数按半像素取整，细线不露缝，粗线不浪费。
        let steps = (dist * 2.0).ceil().max(1.0) as usize;
        for i in 0..=steps {
            let t = i as f32 / steps as f32;
            stamp(
                img,
                ax + (bx - ax) * t,
                ay + (by - ay) * t,
                width / 2.0,
                color,
                clip,
            );
        }
    }
}

/// 矩形边框：四条实心条带拼合。
fn rect_border(
    img: &mut RgbaImage,
    rect: PixelRect,
    width: f32,
    color: AnnoColor,
    clip: &PixelRect,
) {
    let (x, y, w, h) = (rect.x, rect.y, rect.w, rect.h);
    let t = width.max(1.0);
    fill_rect(img, x, y, w, t, color, clip);
    fill_rect(img, x, y + h - t, w, t, color, clip);
    fill_rect(img, x, y, t, h, color, clip);
    fill_rect(img, x + w - t, y, t, h, color, clip);
}

/// 实心矩形填充。
fn fill_rect(
    img: &mut RgbaImage,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    color: AnnoColor,
    clip: &PixelRect,
) {
    for yy in (y.floor() as i32)..((y + h).ceil() as i32) {
        for xx in (x.floor() as i32)..((x + w).ceil() as i32) {
            blend_px(img, xx, yy, color, clip);
        }
    }
}

/// 椭圆边框：参数方程取点盖印章。
fn ellipse_border(
    img: &mut RgbaImage,
    rect: PixelRect,
    width: f32,
    color: AnnoColor,
    clip: &PixelRect,
) {
    let (x, y, w, h) = (rect.x, rect.y, rect.w, rect.h);
    let (rx, ry) = (w / 2.0, h / 2.0);
    if rx <= 0.5 || ry <= 0.5 {
        return;
    }
    let (cx, cy) = (x + rx, y + ry);
    // 步数按周长估算，保证曲线光滑。
    let steps = ((rx + ry) * 3.0).ceil().max(24.0) as usize;
    for i in 0..=steps {
        let theta = i as f32 / steps as f32 * std::f32::consts::TAU;
        stamp(
            img,
            cx + rx * theta.cos(),
            cy + ry * theta.sin(),
            width / 2.0,
            color,
            clip,
        );
    }
}

/// 箭头：主干 + 两个 25 度夹角的头部。
fn draw_arrow(
    img: &mut RgbaImage,
    from: (f32, f32),
    to: (f32, f32),
    width: f32,
    color: AnnoColor,
    clip: &PixelRect,
) {
    stroke_polyline(img, &[from, to], width, color, clip);
    let dx = to.0 - from.0;
    let dy = to.1 - from.1;
    let len = (dx * dx + dy * dy).sqrt();
    if len < 2.0 {
        return;
    }
    // 头部长度随线宽放大，太短的箭头不画头。
    let head = (12.0 + width * 2.0).min(len);
    let angle = dy.atan2(dx);
    // 头部与主干夹角 25 度。
    let spread = 25.0_f32.to_radians();
    for sign in [-1.0, 1.0] {
        let a = angle + std::f32::consts::PI + sign * spread;
        let tip = (to.0 + head * a.cos(), to.1 + head * a.sin());
        stroke_polyline(img, &[to, tip], width, color, clip);
    }
}

/// 马赛克：分块取左上角像素平铺，块大小 12px（Snipaste 观感）。
fn pixelate(img: &mut RgbaImage, x: f32, y: f32, w: f32, h: f32, clip: &PixelRect) {
    const BLOCK: i32 = 12;
    let x0 = x.floor() as i32;
    let y0 = y.floor() as i32;
    let x1 = (x + w).ceil() as i32;
    let y1 = (y + h).ceil() as i32;
    let mut by = y0;
    while by < y1 {
        let mut bx = x0;
        while bx < x1 {
            // 取块左上角颜色（带裁剪检查，越界用黑代替）。
            let sample = if in_clip(bx, by, clip)
                && bx >= 0
                && by >= 0
                && bx < img.width() as i32
                && by < img.height() as i32
            {
                *img.get_pixel(bx as u32, by as u32)
            } else {
                Rgba([0, 0, 0, 255])
            };
            // 整块平铺（不透明，直接覆盖）。
            for yy in by..(by + BLOCK).min(y1) {
                for xx in bx..(bx + BLOCK).min(x1) {
                    if in_clip(xx, yy, clip)
                        && xx >= 0
                        && yy >= 0
                        && xx < img.width() as i32
                        && yy < img.height() as i32
                    {
                        img.put_pixel(xx as u32, yy as u32, sample);
                    }
                }
            }
            bx += BLOCK;
        }
        by += BLOCK;
    }
}

/// 点是否落在裁剪区内。
fn in_clip(x: i32, y: i32, clip: &PixelRect) -> bool {
    x >= clip.x as i32
        && y >= clip.y as i32
        && x < (clip.x + clip.w) as i32
        && y < (clip.y + clip.h) as i32
}

/// 单行文字绘制：逐字形取位图拼排（中文按字推进，无复杂塑形，截图标注够用）。
fn draw_text(
    img: &mut RgbaImage,
    font: &FontVec,
    pos: (f32, f32),
    content: &str,
    color: AnnoColor,
    size: f32,
    clip: &PixelRect,
) {
    let (x, y) = pos;
    let scale = PxScale::from(size.max(8.0));
    let scaled = font.as_scaled(scale);
    // 笔头从行首开始，基线 = 起始行 + 上伸部。
    let mut caret_x = x;
    let baseline = y + scaled.ascent();
    for ch in content.chars() {
        let glyph_id = scaled.glyph_id(ch);
        // 字形先定位再勾边，`OutlinedGlyph` 自带像素位图。
        let glyph = glyph_id.with_scale_and_position(scale, point(caret_x, baseline));
        if let Some(outlined) = scaled.outline_glyph(glyph) {
            let bounds = outlined.px_bounds();
            outlined.draw(|gx, gy, v| {
                if v <= 0.0 {
                    return;
                }
                let px = bounds.min.x as i32 + gx as i32;
                let py = bounds.min.y as i32 + gy as i32;
                // 字形覆盖度调制 alpha，边缘抗锯齿。
                let faded = AnnoColor(color.0, color.1, color.2, (color.3 as f32 * v) as u8);
                blend_px(img, px, py, faded, clip);
            });
        }
        // 无轮廓字形（如空格）只推进笔头。
        caret_x += scaled.h_advance(glyph_id);
    }
}

/// 读取图片文件为 RGBA（导出入口）。
pub fn load_rgba(path: &std::path::Path) -> Option<RgbaImage> {
    image::open(path).ok().map(|img| img.to_rgba8())
}

#[cfg(test)]
mod tests {
    //! 导出模块单测：用合成底图覆盖全部标注类型，保证栅格化不 panic。

    use super::*;

    /// 全类型标注导出冒烟测试（尺寸与 PNG 编码断言）。
    #[test]
    fn export_all_shapes() {
        // 合成底图 200x150，避免依赖外部截图文件。
        let base = RgbaImage::from_pixel(200, 150, Rgba([10, 20, 30, 255]));
        let sel = PixelRect::new(10.0, 10.0, 100.0, 80.0);
        let shapes = vec![
            Shape::Pen {
                pts: vec![(0.0, 0.0), (50.0, 50.0), (120.0, 30.0)],
                color: AnnoColor::RED,
                width: 3.0,
            },
            Shape::Highlighter {
                pts: vec![(20.0, 20.0), (90.0, 60.0)],
                color: AnnoColor::YELLOW,
                width: 4.0,
            },
            Shape::Rect {
                rect: PixelRect::new(15.0, 15.0, 60.0, 40.0),
                color: AnnoColor::GREEN,
                width: 2.0,
            },
            Shape::Ellipse {
                rect: PixelRect::new(30.0, 30.0, 50.0, 30.0),
                color: AnnoColor::BLUE,
                width: 2.0,
            },
            Shape::Arrow {
                from: (10.0, 70.0),
                to: (80.0, 20.0),
                color: AnnoColor::BLACK,
                width: 3.0,
            },
            Shape::Mosaic {
                rect: PixelRect::new(40.0, 40.0, 30.0, 20.0),
            },
            Shape::Text {
                pos: (20.0, 20.0),
                content: "拾景test".to_string(),
                color: AnnoColor::WHITE,
                size: 20.0,
            },
        ];
        // 有字体则连文字一起烧录，无字体也应正常导出底图 + 图形。
        let font = load_system_font();
        let out = render_export(&base, sel, &shapes, &font);
        assert_eq!(out.dimensions(), (100, 80));
        assert!(png_bytes(&out).is_some());
    }
}
