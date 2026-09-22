//! 设置窗口模块：快捷键设置、截图入口、状态与最近保存记录。
//!
//! 设置窗口是 Rupix 的控制中心：平时隐藏到托盘常驻，
//! 截图本身发生在全屏浮层（`overlay`），预览标注在独立窗口（`viewer`），
//! 这里只负责发起流程、配置全局热键，以及展示保存历史。
//! 日常使用快捷键截图即可，无需打开本窗口。

use crate::{
    capture::{begin_capture, begin_picker},
    hotkey::{HotkeyKind, apply_global_hotkey, hotkey_value_to_string, save_hotkey_string},
    i18n::{available_langs, lang, set_lang, tr, tr_fmt},
    viewer::{open_viewer, preview_from_path},
};
use rgpui::{
    Context, Entity, PathPromptOptions, Render, SharedString, Window,
    components::{HotkeyInput, HotkeyInputState, HotkeyValue},
    div,
    prelude::*,
    px, rgb,
};
use std::path::PathBuf;

/// 默认保存目录的配置文件名（与程序放同一目录，参考热键配置）。
const SAVE_DIR_FILE: &str = "rupix-savedir.txt";

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

/// 读取用户配置的截图默认保存目录；未配置或目录已失效时回退到系统默认。
pub fn load_save_dir() -> PathBuf {
    // 配置与程序放在同一目录，重装也不会丢；目录被删/不可用则回退默认。
    let custom = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(SAVE_DIR_FILE)))
        .and_then(|path| std::fs::read_to_string(path).ok())
        .map(|text| PathBuf::from(text.trim()))
        .filter(|dir| !dir.as_os_str().is_empty() && dir.is_dir());
    custom.unwrap_or_else(default_save_dir)
}

/// 持久化用户选择的截图默认保存目录（写入失败仅忽略，不阻断流程）。
///
/// # 参数
/// - `dir`: 用户在目录对话框中选中的真实目录。
pub fn store_save_dir(dir: &std::path::Path) {
    // 优先放在程序所在目录，极端情况回退到系统临时目录。
    let path = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(SAVE_DIR_FILE)))
        .unwrap_or_else(|| std::env::temp_dir().join(SAVE_DIR_FILE));
    let _ = std::fs::write(path, dir.to_string_lossy().as_ref());
}

/// 截图自动保存：刚截到的原图先存一份到默认保存目录（时间戳命名，天然不重名）。
///
/// # 参数
/// - `src`: 刚截到/裁好的原图路径（临时文件）。
///
/// # 返回
/// 成功返回新路径（viewer 用它做预览源，之后的手动保存会另存新文件）；
/// 失败返回 `None`（调用方回退用临时文件，不阻断截图流程）。
pub fn autosave_screenshot(src: &std::path::Path) -> Option<PathBuf> {
    // 目录被用户删掉时尝试重建，重建失败则放弃自动保存。
    let dir = load_save_dir();
    if std::fs::create_dir_all(&dir).is_err() {
        return None;
    }
    let dst = dir.join(timestamped_png_name());
    std::fs::copy(src, &dst).ok()?;
    Some(dst)
}

/// 带毫秒时间戳的截图文件名（自动保存与手动保存共用）。
fn timestamped_png_name() -> String {
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

/// 设置页 Tab：截图（入口）、快捷键（录制）、保存（目录+最近）、通用（语言）。
#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum SettingsTab {
    /// 截图：截图入口按钮。
    #[default]
    Capture,
    /// 快捷键：三种热键录制卡。
    Hotkey,
    /// 保存：默认目录与最近保存。
    Save,
    /// 通用：界面语言。
    General,
}

impl SettingsTab {
    /// Tab 唯一标识（选中态比对用）。
    fn id(self) -> &'static str {
        match self {
            SettingsTab::Capture => "capture",
            SettingsTab::Hotkey => "hotkey",
            SettingsTab::Save => "save",
            SettingsTab::General => "general",
        }
    }

    /// Tab 显示文案（跟随当前语言）。
    fn label(self) -> String {
        match self {
            SettingsTab::Capture => tr("tab.capture"),
            SettingsTab::Hotkey => tr("tab.hotkey"),
            SettingsTab::Save => tr("tab.save"),
            SettingsTab::General => tr("tab.general"),
        }
    }

    /// 从点击 id 还原（未知 id 回退截图页）。
    fn from_id(id: &str) -> Self {
        match id {
            "hotkey" => SettingsTab::Hotkey,
            "save" => SettingsTab::Save,
            "general" => SettingsTab::General,
            _ => SettingsTab::Capture,
        }
    }

    /// 全部 Tab（左侧竖排栏顺序）。
    fn all() -> [SettingsTab; 4] {
        [
            SettingsTab::Capture,
            SettingsTab::Hotkey,
            SettingsTab::Save,
            SettingsTab::General,
        ]
    }
}

/// 截图入口动作（设置页截图 Tab 的四个按钮共用分发）。
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
    /// 设置页当前 Tab（截图 / 保存 / 通用）。
    settings_tab: SettingsTab,
    /// 截图默认保存目录（截图后自动存原图到此，设置页可改）。
    save_dir: PathBuf,
    /// 最近保存的文件（路径 + 提示语，最多保留 10 条）。
    recents: Vec<(PathBuf, String)>,
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
            recents: Vec::new(),
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

    /// 恢复系统默认保存目录（图片库优先）。
    fn reset_save_dir(&mut self, cx: &mut Context<Self>) {
        let dir = default_save_dir();
        store_save_dir(&dir);
        self.save_dir = dir.clone();
        let msg = tr_fmt("savedir.restored", &[("dir", &dir.display().to_string())]);
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

    /// 截图入口按钮（截图 Tab，截取中整行置灰，颜色区分种类）。
    fn entry_button(
        &self,
        cx: &mut Context<Self>,
        id: &'static str,
        label: String,
        color: u32,
        action: EntryAction,
    ) -> impl IntoElement {
        let view = cx.entity();
        let capturing = self.capturing;
        div()
            .id(id)
            .w_full()
            .px_3()
            .py_2()
            .rounded_md()
            .text_sm()
            .text_center()
            .cursor_pointer()
            .bg(if capturing { rgb(0xC0C0C0) } else { rgb(color) })
            .text_color(rgb(0xFFFFFF))
            .child(label)
            .on_click(move |_, window, cx| {
                view.update(cx, |this, cx| match action {
                    EntryAction::Region => this.begin_region(window, cx),
                    EntryAction::Full => this.begin_full(window, cx),
                    EntryAction::Picker => this.begin_picker(window, cx),
                    EntryAction::OpenImage => this.open_image_for_edit(window, cx),
                });
            })
    }

    /// 截图 Tab：截图入口按钮（一行一个，固定 200px 宽） + 使用提示。
    fn capture_tab(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(div().w(px(200.)).child(self.entry_button(
                cx,
                "rupix-btn-region",
                tr("btn.region"),
                0x1E90FF,
                EntryAction::Region,
            )))
            .child(div().w(px(200.)).child(self.entry_button(
                cx,
                "rupix-btn-full",
                tr("btn.full"),
                0x107C10,
                EntryAction::Full,
            )))
            .child(div().w(px(200.)).child(self.entry_button(
                cx,
                "rupix-btn-picker",
                tr("btn.picker"),
                0xFF8C00,
                EntryAction::Picker,
            )))
            .child(div().w(px(200.)).child(self.entry_button(
                cx,
                "rupix-btn-open-image",
                tr("btn.open_image"),
                0x6A5ACD,
                EntryAction::OpenImage,
            )))
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

    /// 通用 Tab：界面语言卡。
    fn general_tab(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        let current = lang();
        div().flex().flex_col().gap_3().child(
            div()
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
                ),
        )
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
            .bg(rgb(0xF7F7F7))
            .text_color(rgb(0x333333))
            // 左侧竖排 Tab 栏（固定宽不收缩）。
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .w(px(118.))
                    .flex_shrink_0()
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
            // 右侧：标题 + 内容 + 状态栏（min_w(0) 允许收缩，长文本换行不撑宽）。
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .overflow_hidden()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .p_4()
                    // 标题（设置窗口：平时隐藏到托盘）。
                    .child(
                        div()
                            .flex()
                            .items_baseline()
                            .gap_2()
                            .child(div().text_xl().child(tr("app.title")))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(0x888888))
                                    .child(tr("app.subtitle")),
                            ),
                    )
                    // Tab 内容区。
                    .child(match tab {
                        SettingsTab::Capture => self.capture_tab(cx).into_any_element(),
                        SettingsTab::Hotkey => self.hotkey_tab(cx).into_any_element(),
                        SettingsTab::Save => self.save_tab(cx).into_any_element(),
                        SettingsTab::General => self.general_tab(cx).into_any_element(),
                    })
                    // 状态栏（常驻底部，各 Tab 操作反馈统一落这里）。
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(0x555555))
                            .child(tr_fmt("app.status", &[("status", &status)])),
                    ),
            )
    }
}
