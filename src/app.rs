//! 设置窗口模块：快捷键设置、截图入口、状态与最近保存记录。
//!
//! 设置窗口是 Rupix 的控制中心：平时隐藏到托盘常驻，
//! 截图本身发生在全屏浮层（`overlay`），预览标注在独立窗口（`viewer`），
//! 这里只负责发起流程、配置全局热键，以及展示保存历史。
//! 日常使用快捷键截图即可，无需打开本窗口。

use crate::{
    capture::{begin_capture, begin_picker},
    hotkey::{HotkeyKind, apply_global_hotkey, hotkey_value_to_string, save_hotkey_string},
    viewer::{open_viewer, preview_from_path},
};
use rgpui::{
    Context, Entity, PathPromptOptions, Render, SharedString, Window,
    components::{HotkeyInput, HotkeyInputState, HotkeyValue},
    div,
    prelude::*,
    rgb,
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
            prompt: Some("选择要编辑的图片".into()),
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
                    state.set_status("所选文件不是有效图片", cx);
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
            prompt: Some("选择截图默认保存目录".into()),
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
                    state.set_status("所选目录无效，未更改保存路径", cx);
                });
                return;
            }
            store_save_dir(&dir);
            let _ = this.update(cx, |state, cx| {
                state.save_dir = dir.clone();
                state.set_status(&format!("默认保存目录已更新：{}", dir.display()), cx);
            });
        })
        .detach();
    }

    /// 恢复系统默认保存目录（图片库优先）。
    fn reset_save_dir(&mut self, cx: &mut Context<Self>) {
        let dir = default_save_dir();
        store_save_dir(&dir);
        self.save_dir = dir.clone();
        self.set_status(&format!("已恢复默认保存目录：{}", dir.display()), cx);
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
            self.set_status("已清空录制，旧快捷键继续生效", cx);
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
                self.set_status(&format!("{}全局截图快捷键：{text}", kind.label()), cx);
            }
            Err(error) => {
                self.set_status(&error, cx);
            }
        }
    }

    /// 单个热键录制卡片（区域 / 全屏各一张，录制后自动应用）。
    fn hotkey_card(
        &self,
        cx: &mut Context<Self>,
        kind: HotkeyKind,
        input: Entity<HotkeyInputState>,
        current: &str,
    ) -> impl IntoElement {
        // kind 可复制，进闭包后按种类回写对应状态。
        let view = cx.entity();
        div()
            .flex()
            .flex_col()
            .gap_1()
            .p_3()
            .rounded_md()
            .bg(rgb(0xFFFFFF))
            .child(div().text_sm().child(format!(
                "{}全局热键（点击框，按下组合键即自动生效）",
                kind.label()
            )))
            .child(
                HotkeyInput::new(input)
                    .placeholder("点这里，然后按下快捷键")
                    .recording_text("正在录制…")
                    .on_change(move |value, _, cx| {
                        view.update(cx, |this, cx| this.on_hotkey_recorded(kind, value, cx));
                    }),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(0x888888))
                    .child(format!("当前：{current}（失焦可用，托盘常驻）")),
            )
    }
}

impl Render for RupixApp {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        let region_hotkey = self.region_hotkey.clone();
        let full_hotkey = self.full_hotkey.clone();
        let picker_hotkey = self.picker_hotkey.clone();
        let region_input = self.region_input.clone();
        let full_input = self.full_input.clone();
        let picker_input = self.picker_input.clone();
        let status = self.status.clone();
        let save_dir = self.save_dir.clone();
        let capturing = self.capturing;
        let recents = self.recents.clone();

        div()
            .size_full()
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .bg(rgb(0xF7F7F7))
            .text_color(rgb(0x333333))
            // 标题（设置窗口：平时隐藏到托盘）。
            .child(
                div()
                    .flex()
                    .items_baseline()
                    .gap_2()
                    .child(div().text_xl().child("Rupix · 拾景 设置"))
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(0x888888))
                            .child("关闭即隐藏到托盘 · 日常凭快捷键截图即可"),
                    ),
            )
            // 快捷键设置：区域 / 全屏 / 取色各一张录制卡（录制后自动应用）。
            .child(self.hotkey_card(cx, HotkeyKind::Region, region_input, &region_hotkey))
            .child(self.hotkey_card(cx, HotkeyKind::Full, full_input, &full_hotkey))
            .child(self.hotkey_card(cx, HotkeyKind::Picker, picker_input, &picker_hotkey))
            // 默认保存路径：截图后自动存一份原图到此，标注后点保存会另存新文件。
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .p_3()
                    .rounded_md()
                    .bg(rgb(0xFFFFFF))
                    .child(div().text_sm().child("默认保存路径（截图自动保存原图到此）"))
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
                                    .child("选择目录…")
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
                                    .child("恢复默认")
                                    .on_click(move |_, _, cx| {
                                        v.update(cx, |this, cx| this.reset_save_dir(cx));
                                    })
                            }),
                    ),
            )
            // 截图入口。
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child({
                        let v = view.clone();
                        div()
                            .id("rupix-btn-region")
                            .px_3()
                            .py_2()
                            .rounded_md()
                            .text_sm()
                            .cursor_pointer()
                            .bg(if capturing {
                                rgb(0xC0C0C0)
                            } else {
                                rgb(0x1E90FF)
                            })
                            .text_color(rgb(0xFFFFFF))
                            .child("区域截图")
                            .on_click(move |_, window, cx| {
                                v.update(cx, |this, cx| this.begin_region(window, cx));
                            })
                    })
                    .child({
                        let v = view.clone();
                        div()
                            .id("rupix-btn-full")
                            .px_3()
                            .py_2()
                            .rounded_md()
                            .text_sm()
                            .cursor_pointer()
                            .bg(if capturing {
                                rgb(0xC0C0C0)
                            } else {
                                rgb(0x107C10)
                            })
                            .text_color(rgb(0xFFFFFF))
                            .child("全屏截图")
                            .on_click(move |_, window, cx| {
                                v.update(cx, |this, cx| this.begin_full(window, cx));
                            })
                    })
                    .child({
                        let v = view.clone();
                        div()
                            .id("rupix-btn-picker")
                            .px_3()
                            .py_2()
                            .rounded_md()
                            .text_sm()
                            .cursor_pointer()
                            .bg(if capturing {
                                rgb(0xC0C0C0)
                            } else {
                                rgb(0xFF8C00)
                            })
                            .text_color(rgb(0xFFFFFF))
                            .child("屏幕取色")
                            .on_click(move |_, window, cx| {
                                v.update(cx, |this, cx| this.begin_picker(window, cx));
                            })
                    })
                    .child({
                        let v = view.clone();
                        div()
                            .id("rupix-btn-open-image")
                            .px_3()
                            .py_2()
                            .rounded_md()
                            .text_sm()
                            .cursor_pointer()
                            .bg(if capturing {
                                rgb(0xC0C0C0)
                            } else {
                                rgb(0x6A5ACD)
                            })
                            .text_color(rgb(0xFFFFFF))
                            .child("打开图片编辑…")
                            .on_click(move |_, window, cx| {
                                v.update(cx, |this, cx| this.open_image_for_edit(window, cx));
                            })
                    }),
            )
            // 使用提示（设置窗口平时隐藏，截图走快捷键 / 托盘）。
            .child(div().text_xs().text_color(rgb(0x888888)).child(
                "区域截图：在全屏遮罩上拖拽框选（悬停窗口高亮时点击直接截该窗口），松手后进独立预览窗口（可缩放/复制/保存，可底部状态栏查看结果），右键/Esc 取消。取色：在冻结帧上移动取色，点击复制 HEX。本窗口关闭即隐藏到托盘。",
            ))
            // 状态栏。
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(0x555555))
                    .child(format!("状态：{status}")),
            )
            // 最近保存：点击用系统默认程序打开。
            .when(!recents.is_empty(), |el| {
                el.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(div().text_sm().child("最近保存（点击打开）"))
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
                )
            })
    }
}
