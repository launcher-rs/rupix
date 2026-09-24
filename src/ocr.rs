//! OCR 模块：基于 ocr-rs（PP-OCRv5 mobile + MNN）的离线中英文识别。
//!
//! 双模式（编译期 feature 切换）：
//! - 默认外置加载：在 [`candidate_dirs`] 列出的目录里找模型三件套，
//!   缺失时调用方应提示用户补齐，而不是在这里 panic；
//! - `--features bundled-ocr-models`：默认三件套用 `include_bytes!` 打进二进制，
//!   运行时零外部文件，开箱即用（约 +21MB 体积），适合发版安装包。
//!   即使开启内嵌，外置模型仍优先（方便不重编就换模型版本）。
//!
//! 多模型族：除默认 PP-OCRv5 mobile 外，[`KNOWN_TRIOS`] 列出的 ocr-rs 兼容家族
//! （v6 small/medium/tiny、v4、v5 fp16）均可直接使用；名字不在表里的三件套
//! 走通用兜底（目录内检测 / 识别 / 字典各恰好一个即认）。用户在设置页
//! （通用 → OCR 模型）指定目录即可切换，无需重编。
//! - 引擎全局懒加载一次，多线程共享（`OcrEngine: Send + Sync` 已由 example 验证）；
//! - [`recognize_image`] 是阻塞调用（单张大图约 0.6s），必须在后台线程跑，
//!   前台直接调会卡死界面（见 viewer 的识别按钮）；
//! - 结果先过 [`filter_lines`]（低置信度与图标误检过滤），坐标直接是图片像素，
//!   与 `shapes` 同一坐标系，可直接落成文字标注。

use std::path::{Path, PathBuf};
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
/// 默认模型文件名：检测模型（内嵌包与自动查找首选，见 [`KNOWN_TRIOS`] 首项）。
const DET_MODEL: &str = "PP-OCRv5_mobile_det.mnn";
/// 默认模型文件名：识别模型（同上）。
const REC_MODEL: &str = "PP-OCRv5_mobile_rec.mnn";
/// 默认模型文件名：字符字典（同上）。
const KEYS_FILE: &str = "ppocr_keys_v5.txt";
/// 环境变量覆盖模型目录（开发期切换模型版本用）。
const MODELS_ENV: &str = "RUPIX_OCR_MODELS";
/// 上游模型库地址（ocr-rs 官方 models 目录，设置页下载指引展示用）。
pub const MODELS_UPSTREAM_URL: &str = "https://github.com/zibo-chen/ocr-rs/tree/next/models";
/// 一键下载命令（仓库根下执行，设置页下载指引展示 / 复制用）。
pub const DOWNLOAD_CMD: &str =
    "powershell -ExecutionPolicy Bypass -File scripts/download-ocr-models.ps1";
/// 设置页自定义模型目录的配置文件名（与程序放同一目录，参考保存目录配置）。
const CUSTOM_DIR_FILE: &str = "rupix-ocrmodels.txt";

/// 已知模型三件套（检测 / 识别 / 字典文件名 + 家族标签），按优先级排序。
///
/// 首选 PP-OCRv5 mobile（默认，与内嵌包一致）；其余为 ocr-rs 2.4 验证过的家族，
/// 用户从上游 models 目录（见 `scripts/download-ocr-models.ps1`）下载任一套，
/// 在设置页指向该目录即可使用。
const KNOWN_TRIOS: &[(&str, &str, &str, &str)] = &[
    (DET_MODEL, REC_MODEL, KEYS_FILE, "PP-OCRv5 mobile"),
    (
        "PP-OCRv5_mobile_det_fp16.mnn",
        "PP-OCRv5_mobile_rec_fp16.mnn",
        "ppocr_keys_v5.txt",
        "PP-OCRv5 mobile fp16",
    ),
    (
        "PP-OCRv6_small_det.mnn",
        "PP-OCRv6_small_rec.mnn",
        "ppocr_keys_v6_small.txt",
        "PP-OCRv6 small",
    ),
    (
        "PP-OCRv6_medium_det.mnn",
        "PP-OCRv6_medium_rec.mnn",
        "ppocr_keys_v6_medium.txt",
        "PP-OCRv6 medium",
    ),
    (
        "PP-OCRv6_tiny_det.mnn",
        "PP-OCRv6_tiny_rec.mnn",
        "ppocr_keys_v6_tiny.txt",
        "PP-OCRv6 tiny",
    ),
    (
        "ch_PP-OCRv4_det_infer.mnn",
        "ch_PP-OCRv4_rec_infer.mnn",
        "ppocr_keys_v4.txt",
        "PP-OCRv4",
    ),
];

/// 解析出的可用模型：三个文件绝对路径 + 家族标签 + 所在目录。
struct ResolvedModels {
    /// 检测模型文件（绝对路径）。
    det: PathBuf,
    /// 识别模型文件（绝对路径）。
    rec: PathBuf,
    /// 字符字典文件（绝对路径）。
    keys: PathBuf,
    /// 家族标签（设置页展示用，如 `PP-OCRv6 small` / `自定义组合`）。
    label: String,
    /// 三件套所在目录。
    dir: PathBuf,
}

/// 内嵌模型字节：仅 `bundled-ocr-models` 特性开启时编译。
///
/// 三件套已进仓库（`assets/ocr-models/`）；更新版本才需跑
/// `scripts/download-ocr-models.ps1`，缺文件时 `include_bytes!` 直接编译失败
/// （这是有意的：发版包不允许静默缺模型）。
#[cfg(feature = "bundled-ocr-models")]
const BUNDLED_DET_MODEL: &[u8] = include_bytes!("../assets/ocr-models/PP-OCRv5_mobile_det.mnn");
/// 内嵌模型字节：识别模型（见 [`BUNDLED_DET_MODEL`] 的下载说明）。
#[cfg(feature = "bundled-ocr-models")]
const BUNDLED_REC_MODEL: &[u8] = include_bytes!("../assets/ocr-models/PP-OCRv5_mobile_rec.mnn");
/// 内嵌模型字节：字符字典（见 [`BUNDLED_DET_MODEL`] 的下载说明）。
#[cfg(feature = "bundled-ocr-models")]
const BUNDLED_KEYS_FILE: &[u8] = include_bytes!("../assets/ocr-models/ppocr_keys_v5.txt");

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

/// 外置模型候选目录（按优先级排序）：
///
/// 1. `RUPIX_OCR_MODELS` 环境变量（显式指定即尊重，即使目录缺失也排第一，方便报错指路）；
/// 2. 设置页自定义目录（持久化在程序旁）；
/// 3. exe 旁 `models/`（发版形态：安装包把三件套放在程序旁边）；
/// 4. 当前工作目录下 `models/`（解压即用形态）；
/// 5. 当前工作目录下 `temp/ocr-models/`（开发期相对路径，`cargo run` 时 cwd 即包根）；
/// 6. 包根下 `temp/ocr-models/` 与 `assets/ocr-models/`（编译期绝对路径兜底：
///    前者覆盖开发期下载，后者是仓库内提交的默认模型，新 clone 开箱即用）。
fn candidate_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    // 1. 环境变量优先，方便开发期切换模型版本；缺失也保留用于报错指路。
    if let Some(dir) = std::env::var_os(MODELS_ENV).map(PathBuf::from) {
        dirs.push(dir);
    }
    // 2. 设置页自定义次之：用户显式选择，优先于自动发现的位置。
    if let Some(dir) = custom_models_dir() {
        dirs.push(dir);
    }
    // 3. 发版形态：模型随安装包放在 exe 旁的 models 目录。
    if let Ok(exe) = std::env::current_exe()
        && let Some(parent) = exe.parent()
    {
        dirs.push(parent.join("models"));
    }
    // 4–5. 当前工作目录下的两种常见布局。
    if let Ok(cwd) = std::env::current_dir() {
        dirs.push(cwd.join("models"));
        dirs.push(cwd.join("temp").join("ocr-models"));
    }
    // 6. 编译期包根兜底：安装后从任意目录启动时相对路径会失效，
    // 用绝对路径保证开发期下载与仓库内置模型都能被找到。
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    dirs.push(manifest.join("temp/ocr-models"));
    dirs.push(manifest.join("assets/ocr-models"));
    // 去重（exe 目录恰好等于 cwd 等情况），保持优先级顺序。
    let mut seen = std::collections::HashSet::new();
    dirs.retain(|dir| seen.insert(dir.clone()));
    dirs
}

/// 设置页自定义的模型目录（持久化在程序旁的 `rupix-ocrmodels.txt`，参考保存目录配置）。
///
/// 返回 `None` 表示用户未自定义，走自动查找；目录被删后也视为未自定义。
pub fn custom_models_dir() -> Option<PathBuf> {
    // 配置与程序放在同一目录，重装也不会丢；目录被删则回退自动查找。
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(CUSTOM_DIR_FILE)))
        .and_then(|path| std::fs::read_to_string(path).ok())
        .map(|text| PathBuf::from(text.trim()))
        .filter(|dir| !dir.as_os_str().is_empty() && dir.is_dir())
}

/// 持久化设置页自定义的模型目录（写入失败仅忽略，不阻断流程）。
///
/// 调用方应先用 [`check_dir`] 确认三件套齐全再存，避免存入不可用目录。
pub fn store_custom_models_dir(dir: &Path) {
    // 优先放在程序所在目录，极端情况回退到系统临时目录。
    let path = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(CUSTOM_DIR_FILE)))
        .unwrap_or_else(|| std::env::temp_dir().join(CUSTOM_DIR_FILE));
    let _ = std::fs::write(path, dir.to_string_lossy().as_ref());
}

/// 清除自定义模型目录，恢复自动查找（文件缺失也视为成功）。
pub fn clear_custom_models_dir() {
    // 按写入时的同优先级定位，删不到就当已清除。
    let path = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(CUSTOM_DIR_FILE)))
        .unwrap_or_else(|| std::env::temp_dir().join(CUSTOM_DIR_FILE));
    let _ = std::fs::remove_file(path);
}

/// 在指定目录里找一套完整三件套：先按 [`KNOWN_TRIOS`] 匹配已知家族，
/// 名字不在表里的走通用兜底（检测 / 识别 / 字典各恰好一个即认，
/// 多个并存时视为歧义而不认，避免用错模型）。
fn find_trio(dir: &Path) -> Option<ResolvedModels> {
    // 已知家族按表内优先级匹配。
    for (det, rec, keys, label) in KNOWN_TRIOS {
        let (det, rec, keys) = (dir.join(det), dir.join(rec), dir.join(keys));
        if det.is_file() && rec.is_file() && keys.is_file() {
            return Some(ResolvedModels {
                det,
                rec,
                keys,
                label: label.to_string(),
                dir: dir.to_path_buf(),
            });
        }
    }
    // 通用兜底：文件名含 det / rec 的各一个 mnn，加含 key 的一个 txt。
    // 大小写不敏感（上游偶有大小写差异）；数量不是恰好各一就不认。
    let entries = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|entry| entry.ok())
        .collect::<Vec<_>>();
    let mut dets = Vec::new();
    let mut recs = Vec::new();
    let mut keys = Vec::new();
    for entry in &entries {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        // 非 UTF-8 文件名按空串处理，自然落选，不中断遍历。
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        if name.ends_with(".mnn") && name.contains("det") {
            dets.push(path);
        } else if name.ends_with(".mnn") && name.contains("rec") {
            recs.push(path);
        } else if name.ends_with(".txt") && name.contains("key") {
            keys.push(path);
        }
    }
    if dets.len() == 1 && recs.len() == 1 && keys.len() == 1 {
        return Some(ResolvedModels {
            det: dets.pop().expect("长度已确认为 1"),
            rec: recs.pop().expect("长度已确认为 1"),
            keys: keys.pop().expect("长度已确认为 1"),
            label: "自定义组合".to_string(),
            dir: dir.to_path_buf(),
        });
    }
    None
}

/// 校验用户选中的目录：完整返回家族标签，缺失返回人类可读的原因（设置页展示用）。
pub fn check_dir(dir: &Path) -> Result<String, String> {
    // 目录本身不可用时直接说明，不再往下找三件套。
    if !dir.is_dir() {
        return Err("目录不存在或不可访问".to_string());
    }
    match find_trio(dir) {
        Some(resolved) => Ok(resolved.label),
        None => Err("缺少完整三件套（检测 / 识别 / 字典各一）".to_string()),
    }
}

/// 在候选目录里找第一套可用的外置模型（目录 + 家族，无则返回 `None`）。
fn resolve_external() -> Option<ResolvedModels> {
    candidate_dirs()
        .into_iter()
        .filter(|dir| dir.is_dir())
        .find_map(|dir| find_trio(&dir))
}

/// 当前实际生效的模型（标签 + 目录）：设置页展示与诊断用，无则返回 `None`。
pub fn active_models_info() -> Option<(String, PathBuf)> {
    resolve_external().map(|resolved| (resolved.label, resolved.dir))
}

/// 模型目录解析结果：外置命中则返回该目录，否则返回首选 hint 目录。
///
/// 返回的目录不一定存在，调用方用 [`models_available`] 判断是否可用；
/// 内嵌特性开启时即使外置缺失也可识别（走内存字节），见 [`engine`]。
pub fn models_dir() -> PathBuf {
    // 外置命中直接用，保证文案与实际加载一致。
    if let Some(resolved) = resolve_external() {
        return resolved.dir;
    }
    // 外置全缺：返回第一候选做 hint（环境变量显式指定时优先指到它）。
    candidate_dirs()
        .into_iter()
        .next()
        // 理论上候选恒非空（至少有包根兜底），这里防呆。
        .unwrap_or_else(|| PathBuf::from("temp/ocr-models"))
}

/// 模型是否可用：内嵌特性开启时恒为真（内存字节兜底），否则看外置三件套。
pub fn models_available() -> bool {
    // 内嵌包自带模型，开箱即用，不再报“模型未找到”。
    if cfg!(feature = "bundled-ocr-models") {
        return true;
    }
    resolve_external().is_some()
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
    let config = ocr_rs::OcrEngineConfig::new().with_det_options(det_options());
    // 外置优先：环境变量 / 设置自定义 / sidecar 允许发版后换模型版本，无需重编。
    if let Some(resolved) = resolve_external() {
        // 未初始化则加载；`set` 竞败（多线程同时点识别）时用胜者实例。
        let fresh =
            ocr_rs::OcrEngine::new(&resolved.det, &resolved.rec, &resolved.keys, Some(config))
                .map_err(|error| {
                    anyhow::anyhow!("OCR 引擎加载失败（{}）：{error}", resolved.label)
                })?;
        let _ = ENGINE.set(fresh);
        return ENGINE
            .get()
            .ok_or_else(|| anyhow::anyhow!("OCR 引擎初始化失败"));
    }
    // 外置缺失时走内嵌字节（仅 bundled 特性编译，未开启则报错指路）。
    #[cfg(feature = "bundled-ocr-models")]
    {
        // 内存加载：`from_bytes` 与文件加载同精度，只是省掉落盘查找。
        let fresh = ocr_rs::OcrEngine::from_bytes(
            BUNDLED_DET_MODEL,
            BUNDLED_REC_MODEL,
            BUNDLED_KEYS_FILE,
            Some(config),
        )
        .map_err(|error| anyhow::anyhow!("OCR 内置模型加载失败：{error}"))?;
        let _ = ENGINE.set(fresh);
        return ENGINE
            .get()
            .ok_or_else(|| anyhow::anyhow!("OCR 引擎初始化失败"));
    }
    #[cfg(not(feature = "bundled-ocr-models"))]
    {
        // 非内嵌包且外置全缺：把查过的位置和补救命令一起告诉用户，
        // 新 clone 开箱即知去哪下载，而不是一句干巴巴的“模型未找到”。
        let tried: Vec<String> = candidate_dirs()
            .iter()
            .map(|dir| dir.display().to_string())
            .collect();
        anyhow::bail!(
            "未找到 OCR 模型（已查找：{}）。请在设置 → OCR 模型中选择目录或下载，或用 RUPIX_OCR_MODELS 环境变量指定；发版可用 --features bundled-ocr-models 内嵌模型",
            tried.join("；")
        );
    }
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
    //! OCR 模块单测：过滤规则、字号换算与三件套识别（纯函数 / 空文件，不加载模型）。

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

    /// 三件套识别：已知家族按名匹配，未知名字走通用兜底，缺件 / 歧义不认。
    ///
    /// 只建空文件（`find_trio` 仅判断存在性，不读模型内容，不触发推理）。
    #[test]
    fn find_trio_cases() {
        // 用例互不干扰：同一父目录下按用例分子目录，跑完整体删除。
        let root = std::env::temp_dir().join("rupix-ocr-trio-test");
        let _ = std::fs::remove_dir_all(&root);
        let touch = |dir: &std::path::Path, names: &[&str]| {
            std::fs::create_dir_all(dir).unwrap();
            for name in names {
                std::fs::write(dir.join(name), []).unwrap();
            }
        };
        // 已知家族：默认三件套命中首项。
        let known = root.join("known");
        touch(&known, &[DET_MODEL, REC_MODEL, KEYS_FILE]);
        assert_eq!(find_trio(&known).unwrap().label, "PP-OCRv5 mobile");
        // 通用兜底：表外名字但检测 / 识别 / 字典各一。
        let custom = root.join("custom");
        touch(&custom, &["foo_det.mnn", "foo_rec.mnn", "foo_keys.txt"]);
        assert_eq!(find_trio(&custom).unwrap().label, "自定义组合");
        // 缺字典：只有两件，不认。
        let partial = root.join("partial");
        touch(&partial, &["foo_det.mnn", "foo_rec.mnn"]);
        assert!(find_trio(&partial).is_none());
        // 歧义：两个检测模型并存，不认。
        let ambiguous = root.join("ambiguous");
        touch(
            &ambiguous,
            &["a_det.mnn", "b_det.mnn", "a_rec.mnn", "a_keys.txt"],
        );
        assert!(find_trio(&ambiguous).is_none());
        // 空目录：不认。
        let empty = root.join("empty");
        touch(&empty, &[]);
        assert!(find_trio(&empty).is_none());
        let _ = std::fs::remove_dir_all(&root);
    }
}
