//! OCR 模块：基于 ocr-rs（PP-OCRv5 mobile + MNN）的离线中英文识别。
//!
//! - 模型三件套（检测 / 识别 / 字典）在 [`models_dir`] 解析出的目录里找，
//!   缺失时调用方应提示用户补齐，而不是在这里 panic；
//! - 引擎全局懒加载一次，多线程共享（`OcrEngine: Send + Sync` 已由 example 验证）；
//! - [`recognize_image`] 是阻塞调用（单张大图约 0.6s），必须在后台线程跑，
//!   前台直接调会卡死界面（见 viewer 的识别按钮）；
//! - 结果先过 [`filter_lines`]（低置信度与图标误检过滤），坐标直接是图片像素，
//!   与 `shapes` 同一坐标系，可直接落成文字标注。

use std::path::PathBuf;
use std::sync::OnceLock;

use image::RgbaImage;

use crate::shapes::PixelRect;

/// 行保留阈值：spike 中正常行 89%+、误检 50–62%，取 0.65 留余量。
pub const MIN_LINE_CONFIDENCE: f32 = 0.65;
/// 单字行保留阈值：spike 里壁纸曲线曾检出 83% 的单个 `0`，单字误检常见，门槛从严。
pub const MIN_SINGLE_CHAR_CONFIDENCE: f32 = 0.9;
/// 落为标注时字号下限（图片像素）：框高过小多半是误检残留或装饰字，不放大展示。
pub const OCR_TEXT_SIZE_MIN: f32 = 12.0;
/// 落为标注时字号上限（图片像素）：整行大标题也钳制，避免一落盘就糊满屏。
pub const OCR_TEXT_SIZE_MAX: f32 = 96.0;
/// 模型文件名：检测模型。
const DET_MODEL: &str = "PP-OCRv5_mobile_det.mnn";
/// 模型文件名：识别模型。
const REC_MODEL: &str = "PP-OCRv5_mobile_rec.mnn";
/// 模型文件名：字符字典。
const KEYS_FILE: &str = "ppocr_keys_v5.txt";
/// 环境变量覆盖模型目录（开发期切换模型版本用）。
const MODELS_ENV: &str = "RUPIX_OCR_MODELS";

/// OCR 识别出的一行：文本 + 置信度 + 图片像素坐标框（可直接落成 `Shape::Text`）。
#[derive(Clone, Debug)]
pub struct OcrLine {
    /// 行文本（已 trim）。
    pub text: String,
    /// 行置信度（0–1）。
    pub confidence: f32,
    /// 行外包矩形（图片像素坐标，与底图同尺寸）。
    pub rect: PixelRect,
}

/// 模型目录解析优先级：环境变量 > exe 旁 sidecar（发版形态）> 开发期 `temp/ocr-models`。
///
/// 返回的目录不一定存在，调用方用 [`models_available`] 判断三件套是否齐全。
pub fn models_dir() -> PathBuf {
    // 环境变量优先，方便开发期切换模型版本。
    if let Some(dir) = std::env::var_os(MODELS_ENV).map(PathBuf::from)
        && dir.is_dir()
    {
        return dir;
    }
    // 发版形态：模型随安装包放在 exe 旁的 models 目录。
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        let sidecar = dir.join("models");
        if has_models(&sidecar) {
            return sidecar;
        }
    }
    // 开发期：包根下 git 忽略的 temp/ocr-models（cargo 运行工作目录即包根）。
    PathBuf::from("temp/ocr-models")
}

/// 指定目录里三件套是否齐全。
fn has_models(dir: &std::path::Path) -> bool {
    [DET_MODEL, REC_MODEL, KEYS_FILE]
        .iter()
        .all(|name| dir.join(name).is_file())
}

/// 三件套是否齐全（任一解析路径下齐全即可）。
pub fn models_available() -> bool {
    has_models(&models_dir())
}

/// 缺模型时提示用户补齐的目录（与 [`models_dir`] 同源，保证文案与实际查找一致）。
pub fn models_hint() -> PathBuf {
    models_dir()
}

/// 全局 OCR 引擎：首次调用时加载模型，之后常驻复用。
static ENGINE: OnceLock<ocr_rs::OcrEngine> = OnceLock::new();

/// 截图向检测参数：默认参数为通用文档调优，在密排小字截图上框偏大、易并行.
///
/// - 长边 1600（默认 960 会把大截图缩小，小字糊掉；单次识别可接受这点耗时）；
/// - 双阈值收紧（0.6 / 0.4），弱响应不参与成框，并行减少；
/// - `unclip_ratio` 1.5 → 1.2：Vatti 外扩是框偏大的主因；
/// - `box_border` 5 → 2：每框固定 padding，能省则省。
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

/// 获取全局引擎（首次初始化；模型缺失 / 加载失败返回错误，且失败不缓存，下次可重试）。
fn engine() -> anyhow::Result<&'static ocr_rs::OcrEngine> {
    // 缓存命中直接返回。
    if let Some(engine) = ENGINE.get() {
        return Ok(engine);
    }
    // 未初始化则加载；`set` 竞败（多线程同时点识别）时用胜者实例。
    let dir = models_dir();
    let config = ocr_rs::OcrEngineConfig::new().with_det_options(det_options());
    let fresh = ocr_rs::OcrEngine::new(
        dir.join(DET_MODEL),
        dir.join(REC_MODEL),
        dir.join(KEYS_FILE),
        Some(config),
    )
    .map_err(|error| anyhow::anyhow!("OCR 引擎加载失败：{error}"))?;
    let _ = ENGINE.set(fresh);
    ENGINE
        .get()
        .ok_or_else(|| anyhow::anyhow!("OCR 引擎初始化失败"))
}

/// 识别整张底图并过滤，返回可直接展示 / 落标注的行。
///
/// # 注意
/// 阻塞调用（单张大图约 0.6s），必须在后台线程执行，禁止在 UI 线程直接调用。
pub fn recognize_image(img: &RgbaImage) -> anyhow::Result<Vec<OcrLine>> {
    // ocr-rs 吃 DynamicImage；底图本就是 Rgba8，零拷贝包一层即可。
    let input = image::DynamicImage::ImageRgba8(img.clone());
    let engine = engine()?;
    let items = engine
        .recognize(&input)
        .map_err(|error| anyhow::anyhow!("OCR 识别失败：{error}"))?;
    // 检测框坐标系与输入图同尺寸（图片像素），直接映射，无需换算。
    let lines = items
        .into_iter()
        .map(|item| {
            let rect = &item.bbox.rect;
            OcrLine {
                text: item.text.trim().to_string(),
                confidence: item.confidence,
                rect: PixelRect::new(
                    (rect.left() as f32).max(0.0),
                    (rect.top() as f32).max(0.0),
                    rect.width() as f32,
                    rect.height() as f32,
                ),
            }
        })
        .collect();
    Ok(filter_lines(lines))
}

/// 过滤识别行：去空行、低置信度行、单字低置信行（图标 / 纹理误检多半落在这里）。
pub fn filter_lines(lines: Vec<OcrLine>) -> Vec<OcrLine> {
    lines
        .into_iter()
        .filter(|line| {
            // 空行（含纯空白）直接丢弃；这里再 trim 一次，不依赖上游是否洗过。
            if line.text.trim().is_empty() {
                return false;
            }
            // 单字行从严：壁纸曲线都能检出 83% 的单个 `0`。
            let threshold = if line.text.chars().count() <= 1 {
                MIN_SINGLE_CHAR_CONFIDENCE
            } else {
                MIN_LINE_CONFIDENCE
            };
            line.confidence >= threshold
        })
        .collect()
}

/// 行框高换算标注字号（图片像素，钳制到展示区间）。
pub fn line_text_size(rect_h: f32) -> f32 {
    rect_h.clamp(OCR_TEXT_SIZE_MIN, OCR_TEXT_SIZE_MAX)
}

/// 按检测框宽度拟合字号（照片式 overlay 原位对齐用）：先以框高试排量出宽度，
/// 再等比缩到框宽，**只降不升**（上限框高）。
///
/// 之前允许放大：窄行会被撑到两倍高，盖住下一行。实测框高本身已接近字号，
/// 封顶后各行尺寸基本一致，宽行只缩小不放大，行间不再打架；
/// 剩下的宽度差左对齐留白，高度差由紧贴内容的底色块吃掉。
pub fn fit_text_size(measured_w: f32, box_w: f32, box_h: f32) -> f32 {
    if measured_w <= 0.0 || box_w <= 0.0 {
        return line_text_size(box_h);
    }
    line_text_size((box_h * box_w / measured_w).min(box_h))
}

/// 多行拼成复制文本（换行分隔，空集合返回空串）。
pub fn lines_text(lines: &[OcrLine]) -> String {
    lines
        .iter()
        .map(|l| l.text.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    //! OCR 模块单测：过滤规则与字号换算（纯函数，不加载模型）。

    use super::*;

    /// 过滤：空行 / 低置信 / 单字低置信丢弃，正常行保留（阈值取自 spike 实测）。
    #[test]
    fn filter_lines_cases() {
        let line = |text: &str, confidence: f32| OcrLine {
            text: text.to_string(),
            confidence,
            rect: PixelRect::new(0.0, 0.0, 100.0, 20.0),
        };
        let out = filter_lines(vec![
            // 正常行保留。
            line("拾景", 0.95),
            line("托盘图标（正常）", 0.89),
            // 空行丢弃。
            line("   ", 0.99),
            // 低置信多字行丢弃。
            line("B~T|××", 0.62),
            // 单字 83% 照样丢弃（spike 里壁纸曲线的 `0`）。
            line("0", 0.83),
            // 单字高置信保留。
            line("C", 0.95),
        ]);
        let texts: Vec<_> = out.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(texts, vec!["拾景", "托盘图标（正常）", "C"]);
    }

    /// 字号换算：框高直接当字号，过大过小钳制。
    #[test]
    fn line_text_size_cases() {
        assert_eq!(line_text_size(28.0), 28.0);
        assert_eq!(line_text_size(4.0), OCR_TEXT_SIZE_MIN);
        assert_eq!(line_text_size(200.0), OCR_TEXT_SIZE_MAX);
    }

    /// 宽度拟合：只降不升（上限框高），无效输入回退框高。
    #[test]
    fn fit_text_size_cases() {
        // 框高 20 试排宽 200，框宽只有 100 → 缩到 10，钳制到下限 12。
        assert_eq!(fit_text_size(200.0, 100.0, 20.0), OCR_TEXT_SIZE_MIN);
        // 试排宽 50，框宽 100：按宽可放到 40，但封顶框高 20，保持一致不遮挡。
        assert_eq!(fit_text_size(50.0, 100.0, 20.0), 20.0);
        // 试排与框同宽 → 正好框高。
        assert_eq!(fit_text_size(100.0, 100.0, 20.0), 20.0);
        // 量不到宽 / 框宽无效 → 回退框高。
        assert_eq!(fit_text_size(0.0, 100.0, 20.0), 20.0);
        assert_eq!(fit_text_size(50.0, 0.0, 20.0), 20.0);
    }

    /// 文本拼接：换行分隔，原样保留行内容。
    #[test]
    fn lines_text_cases() {
        let line = |text: &str| OcrLine {
            text: text.to_string(),
            confidence: 0.99,
            rect: PixelRect::new(0.0, 0.0, 100.0, 20.0),
        };
        assert_eq!(lines_text(&[line("拾景"), line("Rupix")]), "拾景\nRupix");
        assert!(lines_text(&[]).is_empty());
    }
}
