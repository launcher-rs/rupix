//! 国际化模块：内置中英双语，外部语言文件可扩展。
//!
//! - 默认中文，设置页可切换英文，切换即时生效（下次渲染起使用新文案）；
//! - 其他语言：在程序目录放 `lang-<code>.txt`（如 `lang-ja.txt`），
//!   内容为 `key=value`（`#` 开头为注释），缺的 key 自动回退英文内置；
//! - 调用方只用 `tr(key)` / `tr_fmt(key, args)`，不直接读写配置文件。

use std::{
    collections::HashMap,
    sync::{OnceLock, RwLock},
};

/// 语言配置文件名（与程序放同一目录，内容如 `en`，参考热键配置）。
const LANG_FILE: &str = "rupix-lang.txt";
/// 外部语言文件前缀（如 `lang-ja.txt`）。
const LANG_FILE_PREFIX: &str = "lang-";

/// 界面语言：中英内置，其他语言靠外部文件扩展。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Lang {
    /// 中文（默认）。
    Zh,
    /// 英文（内置）。
    En,
    /// 其他语言（外部 `lang-<code>.txt` 扩展，`code` 即文件名中段）。
    Other(String),
}

impl Lang {
    /// 语言代码（配置文件与外部文件命名用）。
    pub fn code(&self) -> String {
        match self {
            Lang::Zh => "zh".to_string(),
            Lang::En => "en".to_string(),
            Lang::Other(code) => code.clone(),
        }
    }

    /// 界面显示名（设置页语言卡片用）。
    pub fn display_name(&self) -> String {
        match self {
            Lang::Zh => "中文".to_string(),
            Lang::En => "English".to_string(),
            Lang::Other(code) => code.clone(),
        }
    }

    /// 从代码解析（未知代码视为外部扩展语言）。
    fn from_code(code: &str) -> Self {
        match code.trim().to_lowercase().as_str() {
            "" => Lang::Zh,
            "zh" | "cn" | "chinese" => Lang::Zh,
            "en" | "english" => Lang::En,
            other => Lang::Other(other.to_string()),
        }
    }
}

/// 全局语言状态（单例，启动时 `init` 一次，设置页切换时更新）。
struct LangState {
    /// 当前语言。
    lang: Lang,
    /// 外部语言文件的覆盖表（`key -> 文案`，优先于内置）。
    extra: HashMap<String, String>,
}

static STATE: OnceLock<RwLock<LangState>> = OnceLock::new();

/// 取全局状态（未初始化时按默认中文初始化，保证任何调用路径可用）。
fn state() -> &'static RwLock<LangState> {
    STATE.get_or_init(|| {
        RwLock::new(LangState {
            lang: Lang::Zh,
            extra: HashMap::new(),
        })
    })
}

/// 初始化语言（程序启动时调一次）：读配置 + 加载外部语言文件。
pub fn init() {
    let lang = load_lang();
    let extra = load_extra(&lang);
    if let Ok(mut state) = state().write() {
        state.lang = lang;
        state.extra = extra;
    }
}

/// 当前语言。
pub fn lang() -> Lang {
    state()
        .read()
        .map(|state| state.lang.clone())
        .unwrap_or(Lang::Zh)
}

/// 切换语言：持久化配置并重载外部覆盖表，调用方 `cx.notify()` 后即时生效。
///
/// # 参数
/// - `code`: `zh` / `en` / 其他外部语言代码。
pub fn set_lang(code: &str) {
    let lang = Lang::from_code(code);
    store_lang(&lang);
    let extra = load_extra(&lang);
    if let Ok(mut state) = state().write() {
        state.lang = lang;
        state.extra = extra;
    }
}

/// 可用语言列表：内置中英 + 程序目录扫描到的外部 `lang-*.txt`。
pub fn available_langs() -> Vec<Lang> {
    let mut langs = vec![Lang::Zh, Lang::En];
    // 扫描程序目录：lang-ja.txt 即扩展日语，无文件时只有中英。
    let dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.to_path_buf()));
    let Some(dir) = dir else {
        return langs;
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return langs;
    };
    let mut codes: Vec<String> = entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().to_string())
        .filter(|name| name.starts_with(LANG_FILE_PREFIX) && name.ends_with(".txt"))
        .filter_map(|name| {
            name.strip_prefix(LANG_FILE_PREFIX)?
                .strip_suffix(".txt")
                .map(|code| code.to_string())
        })
        .filter(|code| !code.is_empty() && code != "zh" && code != "en")
        .collect();
    codes.sort();
    codes.dedup();
    langs.extend(codes.into_iter().map(Lang::Other));
    langs
}

/// 取文案：外部覆盖优先，其次当前语言内置，最后回退中文内置，保底返回 key。
pub fn tr(key: &str) -> String {
    let state = state().read();
    let (lang, extra) = match state.as_ref() {
        Ok(state) => (state.lang.clone(), state.extra.get(key).cloned()),
        Err(_) => (Lang::Zh, None),
    };
    if let Some(text) = extra {
        return text;
    }
    builtin(&lang, key)
        .or_else(|| builtin(&Lang::Zh, key))
        .unwrap_or(key)
        .to_string()
}

/// 带 `{name}` 占位符的文案（`args` 为 `key -> 值`，如截图尺寸、文件名）。
pub fn tr_fmt(key: &str, args: &[(&str, &str)]) -> String {
    let mut text = tr(key);
    for (name, value) in args {
        text = text.replace(&format!("{{{name}}}"), value);
    }
    text
}

/// 读取语言配置；无配置或非法时默认中文。
fn load_lang() -> Lang {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(LANG_FILE)))
        .and_then(|path| std::fs::read_to_string(path).ok())
        .map(|text| Lang::from_code(text.trim()))
        .unwrap_or(Lang::Zh)
}

/// 持久化语言配置（写入失败仅忽略）。
fn store_lang(lang: &Lang) {
    let path = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(LANG_FILE)))
        .unwrap_or_else(|| std::env::temp_dir().join(LANG_FILE));
    let _ = std::fs::write(path, lang.code());
}

/// 加载外部语言覆盖表（中英内置不需要文件，其他语言缺文件则为空表）。
fn load_extra(lang: &Lang) -> HashMap<String, String> {
    let code = lang.code();
    if code == "zh" || code == "en" {
        return HashMap::new();
    }
    let path = std::env::current_exe().ok().and_then(|exe| {
        exe.parent()
            .map(|dir| dir.join(format!("{LANG_FILE_PREFIX}{code}.txt")))
    });
    let Some(path) = path else {
        return HashMap::new();
    };
    let Ok(text) = std::fs::read_to_string(path) else {
        return HashMap::new();
    };
    // 一行一个 `key=value`，`#` 开头与空行跳过，value 允许含 `=`。
    let mut map = HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            let key = key.trim();
            if !key.is_empty() {
                map.insert(key.to_string(), value.trim().to_string());
            }
        }
    }
    map
}

/// 内置文案表（中英；外部语言以英文为底 + 覆盖表补齐）。
fn builtin(lang: &Lang, key: &str) -> Option<&'static str> {
    let zh = matches!(lang, Lang::Zh);
    Some(match key {
        // —— 设置页 Tab ——
        "tab.capture" => {
            if zh {
                "截图"
            } else {
                "Capture"
            }
        }
        "tab.hotkey" => {
            if zh {
                "快捷键"
            } else {
                "Hotkeys"
            }
        }
        "tab.save" => {
            if zh {
                "保存"
            } else {
                "Save"
            }
        }
        "tab.general" => {
            if zh {
                "通用"
            } else {
                "General"
            }
        }
        // —— 设置窗口 ——
        "app.title" => {
            if zh {
                "Rupix · 拾景 设置"
            } else {
                "Rupix Settings"
            }
        }
        "app.subtitle" => {
            if zh {
                "关闭即隐藏到托盘 · 日常凭快捷键截图即可"
            } else {
                "Closing hides to tray · use hotkeys for daily capture"
            }
        }
        "hotkey.card_title" => {
            if zh {
                "{label}全局热键（点击框，按下组合键即自动生效）"
            } else {
                "{label} global hotkey (click the box, press combo to apply)"
            }
        }
        "hotkey.placeholder" => {
            if zh {
                "点这里，然后按下快捷键"
            } else {
                "Click here, then press the hotkey"
            }
        }
        "hotkey.recording" => {
            if zh {
                "正在录制…"
            } else {
                "Recording…"
            }
        }
        "hotkey.current" => {
            if zh {
                "当前：{current}（失焦可用，托盘常驻）"
            } else {
                "Current: {current} (works unfocused, tray resident)"
            }
        }
        "hotkey.applied" => {
            if zh {
                "{label}全局截图快捷键：{text}"
            } else {
                "{label} global hotkey: {text}"
            }
        }
        "hotkey.cleared" => {
            if zh {
                "已清空录制，旧快捷键继续生效"
            } else {
                "Recording cleared, previous hotkey still active"
            }
        }
        "hotkey.region" => {
            if zh {
                "区域截图"
            } else {
                "Region capture"
            }
        }
        "hotkey.full" => {
            if zh {
                "全屏截图"
            } else {
                "Fullscreen capture"
            }
        }
        "hotkey.picker" => {
            if zh {
                "屏幕取色"
            } else {
                "Color picker"
            }
        }
        "hotkey.parse_fail" => {
            if zh {
                "快捷键解析失败（{hotkey}）：{error}"
            } else {
                "Hotkey parse failed ({hotkey}): {error}"
            }
        }
        "hotkey.register_fail" => {
            if zh {
                "{label}全局快捷键注册失败（{hotkey}）：{error}"
            } else {
                "{label} hotkey registration failed ({hotkey}): {error}"
            }
        }
        // —— 保存目录卡 ——
        "savedir.title" => {
            if zh {
                "默认保存路径（截图自动保存原图到此）"
            } else {
                "Default save path (captures auto-save here)"
            }
        }
        "savedir.pick" => {
            if zh {
                "选择目录…"
            } else {
                "Choose folder…"
            }
        }
        "savedir.reset" => {
            if zh {
                "恢复默认"
            } else {
                "Reset"
            }
        }
        "savedir.prompt" => {
            if zh {
                "选择截图默认保存目录"
            } else {
                "Choose default capture folder"
            }
        }
        "savedir.updated" => {
            if zh {
                "默认保存目录已更新：{dir}"
            } else {
                "Default save folder updated: {dir}"
            }
        }
        "savedir.invalid" => {
            if zh {
                "所选目录无效，未更改保存路径"
            } else {
                "Invalid folder, save path unchanged"
            }
        }
        "savedir.restored" => {
            if zh {
                "已恢复默认保存目录：{dir}"
            } else {
                "Default save folder restored: {dir}"
            }
        }
        // —— 语言卡 ——
        "lang.title" => {
            if zh {
                "界面语言（切换即时生效）"
            } else {
                "Language (applies instantly)"
            }
        }
        "lang.note" => {
            if zh {
                "扩展新语言：在程序目录放 lang-<代码>.txt（key=value，每行一条）即可出现在列表中"
            } else {
                "Add a language: drop lang-<code>.txt (key=value per line) next to the exe to list it here"
            }
        }
        "lang.switched" => {
            if zh {
                "已切换为{lang}"
            } else {
                "Switched to {lang}"
            }
        }
        // —— OCR 模型卡（通用 Tab） ——
        "ocr.title" => {
            if zh {
                "OCR 模型（离线识别底图文字）"
            } else {
                "OCR models (offline text recognition)"
            }
        }
        "ocr.active" => {
            if zh {
                "当前：{label}"
            } else {
                "Active: {label}"
            }
        }
        "ocr.missing" => {
            if zh {
                "未找到可用模型，请选择目录或下载后重试"
            } else {
                "No usable models found, choose a folder or download them"
            }
        }
        "ocr.custom" => {
            if zh {
                "自定义：{dir}"
            } else {
                "Custom: {dir}"
            }
        }
        "ocr.auto" => {
            if zh {
                "未自定义，跟随自动查找"
            } else {
                "Not customized, auto-detect"
            }
        }
        "ocr.pick" => {
            if zh {
                "选择目录…"
            } else {
                "Choose folder…"
            }
        }
        "ocr.reset" => {
            if zh {
                "恢复默认"
            } else {
                "Reset"
            }
        }
        "ocr.pick_prompt" => {
            if zh {
                "选择 OCR 模型目录（三件套：检测 / 识别 / 字典）"
            } else {
                "Choose OCR model folder (trio: det / rec / keys)"
            }
        }
        "ocr.saved" => {
            if zh {
                "OCR 模型已切换：{label}"
            } else {
                "OCR models switched: {label}"
            }
        }
        "ocr.restored" => {
            if zh {
                "OCR 模型已恢复自动查找"
            } else {
                "OCR models reset to auto-detect"
            }
        }
        "ocr.incomplete" => {
            if zh {
                "目录不可用，未更改：{reason}"
            } else {
                "Folder unusable, unchanged: {reason}"
            }
        }
        "ocr.note" => {
            if zh {
                "支持 PP-OCRv5/v6 等 ocr-rs 兼容三件套；内嵌包开箱即用，外置目录优先"
            } else {
                "Supports PP-OCRv5/v6 trios compatible with ocr-rs; bundled works out of the box, external folders win"
            }
        }
        // —— 截图入口与提示 ——
        "btn.region" => {
            if zh {
                "区域截图"
            } else {
                "Region"
            }
        }
        "btn.full" => {
            if zh {
                "全屏截图"
            } else {
                "Fullscreen"
            }
        }
        "btn.picker" => {
            if zh {
                "屏幕取色"
            } else {
                "Picker"
            }
        }
        "btn.open_image" => {
            if zh {
                "打开图片编辑…"
            } else {
                "Edit image…"
            }
        }
        "image.pick_prompt" => {
            if zh {
                "选择要编辑的图片"
            } else {
                "Choose an image to edit"
            }
        }
        "image.invalid" => {
            if zh {
                "所选文件不是有效图片"
            } else {
                "Selected file is not a valid image"
            }
        }
        "app.tips" => {
            if zh {
                "区域截图：在全屏遮罩上拖拽框选（悬停窗口高亮时点击直接截该窗口），松手后进独立预览窗口（可缩放/复制/保存，可底部状态栏查看结果），右键/Esc 取消。取色：在冻结帧上移动取色，点击复制 HEX。本窗口关闭即隐藏到托盘。"
            } else {
                "Region capture: drag on the fullscreen mask (click a highlighted window to capture it), then annotate in the preview window (zoom/copy/save, results in its status bar), right-click/Esc cancels. Picker: move on the frozen frame, click copies HEX. Closing this window hides to tray."
            }
        }
        "app.status" => {
            if zh {
                "状态：{status}"
            } else {
                "Status: {status}"
            }
        }
        "app.recents" => {
            if zh {
                "最近保存（点击打开）"
            } else {
                "Recent saves (click to open)"
            }
        }
        "app.boot_ready" => {
            if zh {
                "就绪：区域 {region}，全屏 {full}，取色 {picker}（窗口已隐藏到托盘）"
            } else {
                "Ready: region {region}, fullscreen {full}, picker {picker} (window hidden to tray)"
            }
        }
        "app.window_title" => {
            if zh {
                "Rupix 设置"
            } else {
                "Rupix Settings"
            }
        }
        // —— 托盘 ——
        "tray.tooltip" => {
            if zh {
                "Rupix 拾景"
            } else {
                "Rupix"
            }
        }
        "tray.region" => {
            if zh {
                "区域截图"
            } else {
                "Region capture"
            }
        }
        "tray.full" => {
            if zh {
                "全屏截图"
            } else {
                "Fullscreen capture"
            }
        }
        "tray.picker" => {
            if zh {
                "屏幕取色"
            } else {
                "Color picker"
            }
        }
        "tray.open_image" => {
            if zh {
                "打开图片编辑…"
            } else {
                "Edit image…"
            }
        }
        "tray.show" => {
            if zh {
                "打开设置"
            } else {
                "Settings"
            }
        }
        "tray.quit" => {
            if zh {
                "退出"
            } else {
                "Quit"
            }
        }
        // —— 截图流程状态 ——
        "cap.capturing" => {
            if zh {
                "正在截取屏幕…"
            } else {
                "Capturing screen…"
            }
        }
        "cap.full_done" => {
            if zh {
                "已截取全屏 {w} × {h}（{note}），见预览窗口"
            } else {
                "Fullscreen captured {w} × {h} ({note}), see preview window"
            }
        }
        "cap.region_hint" => {
            if zh {
                "已截取，在遮罩上拖拽框选…"
            } else {
                "Captured, drag on the mask to select…"
            }
        }
        "cap.auto_saved" => {
            if zh {
                "原图已自动保存"
            } else {
                "original auto-saved"
            }
        }
        "cap.auto_fail" => {
            if zh {
                "自动保存失败，仅预览"
            } else {
                "auto-save failed, preview only"
            }
        }
        "cap.no_source" => {
            if zh {
                "未找到可捕获的屏幕源"
            } else {
                "No capturable screen source"
            }
        }
        "cap.stream_fail" => {
            if zh {
                "启动屏幕捕获失败"
            } else {
                "Failed to start screen capture"
            }
        }
        "cap.timeout" => {
            if zh {
                "截取超时，请重试"
            } else {
                "Capture timed out, please retry"
            }
        }
        "cap.picking" => {
            if zh {
                "正在取色…"
            } else {
                "Picking color…"
            }
        }
        "cap.picker_hint" => {
            if zh {
                "移动鼠标取色，点击复制，右键 / Esc 取消"
            } else {
                "Move to pick, click copies, right-click / Esc cancels"
            }
        }
        "cap.cropped" => {
            if zh {
                "已截取 {w} × {h}（{note}），见预览窗口"
            } else {
                "Captured {w} × {h} ({note}), see preview window"
            }
        }
        "cap.crop_fail" => {
            if zh {
                "裁剪失败：{error}"
            } else {
                "Crop failed: {error}"
            }
        }
        "cap.cancelled" => {
            if zh {
                "已取消"
            } else {
                "Cancelled"
            }
        }
        // —— 浮层 ——
        "overlay.hint" => {
            if zh {
                "拖拽选区 / 点击窗口截取；右键 / Esc 取消"
            } else {
                "Drag to select / click window to capture; right-click / Esc cancels"
            }
        }
        "overlay.badge_click" => {
            if zh {
                "{title}  {w} × {h} · 点击截取"
            } else {
                "{title}  {w} × {h} · click to capture"
            }
        }
        // —— 取色器 ——
        "picker.copied" => {
            if zh {
                "已复制颜色 {hex}"
            } else {
                "Color copied {hex}"
            }
        }
        "picker.fail" => {
            if zh {
                "取色失败：图片不可读"
            } else {
                "Pick failed: image unreadable"
            }
        }
        "picker.move_hint" => {
            if zh {
                "移动鼠标取色"
            } else {
                "Move to pick color"
            }
        }
        "picker.click_hint" => {
            if zh {
                "点击复制 · 右键 / Esc 取消"
            } else {
                "Click copies · right-click / Esc cancels"
            }
        }
        // —— 预览窗口标题 ——
        "viewer.title" => {
            if zh {
                "Rupix 预览（{w} × {h}）"
            } else {
                "Rupix Preview ({w} × {h})"
            }
        }
        // —— 预览操作排 ——
        "viewer.fit" => {
            if zh {
                "适应"
            } else {
                "Fit"
            }
        }
        "viewer.undo" => {
            if zh {
                "撤销"
            } else {
                "Undo"
            }
        }
        "viewer.redo" => {
            if zh {
                "重做"
            } else {
                "Redo"
            }
        }
        "viewer.line_width" => {
            if zh {
                "线宽"
            } else {
                "Width"
            }
        }
        "viewer.text_bg" => {
            if zh {
                "底色"
            } else {
                "Bg"
            }
        }
        "viewer.text_bg_latest" => {
            if zh {
                "已切换最近一条文字底色（新建沿用）"
            } else {
                "Latest text bg toggled (also default for new)"
            }
        }
        "viewer.text_bg_default" => {
            if zh {
                "已切换新建文字底色"
            } else {
                "Default bg for new text toggled"
            }
        }
        "viewer.bg_color_applied" => {
            if zh {
                "已设置底色（最近一条+新建沿用）"
            } else {
                "Bg color set (latest + new)"
            }
        }
        "viewer.picker_fg" => {
            if zh {
                "取色：字色"
            } else {
                "Picker: text"
            }
        }
        "viewer.picker_bg" => {
            if zh {
                "取色：底色"
            } else {
                "Picker: background"
            }
        }
        "viewer.ocr" => {
            if zh {
                "识别"
            } else {
                "OCR"
            }
        }
        "viewer.ocr_running" => {
            if zh {
                "识别中…"
            } else {
                "Recognizing…"
            }
        }
        "viewer.ocr_busy" => {
            if zh {
                "识别中，请稍候"
            } else {
                "Recognition running"
            }
        }
        "viewer.ocr_done" => {
            if zh {
                "识别到 {n} 行，已复制文本"
            } else {
                "{n} lines recognized, text copied"
            }
        }
        "viewer.ocr_empty" => {
            if zh {
                "未识别到文字"
            } else {
                "No text found"
            }
        }
        "viewer.ocr_no_models" => {
            if zh {
                "未找到 OCR 模型，请在设置 → 通用 → OCR 模型中选择目录（期望：{dir}）"
            } else {
                "OCR models missing, choose a folder in Settings → General → OCR models (want: {dir})"
            }
        }
        "viewer.ocr_fail" => {
            if zh {
                "识别失败：{error}"
            } else {
                "Recognition failed: {error}"
            }
        }
        "viewer.ocr_panel_title" => {
            if zh {
                "识别结果（{n} 行）"
            } else {
                "OCR Results ({n} lines)"
            }
        }
        "viewer.ocr_copy_all" => {
            if zh {
                "复制全部"
            } else {
                "Copy all"
            }
        }
        "viewer.ocr_apply" => {
            if zh {
                "落为标注"
            } else {
                "Add as notes"
            }
        }
        "viewer.ocr_copied" => {
            if zh {
                "已复制 {n} 行"
            } else {
                "Copied {n} lines"
            }
        }
        "viewer.ocr_copy_one" => {
            if zh {
                "已复制：{text}"
            } else {
                "Copied: {text}"
            }
        }
        "viewer.ocr_applied" => {
            if zh {
                "已落为 {n} 条标注"
            } else {
                "Added {n} notes"
            }
        }
        "viewer.copy" => {
            if zh {
                "复制"
            } else {
                "Copy"
            }
        }
        "viewer.save" => {
            if zh {
                "保存"
            } else {
                "Save"
            }
        }
        "viewer.op_hint" => {
            if zh {
                "V/1-8切换 · 空格平移 · Ctrl+滚轮缩放 · Esc关闭"
            } else {
                "V/1-8 tools · Space pan · Ctrl+wheel zoom · Esc closes"
            }
        }
        // —— 预览标注排 ——
        "tool.select" => {
            if zh {
                "选择"
            } else {
                "Select"
            }
        }
        "tool.rect" => {
            if zh {
                "矩形"
            } else {
                "Rect"
            }
        }
        "tool.ellipse" => {
            if zh {
                "椭圆"
            } else {
                "Ellipse"
            }
        }
        "tool.arrow" => {
            if zh {
                "箭头"
            } else {
                "Arrow"
            }
        }
        "tool.pen" => {
            if zh {
                "画笔"
            } else {
                "Pen"
            }
        }
        "tool.highlighter" => {
            if zh {
                "荧光笔"
            } else {
                "Marker"
            }
        }
        "tool.eraser" => {
            if zh {
                "橡皮擦"
            } else {
                "Eraser"
            }
        }
        "tool.mosaic" => {
            if zh {
                "马赛克"
            } else {
                "Mosaic"
            }
        }
        "tool.text" => {
            if zh {
                "文字"
            } else {
                "Text"
            }
        }
        "tool.picker" => {
            if zh {
                "取色"
            } else {
                "Color"
            }
        }
        "viewer.text_placeholder" => {
            if zh {
                "输入文字"
            } else {
                "Type text"
            }
        }
        "viewer.text_confirm" => {
            if zh {
                "回车确认 · Esc 取消"
            } else {
                "Enter confirms · Esc cancels"
            }
        }
        "viewer.text_new" => {
            if zh {
                "点击新建 · Ctrl+拖动移动 · 点击已有文字编辑"
            } else {
                "Click to add · Ctrl+drag moves · click text to edit"
            }
        }
        "viewer.select_hint" => {
            if zh {
                "拖动标注移动 · 空白处拖动画布"
            } else {
                "Drag shapes to move · drag empty area to pan"
            }
        }
        "viewer.eraser_hint" => {
            if zh {
                "点击 / 拖动删除命中的标注"
            } else {
                "Click / drag to erase hit shapes"
            }
        }
        "viewer.marker_hint" => {
            if zh {
                "拖动涂抹高亮（半透明粗线）"
            } else {
                "Drag to highlight (translucent)"
            }
        }
        "viewer.place_hint" => {
            if zh {
                "选择工具后点击图片放置"
            } else {
                "Pick a tool, then click the image"
            }
        }
        // —— 关闭确认 ——
        "confirm.title" => {
            if zh {
                "有未保存的标注修改，关闭将丢失，是否先保存？"
            } else {
                "Unsaved annotations will be lost. Save first?"
            }
        }
        "confirm.save" => {
            if zh {
                "保存并关闭"
            } else {
                "Save & close"
            }
        }
        "confirm.discard" => {
            if zh {
                "直接关闭"
            } else {
                "Close w/o saving"
            }
        }
        "confirm.cancel" => {
            if zh {
                "取消"
            } else {
                "Cancel"
            }
        }
        // —— 预览状态栏 ——
        "viewer.info" => {
            if zh {
                "{w} × {h} · {zoom}% · {n} 个标注"
            } else {
                "{w} × {h} · {zoom}% · {n} annotations"
            }
        }
        "viewer.unsaved" => {
            if zh {
                " · 未保存"
            } else {
                " · unsaved"
            }
        }
        "viewer.open_file" => {
            if zh {
                "打开文件"
            } else {
                "Open file"
            }
        }
        "viewer.open_dir" => {
            if zh {
                "打开目录"
            } else {
                "Open folder"
            }
        }
        "viewer.ready" => {
            if zh {
                "就绪"
            } else {
                "Ready"
            }
        }
        "viewer.copy_ok" => {
            if zh {
                "已复制：在 Word/画图中粘贴为图片，在文件夹中粘贴为文件"
            } else {
                "Copied: paste as image in Word/Paint, as file in folders"
            }
        }
        "viewer.copy_img_only" => {
            if zh {
                "已复制为图片；文件粘贴不可用：{error}"
            } else {
                "Copied as image; file paste unavailable: {error}"
            }
        }
        "viewer.copy_fail" => {
            if zh {
                "复制失败：{error}"
            } else {
                "Copy failed: {error}"
            }
        }
        "viewer.copy_read_fail" => {
            if zh {
                "复制失败：读取图片失败：{error}"
            } else {
                "Copy failed: cannot read image: {error}"
            }
        }
        "viewer.export_fail" => {
            if zh {
                "复制失败：导出图片失败"
            } else {
                "Copy failed: export failed"
            }
        }
        "viewer.cache_dir_fail" => {
            if zh {
                "复制失败：创建缓存目录失败：{error}"
            } else {
                "Copy failed: cannot create cache dir: {error}"
            }
        }
        "viewer.cache_write_fail" => {
            if zh {
                "复制失败：写入剪贴板文件失败：{error}"
            } else {
                "Copy failed: cannot write clipboard file: {error}"
            }
        }
        "viewer.saved" => {
            if zh {
                "已保存 {name}（{loc}）"
            } else {
                "Saved {name} ({loc})"
            }
        }
        "viewer.saved_full" => {
            if zh {
                "已保存 {path}"
            } else {
                "Saved {path}"
            }
        }
        "viewer.save_fail" => {
            if zh {
                "保存失败"
            } else {
                "Save failed"
            }
        }
        "viewer.image_fallback" => {
            if zh {
                "图片"
            } else {
                "image"
            }
        }
        // —— 导出错误（状态栏透出） ——
        "export.empty_image" => {
            if zh {
                "图片数据为空"
            } else {
                "Image data is empty"
            }
        }
        "export.no_clip_file" => {
            if zh {
                "剪贴板文件不存在"
            } else {
                "Clipboard file missing"
            }
        }
        "export.hdrop_fail" => {
            if zh {
                "拼接文件剪贴板数据失败"
            } else {
                "Failed to build file clipboard data"
            }
        }
        "export.empty_clip" => {
            if zh {
                "剪贴板数据为空"
            } else {
                "Clipboard data is empty"
            }
        }
        "export.alloc_fail" => {
            if zh {
                "分配剪贴板内存失败：{e}"
            } else {
                "Failed to allocate clipboard memory: {e}"
            }
        }
        "export.lock_fail" => {
            if zh {
                "锁定剪贴板内存失败"
            } else {
                "Failed to lock clipboard memory"
            }
        }
        "export.write_fail" => {
            if zh {
                "写入剪贴板失败：{e}"
            } else {
                "Failed to write clipboard: {e}"
            }
        }
        "export.open_fail" => {
            if zh {
                "打开系统剪贴板失败"
            } else {
                "Failed to open system clipboard"
            }
        }
        "export.empty_fail" => {
            if zh {
                "清空剪贴板失败：{e}"
            } else {
                "Failed to empty clipboard: {e}"
            }
        }
        "export.reg_fail" => {
            if zh {
                "注册 PNG 剪贴板格式失败"
            } else {
                "Failed to register PNG clipboard format"
            }
        }
        "export.unsupported" => {
            if zh {
                "当前平台暂不支持复制为文件"
            } else {
                "Copy-as-file not supported on this platform"
            }
        }
        "export.sys_fail" => {
            if zh {
                "调用系统剪贴板失败：{err}"
            } else {
                "System clipboard call failed: {err}"
            }
        }
        "export.file_fail" => {
            if zh {
                "写入文件剪贴板失败"
            } else {
                "Failed to write file clipboard"
            }
        }
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    //! 国际化单测：占位符替换与回退逻辑（纯函数，不碰文件）。

    use super::*;

    /// 占位符按名替换，未知 key 保底返回 key 本身。
    #[test]
    fn tr_fmt_replaces_args() {
        let text = tr_fmt("viewer.saved", &[("name", "a.png"), ("loc", "rupix")]);
        assert!(text.contains("a.png") && text.contains("rupix"));
        assert_eq!(tr("no.such.key"), "no.such.key");
    }

    /// 语言代码解析：中英识别，其余视为扩展语言，空回退中文。
    #[test]
    fn lang_from_code_cases() {
        assert_eq!(Lang::from_code("en"), Lang::En);
        assert_eq!(Lang::from_code(""), Lang::Zh);
        assert_eq!(Lang::from_code("ja"), Lang::Other("ja".to_string()));
    }
}
