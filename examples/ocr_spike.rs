//! OCR 接入 spike：验证 ocr-rs（PP-OCRv5 mobile + MNN）在本机的构建与识别效果。
//!
//! - 模型放在 git 忽略的 `temp/ocr-models/`（det / rec / 字典三件套）；
//! - 用法：`cargo run --example ocr_spike [图片路径...]`，默认跑 `temp/1.png` 与 `temp/22.png`；
//! - 输出：引擎加载耗时、冷 / 热推理耗时、每行文本 + 置信度 + 文本框，
//!   并用多线程并发识别验证 `OcrEngine` 是否 `Send + Sync`。
//! - 额外输出 `temp/ocr-overlay-<名>.png`：红框 = 检测框，白块 = 拟合文字块，
//!   肉眼判断行间是否打架（viewer overlay 的等比几何预览，不含遮罩与字体渲染）。

use std::sync::Arc;
use std::time::Instant;

use ab_glyph::{Font, FontVec, PxScale, ScaleFont as _};
use image::{Rgba, RgbaImage};

/// 模型目录（相对包根目录，`cargo run --example` 的工作目录即包根）。
const MODELS_DIR: &str = "temp/ocr-models";
/// spike 默认测试图（真实截图）。
const DEFAULT_IMAGES: &[&str] = &["temp/1.png", "temp/22.png"];

/// 截图向检测参数（与 `src/ocr.rs::det_options` 同步，spike 结果才有代表性）。
fn det_options() -> ocr_rs::DetOptions {
    ocr_rs::DetOptions {
        max_side_len: 1600,
        box_threshold: 0.6,
        score_threshold: 0.4,
        unclip_ratio: 1.2,
        box_border: 2,
        ..Default::default()
    }
}

/// 量一行文本在指定字号下的 advance 总宽（与 viewer 拟合侧同算法）。
fn measure_width(font: &FontVec, content: &str, size: f32) -> f32 {
    let scale = PxScale::from(size.max(8.0));
    let scaled = font.as_scaled(scale);
    content
        .chars()
        .map(|ch| scaled.h_advance(scaled.glyph_id(ch)))
        .sum()
}

/// 拟合字号（与 `src/ocr.rs::fit_text_size` 同步：只降不升，上限框高）。
fn fit_size(measured_w: f32, box_w: f32, box_h: f32) -> f32 {
    if measured_w <= 0.0 || box_w <= 0.0 {
        return box_h.clamp(12.0, 96.0);
    }
    (box_h * box_w / measured_w).min(box_h).clamp(12.0, 96.0)
}

/// 实心矩形（预览里的拟合文字块）。
fn fill_rect(img: &mut RgbaImage, x: f32, y: f32, w: f32, h: f32, color: Rgba<u8>) {
    let (w_img, h_img) = (img.width() as i32, img.height() as i32);
    for yy in (y.floor() as i32).max(0)..((y + h).ceil() as i32).min(h_img) {
        for xx in (x.floor() as i32).max(0)..((x + w).ceil() as i32).min(w_img) {
            img.put_pixel(xx as u32, yy as u32, color);
        }
    }
}

/// 空心矩形（预览里的检测框，线宽 2px）。
fn hollow_rect(img: &mut RgbaImage, x: f32, y: f32, w: f32, h: f32, color: Rgba<u8>) {
    fill_rect(img, x, y, w, 2.0, color);
    fill_rect(img, x, y + h - 2.0, w, 2.0, color);
    fill_rect(img, x, y, 2.0, h, color);
    fill_rect(img, x + w - 2.0, y, 2.0, h, color);
}

/// 写 overlay 几何预览：白块 = 拟合后文字块，红框 = 检测框。白块压住红框下沿 / 漫出行间即有问题。
fn write_overlay_preview(
    src_path: &str,
    items: &[ocr_rs::OcrResult_],
    font: &FontVec,
) -> anyhow::Result<()> {
    let mut img = image::open(src_path)?.into_rgba8();
    for item in items {
        let r = &item.bbox.rect;
        let (x, y, w, h) = (
            r.left() as f32,
            r.top() as f32,
            r.width() as f32,
            r.height() as f32,
        );
        let measured = measure_width(font, item.text.trim(), h);
        let size = fit_size(measured, w, h);
        let fitted_w = if h > 0.0 { measured * size / h } else { 0.0 };
        fill_rect(&mut img, x, y, fitted_w, size, Rgba([255, 255, 255, 255]));
        hollow_rect(&mut img, x, y, w, h, Rgba([255, 0, 0, 255]));
    }
    let name = std::path::Path::new(src_path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("out");
    let dst = format!("temp/ocr-overlay-{name}.png");
    img.save(&dst)?;
    println!("overlay 预览已写入 {dst}");
    Ok(())
}

/// 在工作线程里跑识别：能编译即证明 `OcrEngine: Send + Sync`。
fn recognize_on_threads(
    engine: Arc<ocr_rs::OcrEngine>,
    images: Vec<image::DynamicImage>,
) -> Vec<anyhow::Result<Vec<ocr_rs::OcrResult_>>> {
    // 每张图一个线程，共享同一个引擎实例。
    let handles: Vec<_> = images
        .into_iter()
        .map(|img| {
            let engine = engine.clone();
            std::thread::spawn(move || engine.recognize(&img).map_err(anyhow::Error::from))
        })
        .collect();
    handles
        .into_iter()
        .map(|h| h.join().expect("识别线程不应 panic"))
        .collect()
}

fn main() -> anyhow::Result<()> {
    // 命令行给图就用给的，否则用默认两张截图。
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args_or_default: Vec<String> = if args.is_empty() {
        DEFAULT_IMAGES.iter().map(|s| s.to_string()).collect()
    } else {
        args
    };

    // 引擎加载计时（包含 MNN 会话创建，正常只需一次）。
    let t0 = Instant::now();
    let config = ocr_rs::OcrEngineConfig::new().with_det_options(det_options());
    let engine = Arc::new(ocr_rs::OcrEngine::new(
        format!("{MODELS_DIR}/PP-OCRv5_mobile_det.mnn"),
        format!("{MODELS_DIR}/PP-OCRv5_mobile_rec.mnn"),
        format!("{MODELS_DIR}/ppocr_keys_v5.txt"),
        Some(config),
    )?);
    println!("引擎加载耗时：{:?}", t0.elapsed());

    // 图片解码（Rupix 底图本就是内存里的 RgbaImage，这里走文件仅为 spike 方便）。
    // temp 下的文件用户随时会覆盖 / 删除，缺失只警告跳过，不中断整轮。
    let mut images = Vec::new();
    let mut paths: Vec<String> = Vec::new();
    for p in &args_or_default {
        match image::open(p) {
            Err(error) => println!("跳过 {p}（打不开）：{error}"),
            Ok(img) => {
                println!("{p}：{}x{}", img.width(), img.height(),);
                paths.push(p.clone());
                images.push(img);
            }
        }
    }
    if images.is_empty() {
        println!("没有可用图片，先跑 `cargo run --example ocr_chart` 生成标尺图");
        return Ok(());
    }

    // 第一轮：冷推理（含模型预热）计时。
    let t0 = Instant::now();
    let cold = recognize_on_threads(engine.clone(), images.clone());
    let cold_elapsed = t0.elapsed();

    // 第二轮：热推理计时。
    let t0 = Instant::now();
    let warm = recognize_on_threads(engine.clone(), images);
    let warm_elapsed = t0.elapsed();

    // overlay 几何预览（只借 cold 结果，后面还要 move 进打印循环）。
    let font_data = std::fs::read("C:\\Windows\\Fonts\\msyh.ttc")?;
    let font = FontVec::try_from_vec_and_index(font_data, 0)?;
    for (path, result) in paths.iter().zip(cold.iter()) {
        if let Ok(items) = result {
            write_overlay_preview(path, items, &font)?;
        }
    }

    // 逐图打印识别结果（冷热两轮行数应一致）。
    for (round, elapsed, batch) in [("冷", cold_elapsed, cold), ("热", warm_elapsed, warm)] {
        println!(
            "── {round}推理总耗时（含 {n} 线程并发）：{elapsed:?} ──",
            n = paths.len()
        );
        for (path, result) in paths.iter().zip(batch) {
            let items = result?;
            println!("{path}：{n} 行", n = items.len());
            for item in &items {
                println!(
                    "  [{:.0}%] {} @({:.0},{:.0})",
                    item.confidence * 100.0,
                    item.text,
                    item.bbox.rect.left(),
                    item.bbox.rect.top(),
                );
            }
        }
    }
    Ok(())
}
