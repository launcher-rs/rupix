//! 主窗口模块：快捷键设置、截图入口、状态与最近保存记录。
//!
//! 主窗口是 Rupix 的控制中心：截图本身发生在全屏浮层（`overlay`），
//! 这里只负责发起流程、配置全局热键，以及展示保存历史。

use std::path::PathBuf;

use rgpui::{
    Context, Entity, Render, SharedString, Window,
    components::{HotkeyInput, HotkeyInputState, HotkeyValue},
    div,
    prelude::*,
    rgb,
};

use crate::{
    capture::begin_capture,
    hotkey::{apply_global_hotkey, hotkey_value_to_string, save_hotkey_string},
};

/// 主视图状态。
pub struct RupixApp {
    /// 是否正在截取中（防止重复推流）。
    pub capturing: bool,
    /// 当前生效的全局热键字符串。
    hotkey_string: String,
    /// 快捷键录制框。
    hotkey_input: Entity<HotkeyInputState>,
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
    /// - `hotkey`: 当前生效的全局热键字符串。
    pub fn new(_window: &mut Window, cx: &mut Context<Self>, hotkey: String) -> Self {
        // 录制框预填当前热键，所见即所得。
        let preset = rgpui::Keystroke::parse(&hotkey)
            .ok()
            .and_then(|ks| HotkeyValue::from_keystroke(&ks));
        let hotkey_input = match preset {
            Some(value) => cx.new(|cx| HotkeyInputState::with_hotkey(cx, value)),
            None => cx.new(HotkeyInputState::new),
        };
        Self {
            capturing: false,
            hotkey_string: hotkey,
            hotkey_input,
            status: SharedString::from("就绪"),
            recents: Vec::new(),
        }
    }

    /// 设置状态栏并刷新。
    pub fn set_status(&mut self, msg: &str, cx: &mut Context<Self>) {
        self.status = SharedString::from(msg.to_string());
        cx.notify();
    }

    /// 新增一条保存记录（供浮层保存成功后回调）。
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

    /// 快捷键录制框变化时自动应用新的全局热键。
    fn on_hotkey_recorded(&mut self, value: Option<HotkeyValue>, cx: &mut Context<Self>) {
        let Some(value) = value else {
            // 清空录制：保持旧热键生效，仅提示。
            self.set_status("已清空录制，旧快捷键继续生效", cx);
            return;
        };
        let text = hotkey_value_to_string(&value);
        match apply_global_hotkey(cx, &text) {
            Ok(()) => {
                self.hotkey_string = text.clone();
                save_hotkey_string(&text);
                self.set_status(&format!("全局截图快捷键：{text}"), cx);
            }
            Err(error) => {
                self.set_status(&error, cx);
            }
        }
    }
}

impl Render for RupixApp {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        let hotkey_text = self.hotkey_string.clone();
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
            // 标题。
            .child(
                div()
                    .flex()
                    .items_baseline()
                    .gap_2()
                    .child(div().text_xl().child("Rupix · 拾景"))
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(0x888888))
                            .child("看到 → 截取 → 留存"),
                    ),
            )
            // 快捷键设置行：录制框 + 说明（录制后自动应用）。
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .p_3()
                    .rounded_md()
                    .bg(rgb(0xFFFFFF))
                    .child(
                        div()
                            .text_sm()
                            .child("全局截图快捷键（点击框，按下组合键即自动生效）"),
                    )
                    .child({
                        let v = view.clone();
                        HotkeyInput::new(self.hotkey_input.clone())
                            .placeholder("点这里，然后按下快捷键")
                            .recording_text("正在录制…")
                            .on_change(move |value, _, cx| {
                                v.update(cx, |this, cx| this.on_hotkey_recorded(value, cx));
                            })
                    })
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(0x888888))
                            .child(format!("当前：{hotkey_text}（失焦可用，托盘常驻）")),
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
                    }),
            )
            // 使用提示。
            .child(div().text_xs().text_color(rgb(0x888888)).child(
                "浮层中拖拽框选，右键取消；标注后保存或复制。关闭本窗口不会退出，将最小化到托盘。",
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
