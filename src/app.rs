//! 设置窗口模块：快捷键设置、截图入口、状态与最近保存记录。
//!
//! 设置窗口是 Rupix 的控制中心：平时隐藏到托盘常驻，
//! 截图本身发生在全屏浮层（`overlay`），预览标注在独立窗口（`viewer`），
//! 这里只负责发起流程、配置全局热键，以及展示保存历史。
//! 日常使用快捷键截图即可，无需打开本窗口。

use crate::{
    capture::{begin_capture, begin_picker, begin_window_capture},
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

/// 主窗口展示的截图预览（全屏图或裁剪图，原图按比例缩小显示）。
#[derive(Clone)]
pub struct PreviewImage {
    /// 预览图片路径（临时目录）。
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
    /// 当前生效的窗口截图热键字符串。
    window_hotkey: String,
    /// 区域热键录制框。
    region_input: Entity<HotkeyInputState>,
    /// 全屏热键录制框。
    full_input: Entity<HotkeyInputState>,
    /// 取色热键录制框。
    picker_input: Entity<HotkeyInputState>,
    /// 窗口截图热键录制框。
    window_input: Entity<HotkeyInputState>,
    /// 状态栏文本。
    status: SharedString,
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
    /// - `window_hotkey`: 当前生效的窗口截图热键字符串。
    pub fn new(
        _window: &mut Window,
        cx: &mut Context<Self>,
        region_hotkey: String,
        full_hotkey: String,
        picker_hotkey: String,
        window_hotkey: String,
    ) -> Self {
        Self {
            capturing: false,
            region_input: Self::make_input(cx, &region_hotkey),
            full_input: Self::make_input(cx, &full_hotkey),
            picker_input: Self::make_input(cx, &picker_hotkey),
            window_input: Self::make_input(cx, &window_hotkey),
            region_hotkey,
            full_hotkey,
            picker_hotkey,
            window_hotkey,
            status: SharedString::from("就绪"),
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

    /// 新增一条保存记录（标注阶段保存成功后回调，现暂未接入）。
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

    /// 发起窗口截图（托盘 / 热键 / 设置页共用入口）。
    pub fn begin_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // 与截图同状态机，同一时间只允许一轮操作。
        begin_window_capture(self, window, cx);
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
                    HotkeyKind::Window => self.window_hotkey = text.clone(),
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
        let window_hotkey = self.window_hotkey.clone();
        let region_input = self.region_input.clone();
        let full_input = self.full_input.clone();
        let picker_input = self.picker_input.clone();
        let window_input = self.window_input.clone();
        let status = self.status.clone();
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
            // 快捷键设置：区域 / 全屏 / 取色 / 窗口各一张录制卡（录制后自动应用）。
            .child(self.hotkey_card(cx, HotkeyKind::Region, region_input, &region_hotkey))
            .child(self.hotkey_card(cx, HotkeyKind::Full, full_input, &full_hotkey))
            .child(self.hotkey_card(cx, HotkeyKind::Picker, picker_input, &picker_hotkey))
            .child(self.hotkey_card(cx, HotkeyKind::Window, window_input, &window_hotkey))
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
                            .id("rupix-btn-window")
                            .px_3()
                            .py_2()
                            .rounded_md()
                            .text_sm()
                            .cursor_pointer()
                            .bg(if capturing {
                                rgb(0xC0C0C0)
                            } else {
                                rgb(0x8B008B)
                            })
                            .text_color(rgb(0xFFFFFF))
                            .child("窗口截图")
                            .on_click(move |_, window, cx| {
                                v.update(cx, |this, cx| this.begin_window(window, cx));
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
                "区域截图：在全屏遮罩上拖拽框选，松手后进独立预览窗口（可缩放/复制/保存，可底部状态栏查看结果），右键/Esc 取消。取色：在冻结帧上移动取色，点击复制 HEX。本窗口关闭即隐藏到托盘。",
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
