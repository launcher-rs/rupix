//! 图片预览窗口：截图完成后独立展示，像图片浏览器一样查看。
//!
//! - 打开时按显示器 75% 区域自适应图片大小，之后窗口固定；
//! - 缩放只改变图片大小（图片居中，超出可拖拽平移），窗口不再跟随变化；
//! - 工具条支持缩小 / 放大 / 适应 / 复制 / 保存 / 关闭；
//! - `Ctrl + 滚轮`、`+` / `-` / `0`、Esc 快捷键齐全；
//! - 后续标注工具（矩形 / 箭头 / 文字）直接加在这个窗口里。

use std::path::PathBuf;

use rgpui::{
    Bounds, Context, Entity, FocusHandle, KeyDownEvent, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Render, ScrollDelta, ScrollWheelEvent, SharedString,
    TitlebarOptions, Window, WindowBounds, WindowOptions, div, img, prelude::*, px, rgb, size,
};

use crate::{
    app::{PreviewImage, RupixApp},
    capture::cap_log,
    export::copy_png_to_clipboard,
};

/// 最小缩放（10%）。
const MIN_ZOOM: f32 = 0.1;
/// 最大缩放（800%）。
const MAX_ZOOM: f32 = 8.0;
/// 单次按钮缩放步进（×1.25）。
const ZOOM_STEP: f32 = 1.25;
/// 顶部工具条高度（逻辑像素，窗口尺寸计算用）。
const BAR_H: f32 = 44.0;

/// 工具条动作（按钮与快捷键共用分发）。
#[derive(Clone, Copy)]
enum ViewerAction {
    /// 放大。
    ZoomIn,
    /// 缩小。
    ZoomOut,
    /// 回到适应窗口。
    Fit,
    /// 复制 PNG 到剪贴板。
    Copy,
    /// 保存到桌面。
    Save,
    /// 关闭窗口。
    Close,
}

/// 打开图片预览窗口。
///
/// # 参数
/// - `main`: 主视图实体（复制 / 保存后回写状态用）。
/// - `cx`: 调用方上下文（全屏截图与区域浮层各调一次，泛型兼容）。
/// - `preview`: 要展示的图片（路径 + 原图尺寸）。
pub fn open_viewer<T: 'static>(main: Entity<RupixApp>, cx: &mut Context<T>, preview: PreviewImage) {
    // 自适应缩放：图片完整落进 75% 显示区域，只缩小不放大。
    let (disp_w, disp_h) = cx
        .primary_display()
        .map(|d| {
            let s = d.bounds().size;
            (s.width.as_f32(), s.height.as_f32())
        })
        .unwrap_or((1920.0, 1080.0));
    let fit = ((disp_w * 0.75) / preview.w as f32)
        .min((disp_h * 0.75 - BAR_H) / preview.h as f32)
        .clamp(MIN_ZOOM, 1.0);
    let win_w = (preview.w as f32 * fit).round().max(320.0);
    let win_h = (preview.h as f32 * fit).round().max(200.0);
    cap_log(&format!(
        "预览窗口打开：img={}x{} fit={fit:.2}",
        preview.w, preview.h
    ));
    let bounds = Bounds::centered(None, size(px(win_w), px(win_h + BAR_H)), cx);
    let opened = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            // 系统标题栏 + 自定义标题（TitlebarOptions 承载标题）。
            titlebar: Some(TitlebarOptions {
                title: Some(SharedString::from(format!(
                    "Rupix 预览（{} × {}）",
                    preview.w, preview.h
                ))),
                ..Default::default()
            }),
            ..Default::default()
        },
        move |window, cx| {
            let view = cx.new(|cx| {
                ImageViewer::new(window, cx, main.clone(), preview.clone(), fit, win_w, win_h)
            });
            // 打开即聚焦，Esc / 快捷键立即可用。
            let handle = view.read(cx).focus_handle.clone();
            window.focus(&handle, cx);
            view
        },
    );
    if let Err(error) = opened {
        cap_log(&format!("预览窗口打开失败：{error:?}"));
    }
}

/// 图片预览视图。
struct ImageViewer {
    /// 主视图实体（回写状态 / 最近文件）。
    main: Entity<RupixApp>,
    /// 展示的图片。
    preview: PreviewImage,
    /// 当前缩放。
    zoom: f32,
    /// 适应窗口的缩放（“适应”按钮恢复用）。
    fit_zoom: f32,
    /// 图片区逻辑宽（窗口固定后不变，平移钳制用）。
    area_w: f32,
    /// 图片区逻辑高。
    area_h: f32,
    /// 平移偏移（图片中心相对区域中心的位移，逻辑像素）。
    offset: (f32, f32),
    /// 平移中上一次的鼠标位置（窗口坐标），None 表示未按下。
    pan_last: Option<(f32, f32)>,
    /// 根视图焦点（键盘快捷键用）。
    focus_handle: FocusHandle,
}

impl ImageViewer {
    /// 创建预览视图。
    fn new(
        _window: &mut Window,
        cx: &mut Context<Self>,
        main: Entity<RupixApp>,
        preview: PreviewImage,
        fit_zoom: f32,
        area_w: f32,
        area_h: f32,
    ) -> Self {
        Self {
            main,
            preview,
            zoom: fit_zoom,
            fit_zoom,
            area_w,
            area_h,
            offset: (0.0, 0.0),
            pan_last: None,
            focus_handle: cx.focus_handle(),
        }
    }

    /// 工具条按钮（文案 + 动作，id 保证唯一）。
    fn bar_button(
        &self,
        cx: &mut Context<Self>,
        label: &'static str,
        action: ViewerAction,
    ) -> impl IntoElement {
        let view = cx.entity();
        div()
            .id(format!("viewer-btn-{label}"))
            .px_2()
            .py_1()
            .rounded_md()
            .text_sm()
            .cursor_pointer()
            .bg(rgb(0x3A3A3A))
            .text_color(rgb(0xFFFFFF))
            .child(label)
            .on_click(move |_, window, cx| {
                view.update(cx, |this, cx| this.handle(action, window, cx));
            })
    }

    /// 工具条动作分发（按钮与快捷键共用）。
    fn handle(&mut self, action: ViewerAction, window: &mut Window, cx: &mut Context<Self>) {
        match action {
            ViewerAction::ZoomIn => self.zoom_by(ZOOM_STEP, cx),
            ViewerAction::ZoomOut => self.zoom_by(1.0 / ZOOM_STEP, cx),
            ViewerAction::Fit => {
                // 回到适应：缩放复位 + 居中。
                self.zoom = self.fit_zoom;
                self.offset = (0.0, 0.0);
                cx.notify();
            }
            ViewerAction::Copy => self.copy_image(cx),
            ViewerAction::Save => self.save_image(cx),
            ViewerAction::Close => window.remove_window(),
        }
    }

    /// 按系数缩放（只改变图片大小，窗口固定；缩到能完整显示时自动回中）。
    fn zoom_by(&mut self, factor: f32, cx: &mut Context<Self>) {
        self.zoom = (self.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        self.clamp_offset();
        cx.notify();
    }

    /// 钳制平移：图比区域小时强制居中；图大时保证视口中心始终压在图上（图丢不了）。
    fn clamp_offset(&mut self) {
        let dw = self.preview.w as f32 * self.zoom;
        let dh = self.preview.h as f32 * self.zoom;
        let (mut ox, mut oy) = self.offset;
        // 图片半宽半高即最大偏移，视口中心恒在图内。
        ox = if dw <= self.area_w {
            0.0
        } else {
            ox.clamp(-dw / 2.0, dw / 2.0)
        };
        oy = if dh <= self.area_h {
            0.0
        } else {
            oy.clamp(-dh / 2.0, dh / 2.0)
        };
        self.offset = (ox, oy);
    }

    /// 左键按下：开始平移（记下鼠标位置）。
    fn on_pan_down(&mut self, ev: &MouseDownEvent, cx: &mut Context<Self>) {
        self.pan_last = Some((ev.position.x.as_f32(), ev.position.y.as_f32()));
        cx.notify();
    }

    /// 鼠标移动：平移中按位移累加偏移。
    fn on_pan_move(&mut self, ev: &MouseMoveEvent, cx: &mut Context<Self>) {
        let Some((lx, ly)) = self.pan_last else {
            return;
        };
        let (x, y) = (ev.position.x.as_f32(), ev.position.y.as_f32());
        let (ox, oy) = self.offset;
        self.offset = (ox + x - lx, oy + y - ly);
        self.pan_last = Some((x, y));
        self.clamp_offset();
        cx.notify();
    }

    /// 左键抬起：结束平移。
    fn on_pan_up(&mut self, cx: &mut Context<Self>) {
        self.pan_last = None;
        cx.notify();
    }

    /// 复制 PNG 到系统剪贴板。
    fn copy_image(&mut self, cx: &mut Context<Self>) {
        let main = self.main.clone();
        match std::fs::read(&self.preview.path) {
            Ok(png) => match copy_png_to_clipboard(cx, &png) {
                Ok(()) => main.update(cx, |state, cx| {
                    state.set_status("已复制到剪贴板", cx);
                }),
                Err(error) => main.update(cx, |state, cx| {
                    state.set_status(&format!("复制失败：{error}"), cx);
                }),
            },
            Err(error) => main.update(cx, |state, cx| {
                state.set_status(&format!("读取图片失败：{error}"), cx);
            }),
        }
        // 跨窗口回写主状态后强制刷新，保证状态栏落定。
        cx.refresh_windows();
    }

    /// 保存到桌面并记入最近文件。
    fn save_image(&mut self, cx: &mut Context<Self>) {
        let main = self.main.clone();
        let dst = save_dir().join(format!(
            "rupix-{}.png",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0)
        ));
        match std::fs::copy(&self.preview.path, &dst) {
            Ok(_) => {
                let msg = format!("已保存 {}", dst.display());
                main.update(cx, |state, cx| {
                    state.push_recent(dst, msg, cx);
                });
            }
            Err(error) => main.update(cx, |state, cx| {
                state.set_status(&format!("保存失败：{error}"), cx);
            }),
        }
        // 跨窗口回写（最近文件 / 状态）后强制刷新，保证主窗口落定。
        cx.refresh_windows();
    }
}

/// 保存目录：桌面优先（兼容 OneDrive 接管），其次图片库，最后临时目录。
fn save_dir() -> PathBuf {
    // 候选目录依次试，只取真实存在的，避免“路径不存在”保存失败。
    let candidates = [
        std::env::var("USERPROFILE")
            .map(|home| PathBuf::from(home).join("Desktop"))
            .ok(),
        std::env::var("OneDrive")
            .map(|root| PathBuf::from(root).join("Desktop"))
            .ok(),
        std::env::var("USERPROFILE")
            .map(|home| PathBuf::from(home).join("Pictures"))
            .ok(),
    ];
    for dir in candidates.into_iter().flatten() {
        if dir.is_dir() {
            return dir;
        }
    }
    std::env::temp_dir()
}

impl Render for ImageViewer {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (w, h) = (self.preview.w, self.preview.h);
        let dw = (w as f32 * self.zoom).round().max(1.0);
        let dh = (h as f32 * self.zoom).round().max(1.0);
        let percent = format!("{}%", (self.zoom * 100.0).round() as u32);

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(0x1E1E1E))
            .text_color(rgb(0xDDDDDD))
            // 根视图可聚焦，快捷键立即可用。
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                match ev.keystroke.key.as_str() {
                    "escape" => window.remove_window(),
                    "+" | "=" => this.zoom_by(ZOOM_STEP, cx),
                    "-" | "_" => this.zoom_by(1.0 / ZOOM_STEP, cx),
                    "0" => this.handle(ViewerAction::Fit, window, cx),
                    _ => {}
                }
            }))
            // 顶部工具条。
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .h(px(BAR_H))
                    .px_3()
                    .bg(rgb(0x2D2D2D))
                    .child(self.bar_button(cx, "−", ViewerAction::ZoomOut))
                    .child(div().text_sm().w(px(52.)).child(percent))
                    .child(self.bar_button(cx, "+", ViewerAction::ZoomIn))
                    .child(self.bar_button(cx, "适应", ViewerAction::Fit))
                    .child(self.bar_button(cx, "复制", ViewerAction::Copy))
                    .child(self.bar_button(cx, "保存", ViewerAction::Save))
                    .child(self.bar_button(cx, "关闭", ViewerAction::Close))
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(0x999999))
                            .child(format!("{w} × {h}（Ctrl+滚轮缩放，拖拽平移）")),
                    ),
            )
            // 图片区：固定窗口内居中，大图可左键拖拽平移，滚轮缩放。
            // 定位用绝对坐标：居中基准 + 平移偏移（图片区尺寸打开时即固定）。
            .child({
                let left = (self.area_w - dw) / 2.0 + self.offset.0;
                let top = (self.area_h - dh) / 2.0 + self.offset.1;
                div()
                    .flex_1()
                    .relative()
                    .overflow_hidden()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, ev: &MouseDownEvent, _window, cx| {
                            this.on_pan_down(ev, cx);
                        }),
                    )
                    .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _window, cx| {
                        this.on_pan_move(ev, cx);
                    }))
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this, _ev: &MouseUpEvent, _window, cx| {
                            this.on_pan_up(cx);
                        }),
                    )
                    .on_mouse_up_out(
                        MouseButton::Left,
                        cx.listener(|this, _ev: &MouseUpEvent, _window, cx| {
                            this.on_pan_up(cx);
                        }),
                    )
                    .on_scroll_wheel(cx.listener(|this, ev: &ScrollWheelEvent, _window, cx| {
                        // 只响应 Ctrl+滚轮，普通滚轮留空（后续可做纵向平移）。
                        if !ev.modifiers.control {
                            return;
                        }
                        let dy = match ev.delta {
                            ScrollDelta::Pixels(p) => p.y.as_f32(),
                            ScrollDelta::Lines(p) => p.y * 40.0,
                        };
                        if dy > 0.0 {
                            this.zoom_by(ZOOM_STEP, cx);
                        } else if dy < 0.0 {
                            this.zoom_by(1.0 / ZOOM_STEP, cx);
                        }
                    }))
                    .child(
                        div()
                            .absolute()
                            .left(px(left))
                            .top(px(top))
                            .child(img(self.preview.path.clone()).w(px(dw)).h(px(dh))),
                    )
            })
    }
}
