//! 截图保存提示：屏幕右下角闪一下的小胶囊。
//!
//! Rupix 默认「截完不弹编辑窗」（见 [`crate::app::load_open_editor`]），但「到底截到
//! 没有」必须有反馈，否则用户会以为没截上、连按几次热键。这里用一枚独立的角标窗口
//! 交代结果：
//!
//! - **不抢焦点**（`focus: false`）：键盘焦点留在原程序，截完可以直接 `Ctrl+V`；
//! - **自动消失**：1.6 秒后移除窗口，不占屏幕、不进任务栏；
//! - **不进下一张截图**：与截图浮层同样排除出系统抓屏（Windows `WDA_EXCLUDEFROMCAPTURE`），
//!   免得连按两次快捷键时把上一次的提示胶囊也截进去。

use std::{path::Path, time::Duration};

use rgpui::{
    App, Bounds, Context, Render, SharedString, Window, WindowBackgroundAppearance, WindowBounds,
    WindowKind, WindowOptions, div, point, prelude::*, px, rgb, size,
};

use crate::{i18n::tr, window::exclude_from_capture};

/// 提示停留时长（毫秒）：够看清一行字，又不至于挡事。
const TOAST_MS: u64 = 1600;

/// 胶囊宽度（逻辑像素）。
const TOAST_W: f32 = 300.0;

/// 胶囊高度（逻辑像素）：两行小字 + 内边距。
const TOAST_H: f32 = 56.0;

/// 距屏幕右下角的边距（逻辑像素）。
const TOAST_MARGIN: f32 = 24.0;

/// 保存提示视图（一枚静态胶囊，没有交互）。
struct SaveToast {
    /// 主行：结果（已保存 / 已保存并复制）。
    title: SharedString,
    /// 副行：文件名（太长时裁掉，不换行撑破窗口）。
    detail: SharedString,
}

impl Render for SaveToast {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().p_1().child(
            div()
                .flex()
                .flex_col()
                .justify_center()
                .gap_0p5()
                .size_full()
                .px_3()
                .rounded_lg()
                // 深灰底 + 白字：在亮色桌面与暗色桌面上都看得清。
                .bg(rgb(0x1F2937))
                .child(
                    div()
                        .text_sm()
                        .text_color(rgb(0xFFFFFF))
                        .whitespace_nowrap()
                        .overflow_hidden()
                        .child(self.title.clone()),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(0xD1D5DB))
                        .whitespace_nowrap()
                        .overflow_hidden()
                        .child(self.detail.clone()),
                ),
        )
    }
}

/// 显示「截图已保存」提示。
///
/// # 参数
/// - `cx`: 应用上下文（开窗口 + 挂自动关闭定时器）。
/// - `path`: 自动保存后的图片路径（只取文件名展示）。
/// - `copied`: 是否已顺手写进剪贴板（决定主行文案）。
pub fn show_saved(cx: &mut App, path: &Path, copied: bool) {
    let title = if copied {
        tr("toast.saved_copied")
    } else {
        tr("toast.saved")
    };
    // 只显示文件名：完整路径太长，胶囊里放不下，用户也不靠它定位。
    let detail = path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default();
    show(cx, title, detail);
}

/// 在屏幕右下角开一枚提示，到时自动关掉。
///
/// # 参数
/// - `title`: 主行文案。
/// - `detail`: 副行文案（一般是文件名）。
fn show(cx: &mut App, title: String, detail: String) {
    let (x, y) = toast_pos(cx);
    let opened = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds {
                origin: point(px(x), px(y)),
                size: size(px(TOAST_W), px(TOAST_H)),
            })),
            titlebar: None,
            // 置顶浮层。Windows 用透明背景让圆角外真正透出桌面；
            // X11 上 Transparent 会换 32 位 visual，在软件合成分支上整窗变全透明
            // （见 CHANGELOG 里浮层那条修复），所以非 Windows 退化为不透明底。
            kind: WindowKind::Overlay,
            window_background: if cfg!(target_os = "windows") {
                WindowBackgroundAppearance::Transparent
            } else {
                WindowBackgroundAppearance::Opaque
            },
            // 不抢焦点：截完要接着在原程序里粘贴，焦点不能被提示抢走。
            focus: false,
            show: true,
            ..Default::default()
        },
        move |window, cx| {
            // 连按两次快捷键时，不要把自己截进下一张图。
            exclude_from_capture(window);
            // 这里**不要**调 `set_mouse_passthrough(true)`：它会给窗口补上
            // `WS_EX_LAYERED`，而本窗口走的是 DirectComposition（无重定向位图），
            // 两者一起用时窗口只剩内容、不参与屏幕合成——提示会彻底看不见。
            // 提示只活 1.6 秒且贴右下角，被点到也不影响截图结果，不值得为穿透牺牲可见性。
            cx.new(|_| SaveToast {
                title: SharedString::from(title),
                detail: SharedString::from(detail),
            })
        },
    );
    // 建窗失败只影响这条提示，不打断截图流程（图已经存好、也复制好了）。
    let Ok(handle) = opened else {
        return;
    };
    cx.spawn(async move |cx| {
        cx.background_executor()
            .timer(Duration::from_millis(TOAST_MS))
            .await;
        // 跨窗口移除要在 App 上下文里做（与托盘 / 浮层的关闭同一模式）。
        cx.update(|cx| {
            let _ = cx.update_window(handle.into(), |_, window, _| window.remove_window());
        });
    })
    .detach();
}

/// 计算提示窗口左上角坐标：主显示器右下角，留出边距。
///
/// 用显示器 `origin + size` 而不是假定原点为 0：多显示器时主屏可能不是最左边那块。
fn toast_pos(cx: &App) -> (f32, f32) {
    let (ox, oy, w, h) = cx
        .primary_display()
        .map(|display| {
            let bounds = display.bounds();
            (
                bounds.origin.x.as_f32(),
                bounds.origin.y.as_f32(),
                bounds.size.width.as_f32(),
                bounds.size.height.as_f32(),
            )
        })
        .unwrap_or((0.0, 0.0, 1920.0, 1080.0));
    (
        ox + w - TOAST_W - TOAST_MARGIN,
        oy + h - TOAST_H - TOAST_MARGIN,
    )
}
