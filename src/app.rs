//! 设置窗口模块：快捷键设置、截图入口、状态与最近保存记录。
//!
//! 设置窗口是 Rupix 的控制中心：平时隐藏到托盘常驻，
//! 截图本身发生在全屏浮层（`overlay`），预览标注在独立窗口（`viewer`），
//! 这里只负责发起流程、配置全局热键，以及展示保存历史。
//! 日常使用快捷键截图即可，无需打开本窗口。

use crate::{
    autostart,
    capture::{begin_capture, begin_picker, cap_log},
    export,
    hotkey::{HotkeyKind, apply_global_hotkey, hotkey_value_to_string, save_hotkey_string},
    i18n::{available_langs, lang, set_lang, tr, tr_fmt},
    ocr, toast,
    viewer::{open_viewer, preview_from_path},
};
use rgpui::{
    Context, Entity, PathPromptOptions, Render, SharedString, Switch, Window,
    components::{
        HotkeyInput, HotkeyInputState, HotkeyValue,
        status_bar::{StatusBar, StatusBarItem},
    },
    div,
    prelude::*,
    px, rgb,
};
use std::path::PathBuf;

/// 默认保存目录的配置文件名（与程序放同一目录，参考热键配置）。
const SAVE_DIR_FILE: &str = "rupix-savedir.txt";

/// 「截图后自动复制到剪贴板」开关的配置文件名（同目录，内容为 `1` / `0`）。
const AUTO_COPY_FILE: &str = "rupix-shot-autocopy.txt";

/// 「截图后打开编辑窗」开关的配置文件名（同目录，内容为 `1` / `0`）。
const OPEN_EDITOR_FILE: &str = "rupix-shot-openeditor.txt";

/// 项目仓库地址（关于页展示、打开与复制共用）。
const REPO_URL: &str = "https://github.com/launcher-rs/rupix";

/// 当前程序版本号（编译期取自 `Cargo.toml`，关于页展示用）。
const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

/// 系统默认保存目录：图片库下的 `rupix` 子目录（不存在自动创建）。
///
/// 候选基址按图片库（兼容 OneDrive 接管）、桌面排序，最后回退临时目录；
/// 创建失败则试下一候选，保证返回的目录真实可用。
pub fn default_save_dir() -> PathBuf {
    const SUB: &str = "rupix";
    let bases = [
        std::env::var("USERPROFILE")
            .map(|home| PathBuf::from(home).join("Pictures"))
            .ok(),
        std::env::var("OneDrive")
            .map(|root| PathBuf::from(root).join("Pictures"))
            .ok(),
        std::env::var("USERPROFILE")
            .map(|home| PathBuf::from(home).join("Desktop"))
            .ok(),
    ];
    for base in bases.into_iter().flatten() {
        if !base.is_dir() {
            continue;
        }
        let dir = base.join(SUB);
        if dir.is_dir() || std::fs::create_dir_all(&dir).is_ok() {
            return dir;
        }
    }
    std::env::temp_dir()
}

/// 单个配置文件（保存目录 / 截图行为开关）的完整路径。
///
/// 优先与程序放在同一目录（重装不丢）；取不到程序路径时回退系统临时目录。
fn setting_path(file: &str) -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(file)))
        .unwrap_or_else(|| std::env::temp_dir().join(file))
}

/// 读取用户配置的截图默认保存目录；未配置或目录已失效时回退到系统默认。
pub fn load_save_dir() -> PathBuf {
    // 配置与程序放在同一目录，重装也不会丢；目录被删/不可用则回退默认。
    let custom = std::fs::read_to_string(setting_path(SAVE_DIR_FILE))
        .ok()
        .map(|text| PathBuf::from(text.trim()))
        .filter(|dir| !dir.as_os_str().is_empty() && dir.is_dir());
    custom.unwrap_or_else(default_save_dir)
}

/// 持久化用户选择的截图默认保存目录（写入失败仅忽略，不阻断流程）。
///
/// # 参数
/// - `dir`: 用户在目录对话框中选中的真实目录。
pub fn store_save_dir(dir: &std::path::Path) {
    let _ = std::fs::write(setting_path(SAVE_DIR_FILE), dir.to_string_lossy().as_ref());
}

/// 解析布尔开关的文件内容：`1` / `true` / `on` 为开，`0` / `false` / `off` 为关。
///
/// 手改出错、空文件等无法识别的内容一律取默认值，不让配置问题影响截图流程。
fn parse_flag(text: &str, default: bool) -> bool {
    match text.trim().to_lowercase().as_str() {
        "1" | "true" | "on" => true,
        "0" | "false" | "off" => false,
        _ => default,
    }
}

/// 读取布尔开关；文件不存在时用 `default`（首次运行即默认行为）。
fn load_flag(file: &str, default: bool) -> bool {
    std::fs::read_to_string(setting_path(file))
        .map(|text| parse_flag(&text, default))
        .unwrap_or(default)
}

/// 持久化布尔开关：写 `1` / `0`（写入失败仅忽略，下次启动回到默认值）。
fn store_flag(file: &str, value: bool) {
    let _ = std::fs::write(setting_path(file), if value { "1" } else { "0" });
}

/// 读取「截图后自动复制到剪贴板」设置（默认开：截完直接 `Ctrl+V` 就能用）。
pub fn load_auto_copy() -> bool {
    load_flag(AUTO_COPY_FILE, true)
}

/// 持久化「截图后自动复制到剪贴板」设置（写入失败仅忽略，不阻断截图流程）。
///
/// # 参数
/// - `enabled`: 是否在截图保存后自动写剪贴板。
pub fn store_auto_copy(enabled: bool) {
    store_flag(AUTO_COPY_FILE, enabled);
}

/// 读取「截图后打开编辑窗」设置（默认关：截完只闪一下提示，需要时再手动编辑）。
pub fn load_open_editor() -> bool {
    load_flag(OPEN_EDITOR_FILE, false)
}

/// 持久化「截图后打开编辑窗」设置（写入失败仅忽略，不阻断截图流程）。
///
/// # 参数
/// - `enabled`: 是否在截图保存后自动打开编辑窗。
pub fn store_open_editor(enabled: bool) {
    store_flag(OPEN_EDITOR_FILE, enabled);
}

/// 截图自动保存：刚截到的原图先存一份到默认保存目录（时间戳命名，天然不重名）。
///
/// 保存成功后按设置自动复制到剪贴板（见 [`load_auto_copy`]）：不需要标注的用户
/// 截完直接 `Ctrl+V` 就能粘贴，省掉「先开预览窗、再点复制」这一步。
///
/// # 参数
/// - `src`: 刚截到/裁好的原图路径（临时文件）。
/// - `cx`: 应用上下文（跨平台写图片剪贴板要用；Windows 走双格式直写，不依赖它）。
///
/// # 返回
/// 成功返回新路径与「是否已复制到剪贴板」（viewer 用路径做预览源，之后的手动保存会另存新文件）；
/// 失败返回 `None`（调用方回退用临时文件，不阻断截图流程）。
pub fn autosave_screenshot(src: &std::path::Path, cx: &mut rgpui::App) -> Option<(PathBuf, bool)> {
    // 目录被用户删掉时尝试重建，重建失败则放弃自动保存。
    let dir = load_save_dir();
    if std::fs::create_dir_all(&dir).is_err() {
        return None;
    }
    let dst = dir.join(timestamped_png_name());
    std::fs::copy(src, &dst).ok()?;
    // 复制失败不影响截图：存盘照旧，只在诊断日志里记一行（用户可在设置里关掉自动复制）。
    let mut copied = false;
    if load_auto_copy() {
        match export::copy_saved_png(cx, &dst) {
            Ok(()) => copied = true,
            Err(error) => cap_log(&format!("截图自动复制失败：{error}")),
        }
    }
    Some((dst, copied))
}

/// 截图完成后的统一收尾：自动保存（按设置顺手复制）、再按设置决定弹编辑窗还是闪提示。
///
/// 区域框选（`overlay`）与全屏（`capture`）两条路径都走这里：截图后做什么只该有
/// 一处判断，否则迟早出现「区域弹窗、全屏不弹」这种分叉。
///
/// # 参数
/// - `state`: 主视图（记下「最近一张」，供「编辑最近一张」入口用）。
/// - `cx`: 主视图上下文（写剪贴板、开窗口都要）。
/// - `preview`: 刚截到 / 裁好的原图（临时文件路径 + 尺寸）。
///
/// # 返回
/// 自动保存后的预览（保存失败时仍是临时文件）与状态行提示语。
pub fn finish_capture(
    state: &mut RupixApp,
    cx: &mut Context<RupixApp>,
    preview: &PreviewImage,
) -> (PreviewImage, String) {
    // 自动保存内部会按设置顺手写一次剪贴板（不需要编辑可直接粘贴）。
    let saved = autosave_screenshot(&preview.path, cx);
    let saved_ok = saved.is_some();
    let (preview, copied, note) = match saved {
        Some((path, copied)) => (
            PreviewImage {
                path,
                w: preview.w,
                h: preview.h,
            },
            copied,
            tr("cap.auto_saved"),
        ),
        None => (preview.clone(), false, tr("cap.auto_fail")),
    };
    // 无论弹不弹编辑窗都记下最近一张：关掉编辑窗后，「编辑最近一张」是重新打开的入口。
    // 这里直接改 `state`，不嵌套 `update`：两条调用路径都已在实体租借内。
    state.last_preview = Some(preview.clone());
    // 自动保存失败时仍然打开编辑窗：此时只有临时文件，让用户自己另存一份才安全。
    if load_open_editor() || !saved_ok {
        open_viewer(cx.entity(), cx, preview.clone());
    } else {
        // 不弹窗时用右下角的胶囊交代结果，否则用户不知道到底截到没有。
        // 文案按「这次到底复制成功没有」来写，配置开着但剪贴板被占用时不会谎报。
        toast::show_saved(cx, &preview.path, copied);
    }
    (preview, note)
}

/// 带毫秒时间戳的截图文件名（自动保存、手动保存与 CLI 默认输出共用）。
pub(crate) fn timestamped_png_name() -> String {
    format!(
        "rupix-{}.png",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0)
    )
}

/// 主窗口展示的截图预览（自动保存后的文件，或用户选择的本地图片）。
#[derive(Clone)]
pub struct PreviewImage {
    /// 预览图片路径（截图自动保存后的文件；本地编辑模式下是用户原图）。
    pub path: PathBuf,
    /// 原图宽度（图片像素）。
    pub w: u32,
    /// 原图高度（图片像素）。
    pub h: u32,
}

/// 设置页 Tab：截图（入口）、快捷键（录制）、保存（目录+最近）、OCR 模型（目录+下载）、通用（语言+开机启动）、关于（版本与仓库）。
#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum SettingsTab {
    /// 截图：截图入口按钮。
    #[default]
    Capture,
    /// 快捷键：三种热键录制卡。
    Hotkey,
    /// 保存：默认目录与最近保存。
    Save,
    /// OCR 模型：自定义目录与下载指引。
    Ocr,
    /// 通用：界面语言与开机启动。
    General,
    /// 关于：版本、简介、许可证与仓库地址。
    About,
}

impl SettingsTab {
    /// Tab 唯一标识（选中态比对用）。
    fn id(self) -> &'static str {
        match self {
            SettingsTab::Capture => "capture",
            SettingsTab::Hotkey => "hotkey",
            SettingsTab::Save => "save",
            SettingsTab::Ocr => "ocr",
            SettingsTab::General => "general",
            SettingsTab::About => "about",
        }
    }

    /// Tab 显示文案（跟随当前语言）。
    fn label(self) -> String {
        match self {
            SettingsTab::Capture => tr("tab.capture"),
            SettingsTab::Hotkey => tr("tab.hotkey"),
            SettingsTab::Save => tr("tab.save"),
            SettingsTab::Ocr => tr("tab.ocr"),
            SettingsTab::General => tr("tab.general"),
            SettingsTab::About => tr("tab.about"),
        }
    }

    /// 从点击 id 还原（未知 id 回退截图页）。
    fn from_id(id: &str) -> Self {
        match id {
            "hotkey" => SettingsTab::Hotkey,
            "save" => SettingsTab::Save,
            "ocr" => SettingsTab::Ocr,
            "general" => SettingsTab::General,
            "about" => SettingsTab::About,
            _ => SettingsTab::Capture,
        }
    }

    /// 全部 Tab（左侧竖排栏顺序）。
    fn all() -> [SettingsTab; 6] {
        [
            SettingsTab::Capture,
            SettingsTab::Hotkey,
            SettingsTab::Save,
            SettingsTab::Ocr,
            SettingsTab::General,
            SettingsTab::About,
        ]
    }
}

/// 模型文件种类（设置页三行各一，文件名不做任何匹配，叫什么都能用）。
#[derive(Clone, Copy)]
enum ModelFileKind {
    /// 检测模型（找文本框）。
    Det,
    /// 识别模型（认框里文字）。
    Rec,
    /// 字符字典（识别结果查字表）。
    Keys,
}

impl ModelFileKind {
    /// 行显示文案（跟随当前语言）。
    fn label(self) -> String {
        match self {
            ModelFileKind::Det => tr("ocr.file_det"),
            ModelFileKind::Rec => tr("ocr.file_rec"),
            ModelFileKind::Keys => tr("ocr.file_keys"),
        }
    }

    /// 行按钮 id 后缀（保证唯一）。
    fn id(self) -> &'static str {
        match self {
            ModelFileKind::Det => "det",
            ModelFileKind::Rec => "rec",
            ModelFileKind::Keys => "keys",
        }
    }

    /// 三种文件共用同一个选择提示。
    fn prompt_key(self) -> &'static str {
        "ocr.pick_prompt"
    }
}

/// 截图入口动作（设置页截图 Tab 的入口按钮共用分发）。
#[derive(Clone, Copy)]
enum EntryAction {
    /// 区域截图。
    Region,
    /// 全屏截图。
    Full,
    /// 屏幕取色。
    Picker,
    /// 打开本地图片编辑。
    OpenImage,
    /// 重新编辑最近一张截图（截完没弹编辑窗时的补救入口）。
    EditLast,
}

/// 截图行为开关动作（截图 Tab 的两行开关共用分发）。
#[derive(Clone, Copy)]
enum BehaviorAction {
    /// 截图后自动复制到剪贴板。
    AutoCopy,
    /// 截图后打开编辑窗。
    OpenEditor,
}

/// 关于页的信息行：左侧固定宽灰标签，右侧值（长文本换行不把卡片撑宽）。
///
/// # 参数
/// - `label`: 行标签（版本 / 简介 / 许可证 …）。
/// - `value`: 行内容。
fn info_row(label: &str, value: &str) -> impl IntoElement {
    div()
        .flex()
        .items_baseline()
        .gap_3()
        .child(
            div()
                .flex_shrink_0()
                .w(px(76.))
                .text_xs()
                .text_color(rgb(0x888888))
                .child(label.to_string()),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .text_xs()
                .text_color(rgb(0x333333))
                .child(value.to_string()),
        )
}

/// 主视图状态。
pub struct RupixApp {
    /// 是否正在截取中（防止重复推流）。
    pub capturing: bool,
    /// 当前生效的区域截图热键字符串。
    region_hotkey: String,
    /// 当前生效的全屏截图热键字符串。
    full_hotkey: String,
    /// 当前生效的屏幕取色热键字符串。
    picker_hotkey: String,
    /// 区域热键录制框。
    region_input: Entity<HotkeyInputState>,
    /// 全屏热键录制框。
    full_input: Entity<HotkeyInputState>,
    /// 取色热键录制框。
    picker_input: Entity<HotkeyInputState>,
    /// 状态栏文本。
    status: SharedString,
    /// 设置页当前 Tab（截图 / 快捷键 / 保存 / OCR 模型 / 通用 / 关于）。
    settings_tab: SettingsTab,
    /// 截图默认保存目录（截图后自动存原图到此，设置页可改）。
    save_dir: PathBuf,
    /// 截图后是否自动复制到剪贴板（截图 Tab 开关，持久化在程序目录）。
    auto_copy: bool,
    /// 截图后是否打开编辑窗（截图 Tab 开关；关闭时只闪保存提示）。
    open_editor: bool,
    /// 最近一张截图（进程内保存，供「编辑最近一张」重开编辑窗）。
    ///
    /// 不写盘：它只是自动保存产物的路径，跨进程记下来反而会留下
    /// 「重启后指向一个已被删掉的文件」这种死记录。
    last_preview: Option<PreviewImage>,
    /// 最近保存的文件（路径 + 提示语，最多保留 10 条）。
    recents: Vec<(PathBuf, String)>,
    /// 开机启动当前状态（通用 Tab 开关的显示与切换依据，启动时读真实状态）。
    autostart: bool,
}

impl RupixApp {
    /// 创建主视图。
    ///
    /// # 参数
    /// - `window`: 主窗口（录制框初始化用）。
    /// - `cx`: 视图上下文。
    /// - `region_hotkey`: 当前生效的区域截图热键字符串。
    /// - `full_hotkey`: 当前生效的全屏截图热键字符串。
    /// - `picker_hotkey`: 当前生效的屏幕取色热键字符串。
    pub fn new(
        _window: &mut Window,
        cx: &mut Context<Self>,
        region_hotkey: String,
        full_hotkey: String,
        picker_hotkey: String,
    ) -> Self {
        Self {
            capturing: false,
            region_input: Self::make_input(cx, &region_hotkey),
            full_input: Self::make_input(cx, &full_hotkey),
            picker_input: Self::make_input(cx, &picker_hotkey),
            region_hotkey,
            full_hotkey,
            picker_hotkey,
            status: SharedString::from("就绪"),
            settings_tab: SettingsTab::default(),
            save_dir: load_save_dir(),
            // 两个截图行为开关直接取持久化值：默认「自动复制开、打开编辑窗关」。
            auto_copy: load_auto_copy(),
            open_editor: load_open_editor(),
            last_preview: None,
            recents: Vec::new(),
            // 开机启动状态实时取自注册表 / 自启动文件，避免与系统实际状态脱节。
            autostart: autostart::is_enabled(),
        }
    }

    /// 按按键串预填录制框（非法串则给空框，所见即所得）。
    fn make_input(cx: &mut Context<Self>, hotkey: &str) -> Entity<HotkeyInputState> {
        // 录制框预填当前热键，所见即所得。
        let preset = rgpui::Keystroke::parse(hotkey)
            .ok()
            .and_then(|ks| HotkeyValue::from_keystroke(&ks));
        match preset {
            Some(value) => cx.new(|cx| HotkeyInputState::with_hotkey(cx, value)),
            None => cx.new(HotkeyInputState::new),
        }
    }

    /// 设置状态栏并刷新。
    pub fn set_status(&mut self, msg: &str, cx: &mut Context<Self>) {
        self.status = SharedString::from(msg.to_string());
        cx.notify();
    }

    /// 新增一条保存记录（标注窗口保存成功后回写，设置页展示最近保存）。
    #[allow(dead_code)]
    pub fn push_recent(&mut self, path: PathBuf, msg: String, cx: &mut Context<Self>) {
        // 新记录插到最前，超限截断。
        self.recents.insert(0, (path, msg.clone()));
        self.recents.truncate(10);
        self.set_status(&msg, cx);
    }

    /// 发起区域截图（按钮 / 托盘 / 全局热键共用入口）。
    pub fn begin_region(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // 直接透传可变引用，`begin_capture` 内部不再二次租借实体。
        begin_capture(self, window, cx, false);
    }

    /// 发起全屏截图（打开即全选，直接标注）。
    pub fn begin_full(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        begin_capture(self, window, cx, true);
    }

    /// 发起屏幕取色（托盘 / 热键 / 设置页共用入口）。
    pub fn begin_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // 与截图同状态机，同一时间只允许一轮操作。
        begin_picker(self, window, cx);
    }

    /// 从本地选择一张图片，用 Rupix 预览窗口做简单标注编辑。
    ///
    /// 原图只读打开，保存时另存为带时间戳的新文件，不会覆盖原图。
    pub fn open_image_for_edit(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        // 原生文件对话框走平台实现，选图后在异步任务里打开预览窗口。
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(tr("image.pick_prompt").into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = receiver.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let Some(preview) = preview_from_path(&path) else {
                // 非图片文件：回写设置窗口状态提示。
                let _ = this.update(cx, |state, cx| {
                    state.set_status(&tr("image.invalid"), cx);
                });
                return;
            };
            let _ = this.update(cx, |_, cx| {
                open_viewer(cx.entity(), cx, preview);
            });
        })
        .detach();
    }

    /// 选择默认保存目录（原生目录对话框，选后持久化并刷新显示）。
    fn pick_save_dir(&mut self, cx: &mut Context<Self>) {
        // 原生目录对话框：只选单个已存在的目录。
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(tr("savedir.prompt").into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = receiver.await else {
                return;
            };
            let Some(dir) = paths.into_iter().next() else {
                return;
            };
            // 只要真实存在的目录，无效选择不更改当前配置。
            if !dir.is_dir() {
                let _ = this.update(cx, |state, cx| {
                    state.set_status(&tr("savedir.invalid"), cx);
                });
                return;
            }
            store_save_dir(&dir);
            let _ = this.update(cx, |state, cx| {
                state.save_dir = dir.clone();
                let msg = tr_fmt("savedir.updated", &[("dir", &dir.display().to_string())]);
                state.set_status(&msg, cx);
            });
        })
        .detach();
    }

    /// 模型文件种类（设置页三行各一，文件名不做任何匹配）。
    fn pick_model_file(&mut self, kind: ModelFileKind, cx: &mut Context<Self>) {
        // 原生文件对话框：只选单个已存在的文件。
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(tr(kind.prompt_key()).into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = receiver.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            // 空文件直接拒收（占位或没下完），其他照单全收。
            if !ocr::is_usable_file(&path) {
                let _ = this.update(cx, |state, cx| {
                    let msg = tr_fmt("ocr.file_empty", &[("file", &path.display().to_string())]);
                    state.set_status(&msg, cx);
                });
                return;
            }
            let _ = this.update(cx, |state, cx| {
                // 读出现有两项，换掉这一项后整体存回。
                let (mut det, mut rec, mut keys) = ocr::custom_model_files()
                    .map(|(det, rec, keys)| (Some(det), Some(rec), Some(keys)))
                    .unwrap_or((None, None, None));
                match kind {
                    ModelFileKind::Det => det = Some(path.clone()),
                    ModelFileKind::Rec => rec = Some(path.clone()),
                    ModelFileKind::Keys => keys = Some(path.clone()),
                }
                ocr::store_custom_model_files(det.as_deref(), rec.as_deref(), keys.as_deref());
                let msg = tr_fmt(
                    "ocr.file_set",
                    &[
                        ("label", &kind.label()),
                        ("file", &path.display().to_string()),
                    ],
                );
                state.set_status(&msg, cx);
            });
        })
        .detach();
    }

    /// 清除手动指定的三个文件，恢复自动查找。
    fn reset_model_files(&mut self, cx: &mut Context<Self>) {
        ocr::clear_custom_model_files();
        self.set_status(&tr("ocr.restored"), cx);
    }

    /// 恢复系统默认保存目录（图片库优先）。
    fn reset_save_dir(&mut self, cx: &mut Context<Self>) {
        let dir = default_save_dir();
        store_save_dir(&dir);
        self.save_dir = dir.clone();
        let msg = tr_fmt("savedir.restored", &[("dir", &dir.display().to_string())]);
        self.set_status(&msg, cx);
    }

    /// 切换开机启动：登记 / 删除当前系统的自启动项，成功后同步开关状态。
    ///
    /// 失败（权限不足、目录不可写等）只在状态栏提示，开关保持原状态，
    /// 免得界面显示“已开启”而系统里其实没生效。
    fn toggle_autostart(&mut self, cx: &mut Context<Self>) {
        let target = !self.autostart;
        match autostart::set_enabled(target) {
            Ok(()) => {
                self.autostart = target;
                let msg = tr(if target {
                    "autostart.enabled"
                } else {
                    "autostart.disabled"
                });
                self.set_status(&msg, cx);
            }
            Err(error) => {
                let msg = tr_fmt("autostart.failed", &[("err", &error.to_string())]);
                self.set_status(&msg, cx);
            }
        }
    }

    /// 设置「截图后自动复制到剪贴板」：先持久化再回执，下次启动保持一致。
    fn set_auto_copy(&mut self, enabled: bool, cx: &mut Context<Self>) {
        // 点当前已选中的那一侧不重复写盘、也不刷状态。
        if self.auto_copy == enabled {
            return;
        }
        self.auto_copy = enabled;
        store_auto_copy(enabled);
        self.report_behavior("behavior.auto_copy.title", enabled, cx);
    }

    /// 设置「截图后打开编辑窗」：关闭后截完只在右下角闪一下保存提示。
    fn set_open_editor(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if self.open_editor == enabled {
            return;
        }
        self.open_editor = enabled;
        store_open_editor(enabled);
        self.report_behavior("behavior.open_editor.title", enabled, cx);
    }

    /// 是否有可编辑的「最近一张截图」（入口按钮置灰用）。
    ///
    /// 顺手校验文件还在：用户可能在资源管理器里把它删了。
    pub fn has_last_preview(&self) -> bool {
        self.last_preview
            .as_ref()
            .is_some_and(|preview| preview.path.is_file())
    }

    /// 把「最近一张截图」重新打开进编辑窗（设置页按钮与托盘菜单共用）。
    ///
    /// 没有最近一张（或文件已被删掉）时只在状态栏提示，不开空窗口。
    pub fn open_last_for_edit(&mut self, cx: &mut Context<Self>) {
        let Some(preview) = self.last_preview.clone() else {
            self.set_status(&tr("image.no_recent"), cx);
            return;
        };
        if !preview.path.is_file() {
            // 文件没了就别留着这条记录，免得入口一直亮着却点不开。
            self.last_preview = None;
            self.set_status(&tr("image.no_recent"), cx);
            cx.notify();
            return;
        }
        open_viewer(cx.entity(), cx, preview);
    }

    /// 开关切换后的状态栏回执（「标题：已开启 / 已关闭」）。
    fn report_behavior(&mut self, title_key: &str, on: bool, cx: &mut Context<Self>) {
        let state = tr(if on { "behavior.on" } else { "behavior.off" });
        let msg = tr_fmt(
            "behavior.changed",
            &[("title", &tr(title_key)), ("state", &state)],
        );
        self.set_status(&msg, cx);
    }

    /// 快捷键录制框变化时自动应用新的全局热键（区域 / 全屏各自独立）。
    fn on_hotkey_recorded(
        &mut self,
        kind: HotkeyKind,
        value: Option<HotkeyValue>,
        cx: &mut Context<Self>,
    ) {
        let Some(value) = value else {
            // 清空录制：保持旧热键生效，仅提示。
            self.set_status(&tr("hotkey.cleared"), cx);
            return;
        };
        let text = hotkey_value_to_string(&value);
        match apply_global_hotkey(cx, kind, &text) {
            Ok(()) => {
                match kind {
                    HotkeyKind::Region => self.region_hotkey = text.clone(),
                    HotkeyKind::Full => self.full_hotkey = text.clone(),
                    HotkeyKind::Picker => self.picker_hotkey = text.clone(),
                }
                save_hotkey_string(kind, &text);
                let msg = tr_fmt(
                    "hotkey.applied",
                    &[("label", &kind.label()), ("text", &text)],
                );
                self.set_status(&msg, cx);
            }
            Err(error) => {
                self.set_status(&error, cx);
            }
        }
    }

    /// 单个热键录制卡片（每种截图一张，录制后自动应用）。
    fn hotkey_card(
        &self,
        cx: &mut Context<Self>,
        kind: HotkeyKind,
        input: Entity<HotkeyInputState>,
        current: &str,
    ) -> impl IntoElement {
        // kind 可复制，进闭包后按种类回写对应状态。
        let view = cx.entity();
        let title = tr_fmt("hotkey.card_title", &[("label", &kind.label())]);
        let current_text = tr_fmt("hotkey.current", &[("current", current)]);
        div()
            .flex()
            .flex_col()
            .gap_1()
            .p_3()
            .rounded_md()
            .bg(rgb(0xFFFFFF))
            .child(div().text_sm().child(title))
            .child(
                HotkeyInput::new(input)
                    .placeholder(tr("hotkey.placeholder"))
                    .recording_text(tr("hotkey.recording"))
                    .on_change(move |value, _, cx| {
                        view.update(cx, |this, cx| this.on_hotkey_recorded(kind, value, cx));
                    }),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(0x888888))
                    .child(current_text),
            )
    }

    /// 截图入口按钮（截图 Tab，截取中或条件不满足时置灰，颜色区分种类）。
    ///
    /// # 参数
    /// - `enabled`: 额外可用条件（如「编辑最近一张」要先有截图）；`false` 时置灰且不响应点击。
    fn entry_button(
        &self,
        cx: &mut Context<Self>,
        id: &'static str,
        label: String,
        color: u32,
        enabled: bool,
        action: EntryAction,
    ) -> impl IntoElement {
        let view = cx.entity();
        // 截取中一律禁用，避免重复发起；其余条件由调用方给。
        let active = enabled && !self.capturing;
        div()
            .id(id)
            .w_full()
            .px_3()
            .py_2()
            .rounded_md()
            .text_sm()
            .text_center()
            .cursor_pointer()
            .bg(if active { rgb(color) } else { rgb(0xC0C0C0) })
            .text_color(rgb(0xFFFFFF))
            .child(label)
            .on_click(move |_, window, cx| {
                if !active {
                    return;
                }
                view.update(cx, |this, cx| match action {
                    EntryAction::Region => this.begin_region(window, cx),
                    EntryAction::Full => this.begin_full(window, cx),
                    EntryAction::Picker => this.begin_picker(window, cx),
                    EntryAction::OpenImage => this.open_image_for_edit(window, cx),
                    EntryAction::EditLast => this.open_last_for_edit(cx),
                });
            })
    }

    /// 截图行为选项行（标题 + 开关 + 说明），截图 Tab 两行共用。
    ///
    /// 用 rgpui 的 `Switch`：标题在左、开关在右，说明文字压在下面。
    fn behavior_row(
        &self,
        cx: &mut Context<Self>,
        id: &'static str,
        title: String,
        note: String,
        on: bool,
        action: BehaviorAction,
    ) -> impl IntoElement {
        // 动作可复制，进开关回调后按种类回写对应设置。
        let view = cx.entity();
        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(div().text_sm().child(title))
                    .child(
                        Switch::new(id)
                            .checked(on)
                            // 与其他入口按钮同色，避免用主题默认的深色。
                            .color(rgb(0x1E90FF))
                            .on_change(move |checked, _, cx| {
                                view.update(cx, |this, cx| match action {
                                    BehaviorAction::AutoCopy => this.set_auto_copy(checked, cx),
                                    BehaviorAction::OpenEditor => {
                                        this.set_open_editor(checked, cx);
                                    }
                                });
                            }),
                    ),
            )
            .child(div().text_xs().text_color(rgb(0x555555)).child(note))
    }

    /// 截图 Tab：截图入口按钮（两列两行，各 200px 宽） + 编辑最近一张 + 截图行为选项 + 使用提示。
    ///
    /// 入口按钮排成 2×2 而不是竖着一行一个：右侧内容区只有约 410px 宽，
    /// 竖排四个按钮会把下面的「截图行为」挤出可视区，需要滚动才能看到。
    fn capture_tab(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(div().w(px(200.)).child(self.entry_button(
                        cx,
                        "rupix-btn-region",
                        tr("btn.region"),
                        0x1E90FF,
                        true,
                        EntryAction::Region,
                    )))
                    .child(div().w(px(200.)).child(self.entry_button(
                        cx,
                        "rupix-btn-full",
                        tr("btn.full"),
                        0x107C10,
                        true,
                        EntryAction::Full,
                    ))),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(div().w(px(200.)).child(self.entry_button(
                        cx,
                        "rupix-btn-picker",
                        tr("btn.picker"),
                        0xFF8C00,
                        true,
                        EntryAction::Picker,
                    )))
                    .child(div().w(px(200.)).child(self.entry_button(
                        cx,
                        "rupix-btn-open-image",
                        tr("btn.open_image"),
                        0x6A5ACD,
                        true,
                        EntryAction::OpenImage,
                    ))),
            )
            // 编辑最近一张：关掉「截图后打开编辑窗」后，这是找回上一张直接标注的入口。
            // 还没有截图（或文件已被删）时置灰，避免点开一个空窗。
            .child(div().w(px(408.)).child(self.entry_button(
                cx,
                "rupix-btn-edit-last",
                tr("btn.edit_last"),
                0x008B8B,
                self.has_last_preview(),
                EntryAction::EditLast,
            )))
            // 截图行为：截完自动复制 / 自动弹编辑窗，两项都即时持久化。
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .p_3()
                    .rounded_md()
                    .bg(rgb(0xFFFFFF))
                    .child(div().text_sm().child(tr("behavior.title")))
                    .child(self.behavior_row(
                        cx,
                        "rupix-btn-autocopy",
                        tr("behavior.auto_copy.title"),
                        tr("behavior.auto_copy.note"),
                        self.auto_copy,
                        BehaviorAction::AutoCopy,
                    ))
                    .child(self.behavior_row(
                        cx,
                        "rupix-btn-openeditor",
                        tr("behavior.open_editor.title"),
                        tr("behavior.open_editor.note"),
                        self.open_editor,
                        BehaviorAction::OpenEditor,
                    )),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(0x888888))
                    .child(tr("app.tips")),
            )
    }

    /// 快捷键 Tab：三种热键录制卡。
    fn hotkey_tab(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(self.hotkey_card(
                cx,
                HotkeyKind::Region,
                self.region_input.clone(),
                &self.region_hotkey.clone(),
            ))
            .child(self.hotkey_card(
                cx,
                HotkeyKind::Full,
                self.full_input.clone(),
                &self.full_hotkey.clone(),
            ))
            .child(self.hotkey_card(
                cx,
                HotkeyKind::Picker,
                self.picker_input.clone(),
                &self.picker_hotkey.clone(),
            ))
    }

    /// 保存 Tab：默认目录卡 + 最近保存。
    fn save_tab(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        let save_dir = self.save_dir.clone();
        let recents = self.recents.clone();
        let mut tab = div().flex().flex_col().gap_3().child(
            div()
                .flex()
                .flex_col()
                .gap_2()
                .p_3()
                .rounded_md()
                .bg(rgb(0xFFFFFF))
                .child(div().text_sm().child(tr("savedir.title")))
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(0x555555))
                        .child(save_dir.display().to_string()),
                )
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .child({
                            let v = view.clone();
                            div()
                                .id("rupix-btn-save-dir")
                                .px_3()
                                .py_1()
                                .rounded_md()
                                .text_sm()
                                .cursor_pointer()
                                .bg(rgb(0x1E90FF))
                                .text_color(rgb(0xFFFFFF))
                                .child(tr("savedir.pick"))
                                .on_click(move |_, _, cx| {
                                    v.update(cx, |this, cx| this.pick_save_dir(cx));
                                })
                        })
                        .child({
                            let v = view.clone();
                            div()
                                .id("rupix-btn-save-dir-reset")
                                .px_3()
                                .py_1()
                                .rounded_md()
                                .text_sm()
                                .cursor_pointer()
                                .bg(rgb(0x888888))
                                .text_color(rgb(0xFFFFFF))
                                .child(tr("savedir.reset"))
                                .on_click(move |_, _, cx| {
                                    v.update(cx, |this, cx| this.reset_save_dir(cx));
                                })
                        }),
                ),
        );
        // 最近保存：点击用系统默认程序打开。
        if !recents.is_empty() {
            tab = tab.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(div().text_sm().child(tr("app.recents")))
                    .children(recents.into_iter().enumerate().map(|(i, (path, msg))| {
                        let name = path
                            .file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or("?")
                            .to_string();
                        div()
                            .id(format!("rupix-recent-{i}"))
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .text_xs()
                            .cursor_pointer()
                            .bg(rgb(0xFFFFFF))
                            .text_color(rgb(0x1E90FF))
                            .child(format!("{name}  {msg}"))
                            .on_click(move |_, _, cx| {
                                cx.open_with_system(&path);
                            })
                    })),
            );
        }
        tab
    }

    /// OCR 模型 Tab 里的配置卡：当前生效 + 手动指定的三个文件（各行选文件）。
    fn ocr_card(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        // 实时解析（环境变量变化即时体现，不缓存到视图状态）。
        let active = ocr::active_models_info();
        let mut card = div()
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .rounded_md()
            .bg(rgb(0xFFFFFF))
            .child(div().text_sm().child(tr("ocr.title")));
        // 当前生效只显示标签（手动指定 / 家族名 / 内置），文件夹本身不展示；
        // 真缺模型才标红一行。
        match active {
            Some((label, _)) => {
                card = card.child(
                    div()
                        .text_xs()
                        .text_color(rgb(0x555555))
                        .child(tr_fmt("ocr.active", &[("label", &label)])),
                );
            }
            None if ocr::is_bundled() => {
                card = card.child(
                    div()
                        .text_xs()
                        .text_color(rgb(0x555555))
                        .child(tr_fmt("ocr.active", &[("label", &tr("ocr.bundled"))])),
                );
            }
            None => {
                card = card.child(
                    div()
                        .text_xs()
                        .text_color(rgb(0xAA0000))
                        .child(tr("ocr.missing")),
                );
            }
        }
        // 三行只展示文件：手动指定优先，否则用生效三件套的对应文件，
        // 无生效模型就显示为空——默认文件夹本身不展示。
        let (det, rec, keys) = ocr::effective_model_files();
        for (kind, current) in [
            (ModelFileKind::Det, det),
            (ModelFileKind::Rec, rec),
            (ModelFileKind::Keys, keys),
        ] {
            let v = view.clone();
            let shown = current
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| tr("ocr.unset"));
            card = card.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_xs()
                            .text_color(rgb(0x333333))
                            .child(kind.label()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .text_xs()
                            .text_color(rgb(0x555555))
                            .child(shown),
                    )
                    .child(
                        div()
                            .id(format!("rupix-btn-ocr-file-{}", kind.id()))
                            .flex_shrink_0()
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .text_xs()
                            .cursor_pointer()
                            .bg(rgb(0x1E90FF))
                            .text_color(rgb(0xFFFFFF))
                            .child(tr("ocr.pick_file"))
                            .on_click(move |_, _, cx| {
                                v.update(cx, |this, cx| this.pick_model_file(kind, cx));
                            }),
                    ),
            );
        }
        card.child(div().flex().gap_2().child({
            let v = view.clone();
            div()
                .id("rupix-btn-ocr-files-reset")
                .px_3()
                .py_1()
                .rounded_md()
                .text_sm()
                .cursor_pointer()
                .bg(rgb(0x888888))
                .text_color(rgb(0xFFFFFF))
                .child(tr("ocr.reset"))
                .on_click(move |_, _, cx| {
                    v.update(cx, |this, cx| this.reset_model_files(cx));
                })
        }))
        .child(
            div()
                .text_xs()
                .text_color(rgb(0x888888))
                .child(tr("ocr.note")),
        )
    }

    /// OCR 模型 Tab 里的下载指引卡：上游地址 + 一键命令（均可复制）+ 三件套说明。
    fn download_card(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        // 小按钮构造：文案 + 点击复制的文本，复制后状态栏反馈。
        let copy_button =
            |id: &'static str, label: String, text: String, view: Entity<RupixApp>| {
                div()
                    .id(id)
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .text_xs()
                    .cursor_pointer()
                    .bg(rgb(0xEEEEEE))
                    .text_color(rgb(0x333333))
                    .child(label)
                    .on_click(move |_, _, cx| {
                        let text = text.clone();
                        view.update(cx, |this, cx| {
                            cx.write_to_clipboard(rgpui::ClipboardItem::new_string(text));
                            this.set_status(&tr("ocr.copied"), cx);
                        });
                    })
            };
        div()
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .rounded_md()
            .bg(rgb(0xFFFFFF))
            .child(div().text_sm().child(tr("ocr.dl_title")))
            // 上游地址行：地址文本 + 复制链接按钮。
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(0x555555))
                    .child(tr("ocr.dl_from")),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(0x1E90FF))
                            .child(ocr::MODELS_UPSTREAM_URL.to_string()),
                    )
                    .child(copy_button(
                        "rupix-btn-ocr-copy-url",
                        tr("ocr.copy"),
                        ocr::MODELS_UPSTREAM_URL.to_string(),
                        view.clone(),
                    )),
            )
            // 自有模型库行：地址文本 + 复制链接按钮。
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(0x555555))
                    .child(tr("ocr.dl_hf")),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(0x1E90FF))
                            .child(ocr::HF_MODELS_URL.to_string()),
                    )
                    .child(copy_button(
                        "rupix-btn-ocr-copy-hf",
                        tr("ocr.copy"),
                        ocr::HF_MODELS_URL.to_string(),
                        view.clone(),
                    )),
            )
            // 一键命令行：命令文本 + 复制命令按钮。
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(0x555555))
                    .child(tr("ocr.dl_cmd")),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(0x333333))
                            .child(ocr::DOWNLOAD_CMD.to_string()),
                    )
                    .child(copy_button(
                        "rupix-btn-ocr-copy-cmd",
                        tr("ocr.copy"),
                        ocr::DOWNLOAD_CMD.to_string(),
                        view.clone(),
                    )),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(0x888888))
                    .child(tr("ocr.dl_files")),
            )
    }

    /// OCR 模型 Tab：配置卡（当前家族 + 自定义目录）+ 下载指引卡。
    fn ocr_tab(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(self.ocr_card(cx))
            .child(self.download_card(cx))
    }

    /// 通用 Tab：界面语言卡 + 开机启动卡。
    fn general_tab(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        let current = lang();
        let lang_card = div()
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .rounded_md()
            .bg(rgb(0xFFFFFF))
            .child(div().text_sm().child(tr("lang.title")))
            .child(
                div()
                    .flex()
                    .gap_2()
                    .children(available_langs().into_iter().map(|item| {
                        let v = view.clone();
                        let selected = item == current;
                        let code = item.code();
                        let name = item.display_name();
                        div()
                            .id(format!("rupix-lang-{code}"))
                            .px_3()
                            .py_1()
                            .rounded_md()
                            .text_sm()
                            .cursor_pointer()
                            .bg(if selected {
                                rgb(0x1E90FF)
                            } else {
                                rgb(0xEEEEEE)
                            })
                            .text_color(if selected {
                                rgb(0xFFFFFF)
                            } else {
                                rgb(0x333333)
                            })
                            .child(name.clone())
                            .on_click(move |_, _, cx| {
                                set_lang(&code);
                                v.update(cx, |this, cx| {
                                    let msg = tr_fmt("lang.switched", &[("lang", &name)]);
                                    this.set_status(&msg, cx);
                                });
                            })
                    })),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(0x888888))
                    .child(tr("lang.note")),
            );
        // 语言卡 + 开机启动卡，竖排成一列（OCR 配置已独立成 Tab）。
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(lang_card)
            .child(self.autostart_card(cx))
    }

    /// 开机启动卡（通用 Tab）：显示当前状态，点击在“开启 / 关闭”间切换。
    ///
    /// 状态以系统实际登记为准（实时读注册表 / 自启动文件），不额外存配置，
    /// 避免与任务管理器、系统设置里的改动打架。
    fn autostart_card(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let v = cx.entity();
        let on = self.autostart;
        let mut card = div()
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .rounded_md()
            .bg(rgb(0xFFFFFF))
            .child(div().text_sm().child(tr("autostart.title")))
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(0x555555))
                    .child(tr("autostart.note")),
            );
        if autostart::is_supported() {
            // 开关做成两态胶囊按钮（与语言选择同款交互）：绿 = 已开启，灰 = 已关闭。
            card = card.child(
                div()
                    .id("rupix-btn-autostart")
                    .w(px(104.))
                    .px_3()
                    .py_1()
                    .rounded_md()
                    .text_sm()
                    .text_center()
                    .cursor_pointer()
                    .bg(if on { rgb(0x1F9E4B) } else { rgb(0xEEEEEE) })
                    .text_color(if on { rgb(0xFFFFFF) } else { rgb(0x333333) })
                    .child(tr(if on { "autostart.on" } else { "autostart.off" }))
                    .on_click(move |_, _, cx| {
                        v.update(cx, |this, cx| this.toggle_autostart(cx));
                    }),
            );
        } else {
            // 未适配的平台只说明不支持，不放一个点了没反应的开关。
            card = card.child(
                div()
                    .text_xs()
                    .text_color(rgb(0xAA0000))
                    .child(tr("autostart.unsupported")),
            );
        }
        card
    }

    /// 关于 Tab：版本、简介与仓库地址（地址可一键打开 / 复制）。
    ///
    /// 刻意只放三条信息：用户打开「关于」最想知道的就是这是什么、什么版本、去哪反馈，
    /// 技术栈 / 许可证 / 理念这类内容放 README，不占设置页位置。
    fn about_tab(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        let version = format!("v{APP_VERSION}");
        let mut about_card = div()
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .rounded_md()
            .bg(rgb(0xFFFFFF))
            .child(div().text_sm().child(tr("about.title")))
            .child(info_row(&tr("about.version"), &version))
            .child(info_row(&tr("about.desc"), &tr("about.desc_value")));
        // 仓库行：长地址允许收缩换行，右侧固定两个小按钮。
        let open_view = view.clone();
        about_card = about_card.child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(0x888888))
                        .child(tr("about.repo")),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .text_xs()
                                .text_color(rgb(0x1E90FF))
                                .child(REPO_URL.to_string()),
                        )
                        // 打开仓库：交系统默认浏览器（各平台后端都认 URL）。
                        .child(
                            div()
                                .id("rupix-btn-about-open")
                                .flex_shrink_0()
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .text_xs()
                                .cursor_pointer()
                                .bg(rgb(0x1E90FF))
                                .text_color(rgb(0xFFFFFF))
                                .child(tr("about.open"))
                                .on_click(|_, _, cx| {
                                    cx.open_with_system(std::path::Path::new(REPO_URL));
                                }),
                        )
                        // 复制链接：写剪贴板 + 状态栏回执（与 OCR 卡的复制一致）。
                        .child(
                            div()
                                .id("rupix-btn-about-copy")
                                .flex_shrink_0()
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .text_xs()
                                .cursor_pointer()
                                .bg(rgb(0xEEEEEE))
                                .text_color(rgb(0x333333))
                                .child(tr("about.copy"))
                                .on_click(move |_, _, cx| {
                                    let url = REPO_URL.to_string();
                                    open_view.update(cx, |this, cx| {
                                        cx.write_to_clipboard(rgpui::ClipboardItem::new_string(
                                            url,
                                        ));
                                        this.set_status(&tr("about.copied"), cx);
                                    });
                                }),
                        ),
                ),
        );
        about_card
    }
}

impl Render for RupixApp {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        let tab = self.settings_tab;
        let status = self.status.clone();

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(0xF7F7F7))
            .text_color(rgb(0x333333))
            // 主体：左侧 Tab 栏 + 右侧内容区（共同撑着状态栏以上的空间）。
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h(px(0.))
                    .w_full()
                    // 左侧竖排 Tab 栏（固定宽不收缩；窗口压矮时内部滚动而非裁掉）。
                    .child(
                        div()
                            .id("settings-tabs")
                            .flex()
                            .flex_col()
                            .gap_1()
                            .w(px(118.))
                            .flex_shrink_0()
                            .min_h(px(0.))
                            .overflow_y_scroll()
                            .p_3()
                            .bg(rgb(0xFFFFFF))
                            .children(SettingsTab::all().into_iter().map(|item| {
                                let v = view.clone();
                                let selected = item == tab;
                                div()
                                    .id(format!("rupix-tab-{}", item.id()))
                                    .px_3()
                                    .py_2()
                                    .rounded_md()
                                    .text_sm()
                                    .cursor_pointer()
                                    .bg(if selected {
                                        rgb(0x1E90FF)
                                    } else {
                                        rgb(0xFFFFFF)
                                    })
                                    .text_color(if selected {
                                        rgb(0xFFFFFF)
                                    } else {
                                        rgb(0x333333)
                                    })
                                    .hover(|style| {
                                        style.bg(if selected {
                                            rgb(0x1E90FF)
                                        } else {
                                            rgb(0xEEEEEE)
                                        })
                                    })
                                    .child(item.label())
                                    .on_click(move |_, _, cx| {
                                        v.update(cx, |this, cx| {
                                            this.settings_tab = SettingsTab::from_id(item.id());
                                            cx.notify();
                                        });
                                    })
                            })),
                    )
                    // 右侧：标题 + 可滚内容（min_w(0) 允许收缩，长文本换行不撑宽）。
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .min_h(px(0.))
                            .overflow_hidden()
                            .flex()
                            .flex_col()
                            // 标题（设置窗口：平时隐藏到托盘；固定不滚）。
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .flex()
                                    .items_baseline()
                                    .gap_2()
                                    .px_4()
                                    .pt_4()
                                    .pb_3()
                                    .child(div().text_xl().child(tr("app.title")))
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(rgb(0x888888))
                                            .child(tr("app.subtitle")),
                                    ),
                            )
                            // Tab 内容区：窗口压小时内部滚动，标题保持可见。
                            .child(
                                div()
                                    .id("settings-content")
                                    .flex_1()
                                    .min_h(px(0.))
                                    .w_full()
                                    .overflow_y_scroll()
                                    .px_4()
                                    .child(match tab {
                                        SettingsTab::Capture => {
                                            self.capture_tab(cx).into_any_element()
                                        }
                                        SettingsTab::Hotkey => {
                                            self.hotkey_tab(cx).into_any_element()
                                        }
                                        SettingsTab::Save => self.save_tab(cx).into_any_element(),
                                        SettingsTab::Ocr => self.ocr_tab(cx).into_any_element(),
                                        SettingsTab::General => {
                                            self.general_tab(cx).into_any_element()
                                        }
                                        SettingsTab::About => self.about_tab(cx).into_any_element(),
                                    }),
                            ),
                    ),
            )
            // 状态栏：rgpui 的 StatusBar 组件，横跨整个窗口贴底，各 Tab 的反馈都落这里。
            // 单行显示，超长时靠 tooltip 看全文。
            .child(StatusBar::new().left(vec![
                StatusBarItem::new(status.clone())
                    .id("rupix-status")
                    .tooltip(status),
            ]))
    }
}

#[cfg(test)]
mod tests {
    //! 设置窗口模块单测：只测纯逻辑（配置文件解析），不碰窗口与剪贴板。

    use super::*;

    /// 开关文件内容解析：常见写法都认，无法识别的取默认值。
    #[test]
    fn parse_flag_cases() {
        for text in ["1", "1\n", " true ", "ON", "On"] {
            assert!(parse_flag(text, false), "{text:?} 应解析为开");
        }
        for text in ["0", "0\r\n", "false", " OFF "] {
            assert!(!parse_flag(text, true), "{text:?} 应解析为关");
        }
        // 空文件、手改出错时保持调用方给的默认值（自动复制默认开、编辑窗默认关）。
        assert!(parse_flag("", true));
        assert!(!parse_flag("", false));
        assert!(!parse_flag("yes please", false));
        assert!(parse_flag("?", true));
    }
}
