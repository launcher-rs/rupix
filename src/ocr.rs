//! OCR 模块：基于 ocr-rs（PP-OCRv5 mobile + MNN）的离线中英文识别。
//!
//! 三处来源（按优先级）：
//! 1. 内嵌：`--features bundled-ocr-models` 把默认三件套打进二进制（约 +21MB），
//!    开箱即用；默认关闭，外置优先。
//! 2. 手动指定：设置页 OCR 模型中逐个指定检测 / 识别 / 字典三个文件，
//!    文件名不做任何匹配，换模型直接选文件即可。
//! 3. 默认目录：在 [`candidate_dirs`] 列出的目录里找模型三件套
//!    （[`KNOWN_TRIOS`] 列出的家族按名识别，表外走通用兜底）。
//!
//! 缺失时调用方只管提示用户模型不存在，不 panic。
//!
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
/// 自有模型库地址（HF 全家族目录，设置页下载指引展示用）。
pub const HF_MODELS_URL: &str = "https://huggingface.co/launcher-rs/rupix-models";
/// 一键下载命令（仓库根下执行，设置页下载指引展示 / 复制用）。
pub const DOWNLOAD_CMD: &str =
    "powershell -ExecutionPolicy Bypass -File scripts/download-ocr-models.ps1";
/// 设置页手动指定的三件套文件名（存程序旁的 `rupix-ocrfiles.txt`，三行各一）。
const CUSTOM_FILES_FILE: &str = "rupix-ocrfiles.txt";

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
    // 小语种：与默认共用检测模型，识别模型与字典按语种区分
    // （自有 HF 仓库 `ocr/pp-ocrv5-<语种>/` 目录即按此结构存放）。
    (
        DET_MODEL,
        "arabic_PP-OCRv5_mobile_rec_infer.mnn",
        "ppocr_keys_arabic.txt",
        "PP-OCRv5 阿拉伯语",
    ),
    (
        DET_MODEL,
        "cyrillic_PP-OCRv5_mobile_rec_infer.mnn",
        "ppocr_keys_cyrillic.txt",
        "PP-OCRv5 西里尔语",
    ),
    (
        DET_MODEL,
        "devanagari_PP-OCRv5_mobile_rec_infer.mnn",
        "ppocr_keys_devanagari.txt",
        "PP-OCRv5 天城文",
    ),
    (
        DET_MODEL,
        "el_PP-OCRv5_mobile_rec_infer.mnn",
        "ppocr_keys_el.txt",
        "PP-OCRv5 希腊语",
    ),
    (
        DET_MODEL,
        "en_PP-OCRv5_mobile_rec_infer.mnn",
        "ppocr_keys_en.txt",
        "PP-OCRv5 英语",
    ),
    (
        DET_MODEL,
        "eslav_PP-OCRv5_mobile_rec_infer.mnn",
        "ppocr_keys_eslav.txt",
        "PP-OCRv5 东斯拉夫语",
    ),
    (
        DET_MODEL,
        "korean_PP-OCRv5_mobile_rec_infer.mnn",
        "ppocr_keys_korean.txt",
        "PP-OCRv5 韩语",
    ),
    (
        DET_MODEL,
        "latin_PP-OCRv5_mobile_rec_infer.mnn",
        "ppocr_keys_latin.txt",
        "PP-OCRv5 拉丁语",
    ),
    (
        DET_MODEL,
        "ta_PP-OCRv5_mobile_rec_infer.mnn",
        "ppocr_keys_ta.txt",
        "PP-OCRv5 泰米尔语",
    ),
    (
        DET_MODEL,
        "te_PP-OCRv5_mobile_rec_infer.mnn",
        "ppocr_keys_te.txt",
        "PP-OCRv5 泰卢固语",
    ),
    (
        DET_MODEL,
        "th_PP-OCRv5_mobile_rec_infer.mnn",
        "ppocr_keys_th.txt",
        "PP-OCRv5 泰语",
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

/// 外置模型候选目录（按优先级排序；手动指定的三个文件另走 [`custom_model_files`]，优先级更高）：
/// 1. `RUPIX_OCR_MODELS` 环境变量（显式指定即尊重，即使目录缺失也排第一，方便报错指路）；
/// 2. exe 旁 `models/`（默认位置：构建脚本同步开发期模型到输出目录旁，
///    发版时随安装包把 `models` 目录放 exe 旁即可）；
/// 3. 当前工作目录下 `models/`（解压即用形态）；
/// 4. 包根下 `assets/ocr-models/`（编译期绝对路径兜底：仓库内提交的默认模型）。
fn candidate_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    // 1. 环境变量优先，方便开发期切换模型版本；缺失也保留用于报错指路。
    if let Some(dir) = std::env::var_os(MODELS_ENV).map(PathBuf::from) {
        dirs.push(dir);
    }
    // 2. 发版形态：模型随安装包放在 exe 旁的 models 目录。
    if let Ok(exe) = std::env::current_exe()
        && let Some(parent) = exe.parent()
    {
        dirs.push(parent.join("models"));
    }
    // 3. 当前工作目录下的解压即用布局。
    if let Ok(cwd) = std::env::current_dir() {
        dirs.push(cwd.join("models"));
    }
    // 4. 包根兜底（仅调试构建）：仓库内提交的默认模型，方便 `cargo run` 开箱即用；
    // 发版不带编译期固定路径，二进制拷到哪都不回头找本机包根。
    #[cfg(debug_assertions)]
    dirs.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/ocr-models"));
    // 去重（exe 目录恰好等于 cwd 等情况），保持优先级顺序。
    let mut seen = std::collections::HashSet::new();
    dirs.retain(|dir| seen.insert(dir.clone()));
    dirs
}

/// 设置页手动指定的三件套文件（检测 / 识别 / 字典各一，存程序旁配置文件）。
///
/// 三行分别是检测 / 识别 / 字典的绝对路径；任一行缺失或文件不可用即视为未指定，
/// 回退自动查找。文件名不做任何匹配，叫什么都能用。
pub fn custom_model_files() -> Option<(PathBuf, PathBuf, PathBuf)> {
    // 配置与程序放在同一目录，重装也不会丢；文件被删则回退自动查找。
    let text = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(CUSTOM_FILES_FILE)))
        .and_then(|path| std::fs::read_to_string(path).ok())?;
    let mut lines = text.lines();
    let file = |line: Option<&str>| {
        line.map(str::trim)
            .filter(|text| !text.is_empty())
            .map(PathBuf::from)
            .filter(|path| path.is_file())
    };
    match (file(lines.next()), file(lines.next()), file(lines.next())) {
        (Some(det), Some(rec), Some(keys)) => Some((det, rec, keys)),
        _ => None,
    }
}

/// 持久化手动指定的三件套文件（None 表示该项未设置，写入失败仅忽略）。
pub fn store_custom_model_files(det: Option<&Path>, rec: Option<&Path>, keys: Option<&Path>) {
    // 优先放在程序所在目录，极端情况回退到系统临时目录。
    let path = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(CUSTOM_FILES_FILE)))
        .unwrap_or_else(|| std::env::temp_dir().join(CUSTOM_FILES_FILE));
    let line = |item: Option<&Path>| {
        item.map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    let text = format!("{}\n{}\n{}\n", line(det), line(rec), line(keys));
    let _ = std::fs::write(path, text);
}

/// 清除手动指定的三件套，恢复自动查找（文件缺失也视为成功）。
pub fn clear_custom_model_files() {
    // 按写入时的同优先级定位，删不到就当已清除。
    let path = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(CUSTOM_FILES_FILE)))
        .unwrap_or_else(|| std::env::temp_dir().join(CUSTOM_FILES_FILE));
    let _ = std::fs::remove_file(path);
}

/// 文件可用（存在且非空）：设置页选文件时过滤占位空文件用。
pub fn is_usable_file(path: &Path) -> bool {
    // 元数据读不到按不可用处理。
    path.is_file() && path.metadata().map(|meta| meta.len() > 0).unwrap_or(false)
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
    // 用模式匹配直接取出唯一元素：数量不是恰好各一则整体不认，不需要 unwrap。
    // 注意绑定的名字不能叫 keys：会把守卫里的 keys（待判空向量）遮蔽成 PathBuf。
    match (dets.pop(), recs.pop(), keys.pop()) {
        (Some(det), Some(rec), Some(key_file))
            if dets.is_empty() && recs.is_empty() && keys.is_empty() =>
        {
            Some(ResolvedModels {
                det,
                rec,
                keys: key_file,
                label: "自定义组合".to_string(),
                dir: dir.to_path_buf(),
            })
        }
        _ => None,
    }
}

/// 在候选位置找第一套可用的外置模型（文件 + 家族，无则返回 `None`）。
///
/// 手动指定的三个文件优先（文件名不做匹配，换模型不用搬文件），
/// 其次按目录优先级自动发现。
fn resolve_external() -> Option<ResolvedModels> {
    // 手动指定优先：三文件齐全即用，标签固定便于设置页展示。
    if let Some((det, rec, keys)) = custom_model_files() {
        return Some(ResolvedModels {
            dir: det
                .parent()
                .map(std::path::Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from(".")),
            det,
            rec,
            keys,
            label: "手动指定".to_string(),
        });
    }
    candidate_dirs()
        .into_iter()
        .filter(|dir| dir.is_dir())
        .find_map(|dir| find_trio(&dir))
}

/// 当前实际生效的模型标签：设置页展示用，无则返回 `None`。
pub fn active_models_info() -> Option<(String, PathBuf)> {
    resolve_external().map(|resolved| (resolved.label, resolved.dir))
}

/// 设置页展示的三文件（检测 / 识别 / 字典各一，缺位为 `None`）。
///
/// 手动指定齐全就用它；否则用当前生效三件套的对应文件；
/// 无生效模型就全空——默认文件夹本身不展示，只展示文件。
pub fn effective_model_files() -> (Option<PathBuf>, Option<PathBuf>, Option<PathBuf>) {
    // 手动指定优先：三文件齐全即整套采用。
    if let Some((det, rec, keys)) = custom_model_files() {
        return (Some(det), Some(rec), Some(keys));
    }
    // 其次用当前生效三件套（默认目录自动发现的结果）。
    if let Some(resolved) = resolve_external() {
        return (Some(resolved.det), Some(resolved.rec), Some(resolved.keys));
    }
    (None, None, None)
}

/// 是否内嵌构建（设置页展示用：无外置时也可用，不算缺模型）。
pub fn is_bundled() -> bool {
    cfg!(feature = "bundled-ocr-models")
}

/// 模型是否可用：内嵌特性开启时恒为真（内存字节兜底），否则看外置三件套。
pub fn models_available() -> bool {
    // 内嵌包自带模型，开箱即用，不再报“模型未找到”。
    if cfg!(feature = "bundled-ocr-models") {
        return true;
    }
    resolve_external().is_some()
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
        // 非内嵌包且外置全缺：只提醒模型不存在（去哪找模型见设置页）。
        let tried: Vec<String> = candidate_dirs()
            .iter()
            .map(|dir| dir.display().to_string())
            .collect();
        anyhow::bail!(
            "未找到 OCR 模型（已查找：{}）。请在设置 → OCR 模型中指定三个模型文件，或用 RUPIX_OCR_MODELS 环境变量指定目录",
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
        // 小语种：文件名全不一样，表内命中带出家族名。
        let latin = root.join("latin");
        touch(
            &latin,
            &[
                DET_MODEL,
                "latin_PP-OCRv5_mobile_rec_infer.mnn",
                "ppocr_keys_latin.txt",
            ],
        );
        assert_eq!(find_trio(&latin).unwrap().label, "PP-OCRv5 拉丁语");
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
