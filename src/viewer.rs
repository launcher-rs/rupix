//! 图片预览与标注窗口：截图完成后独立展示，像图片浏览器一样查看并标注。
//!
//! - 打开时按显示器 75% 区域自适应图片大小，之后窗口固定；
//! - 缩放只改变图片大小（图片居中，超出可拖拽平移），窗口不再跟随变化；
//! - 第一排：缩小 / 放大 / 适应 / 复制 / 保存 / 关闭；
//! - 第二排：选择 / 矩形 / 椭圆 / 箭头 / 画笔 / 马赛克 / 文字 / 撤销 / 重做；
//! - 第三排：颜色 / 线宽 / 文字背景开关 / 操作提示；
//! - 标注画在覆盖图片的 `canvas` 上实时预览，
//!   保存 / 复制时经 `export::render_export` 烧录为同一坐标系的最终图；
//! - 文字工具支持：点击新建、点击已有文字重新编辑、Ctrl+拖动移动。
//! - `Ctrl + 滚轮`、`+` / `-` / `0`、Esc 快捷键齐全。

use std::path::PathBuf;

use ab_glyph::FontVec;
use image::RgbaImage;
use rgpui::{
    Bounds, Context, Div, Entity, FocusHandle, KeyDownEvent, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Path, PathBuilder, Pixels, Render, Rgba, ScrollDelta,
    ScrollWheelEvent, SharedString, TitlebarOptions, Window, WindowBounds, WindowOptions, canvas,
    div, img,
    input_ui::{Input, InputState},
    point,
    prelude::*,
    px, rgb, size,
};

use crate::{
    app::{PreviewImage, RupixApp},
    capture::cap_log,
    export::{copy_png_to_clipboard, load_system_font, png_bytes, render_export},
    shapes::{AnnoColor, PixelRect, Shape},
};

/// 最小缩放（10%）。
const MIN_ZOOM: f32 = 0.1;
/// 最大缩放（800%）。
const MAX_ZOOM: f32 = 8.0;
/// 单次按钮缩放步进（×1.25）。
const ZOOM_STEP: f32 = 1.25;
/// 工具条单排高度（逻辑像素）。
const ROW_H: f32 = 44.0;
/// 工具条总高度（三排：查看 / 工具 / 颜色线宽）。
const TOOLBAR_H: f32 = ROW_H * 3.0;
/// 画笔默认线宽（图片像素）。
const DEFAULT_WIDTH: f32 = 4.0;
/// 文字标注默认字号（图片像素）。
const DEFAULT_TEXT_SIZE: f32 = 28.0;

/// 标注工具（工具条第二排）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum AnnoTool {
    /// 选择 / 拖拽平移。
    Select,
    /// 矩形框。
    Rect,
    /// 椭圆框。
    Ellipse,
    /// 箭头。
    Arrow,
    /// 自由画笔。
    Pen,
    /// 马赛克：矩形区域打码。
    Mosaic,
    /// 文字（点选落点，内容取输入框）。
    Text,
}

/// 工具条动作（按钮与快捷键共用分发，Debug 用于诊断日志）。
#[derive(Clone, Copy, Debug)]
enum ViewerAction {
    /// 放大。
    ZoomIn,
    /// 缩小。
    ZoomOut,
    /// 回到适应窗口。
    Fit,
    /// 复制烧录图到剪贴板。
    Copy,
    /// 保存烧录图到桌面。
    Save,
    /// 关闭窗口。
    Close,
    /// 切换标注工具。
    Tool(AnnoTool),
    /// 撤销一个标注。
    Undo,
    /// 重做一个标注。
    Redo,
    /// 切换标注颜色。
    Color(AnnoColor),
    /// 切换线宽（图片像素）。
    Width(f32),
    /// 切换文字背景显示。
    TextBg,
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
        .min((disp_h * 0.75 - TOOLBAR_H) / preview.h as f32)
        .clamp(MIN_ZOOM, 1.0);
    // 三排工具条较宽，窗口最小 480 逻辑像素，图片居中。
    let win_w = (preview.w as f32 * fit).round().max(480.0);
    let win_h = (preview.h as f32 * fit).round().max(200.0);
    cap_log(&format!(
        "预览窗口打开：img={}x{} fit={fit:.2}",
        preview.w, preview.h
    ));
    let bounds = Bounds::centered(None, size(px(win_w), px(win_h + TOOLBAR_H)), cx);
    let opened = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
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

/// Ctrl+拖动文字时的临时状态。
struct DragText {
    /// 被拖动的 shapes 索引。
    idx: usize,
    /// 鼠标落点相对文字 pos 的偏移（图片像素）。
    offset: (f32, f32),
}

/// 图片预览视图。
struct ImageViewer {
    /// 主视图实体（回写状态 / 最近文件）。
    main: Entity<RupixApp>,
    /// 展示的图片。
    preview: PreviewImage,
    /// 当前缩放。
    zoom: f32,
    /// 适应窗口的缩放（"适应"按钮恢复用）。
    fit_zoom: f32,
    /// 图片区逻辑宽（窗口固定后不变，平移钳制用）。
    area_w: f32,
    /// 图片区逻辑高。
    area_h: f32,
    /// 平移偏移（图片中心相对区域中心的位移，逻辑像素）。
    offset: (f32, f32),
    /// 平移中上一次的鼠标位置（窗口坐标），None 表示未按下。
    pan_last: Option<(f32, f32)>,
    /// 当前标注工具。
    tool: AnnoTool,
    /// 已提交的标注（图片像素坐标）。
    shapes: Vec<Shape>,
    /// 重做栈（撤销的标注暂存于此）。
    redo_stack: Vec<Shape>,
    /// 拖拽中的图形（抬起鼠标才提交）。
    draft: Option<Shape>,
    /// 图形类工具的起点（图片像素），画笔工具不用。
    anchor: Option<(f32, f32)>,
    /// 当前标注颜色。
    color: AnnoColor,
    /// 当前线宽（图片像素）。
    width: f32,
    /// 正在编辑的文字标注（点击后出现，回车/失焦提交）。
    text_editing: Option<TextEdit>,
    /// 文字背景是否显示（默认开启）。
    text_bg: bool,
    /// Ctrl+拖动文字的临时状态。
    drag_text: Option<DragText>,
    /// 缓存的系统字体（文字烧录用，懒加载）。
    font: Option<FontVec>,
    /// 字体是否已尝试加载（避免每次保存都读盘）。
    font_loaded: bool,
    /// 根视图焦点（键盘快捷键用）。
    focus_handle: FocusHandle,
}

/// 文字工具正在编辑的状态：点击图片后出现，回车或失焦提交。
struct TextEdit {
    /// 落点（图片像素坐标）。
    img_pos: (f32, f32),
    /// 输入框实体。
    input: Entity<InputState>,
    /// 输入框在图片容器内的定位坐标（canvas 本地像素）。
    container_pos: (f32, f32),
    /// 重新编辑时原 shapes 索引（None 表示新建）。
    edit_index: Option<usize>,
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
            tool: AnnoTool::Select,
            shapes: Vec::new(),
            redo_stack: Vec::new(),
            draft: None,
            anchor: None,
            color: AnnoColor::RED,
            width: DEFAULT_WIDTH,
            text_editing: None,
            text_bg: true,
            drag_text: None,
            font: None,
            font_loaded: false,
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

    /// 小按钮（第二 / 三排工具用，选中态高亮，id 保证唯一）。
    fn mini_button(
        &self,
        cx: &mut Context<Self>,
        id: &str,
        label: &'static str,
        action: ViewerAction,
        selected: bool,
    ) -> impl IntoElement {
        let view = cx.entity();
        div()
            .id(format!("viewer-mini-{id}"))
            .px_2()
            .py_1()
            .rounded_md()
            .text_xs()
            .cursor_pointer()
            .bg(if selected {
                rgb(0x1E90FF)
            } else {
                rgb(0x3A3A3A)
            })
            .text_color(rgb(0xFFFFFF))
            .child(label)
            .on_click(move |_, window, cx| {
                view.update(cx, |this, cx| this.handle(action, window, cx));
            })
    }

    /// 颜色块（第三排，选中态白色描边）。
    fn color_swatch(&self, cx: &mut Context<Self>, color: AnnoColor) -> impl IntoElement {
        let view = cx.entity();
        let selected = self.color == color;
        let c = color.to_rgpui();
        div()
            .id(format!("viewer-color-{}-{}-{}", color.0, color.1, color.2))
            .w(px(22.))
            .h(px(22.))
            .rounded_sm()
            .cursor_pointer()
            .bg(c)
            .border_2()
            .border_color(if selected {
                rgb(0xFFFFFF)
            } else {
                rgb(0x3A3A3A)
            })
            .on_click(move |_, window, cx| {
                view.update(cx, |this, cx| {
                    this.handle(ViewerAction::Color(color), window, cx)
                });
            })
    }

    /// 工具条动作分发（按钮与快捷键共用）。
    fn handle(&mut self, action: ViewerAction, window: &mut Window, cx: &mut Context<Self>) {
        match action {
            ViewerAction::ZoomIn => self.zoom_by(ZOOM_STEP, cx),
            ViewerAction::ZoomOut => self.zoom_by(1.0 / ZOOM_STEP, cx),
            ViewerAction::Fit => {
                self.zoom = self.fit_zoom;
                self.offset = (0.0, 0.0);
                cx.notify();
            }
            ViewerAction::Copy => self.copy_image(cx),
            ViewerAction::Save => self.save_image(cx),
            ViewerAction::Close => window.remove_window(),
            ViewerAction::Tool(tool) => {
                // 切换工具时丢弃未提交的草稿，提交文字编辑，避免残留。
                if self.text_editing.is_some() {
                    self.commit_text(window, cx);
                }
                self.tool = tool;
                self.draft = None;
                self.anchor = None;
                self.drag_text = None;
                cx.notify();
            }
            ViewerAction::Undo => self.undo(cx),
            ViewerAction::Redo => self.redo(cx),
            ViewerAction::Color(color) => {
                self.color = color;
                cx.notify();
            }
            ViewerAction::Width(width) => {
                self.width = width;
                cx.notify();
            }
            ViewerAction::TextBg => {
                self.text_bg = !self.text_bg;
                cx.notify();
            }
        }
    }

    /// 撤销一个标注（进重做栈）。
    fn undo(&mut self, cx: &mut Context<Self>) {
        if let Some(shape) = self.shapes.pop() {
            self.redo_stack.push(shape);
            cx.notify();
        }
    }

    /// 重做一个标注（从重做栈取回）。
    fn redo(&mut self, cx: &mut Context<Self>) {
        if let Some(shape) = self.redo_stack.pop() {
            self.shapes.push(shape);
            cx.notify();
        }
    }

    /// 窗口坐标转图片像素。
    ///
    /// 鼠标 `position` 是窗口坐标；图片区域从工具栏下方开始，
    /// 图片在区域内居中 + 偏移。直接用布局参数换算。
    fn to_image(&self, pos: rgpui::Point<Pixels>) -> Option<(f32, f32)> {
        if self.zoom <= 0.0 {
            return None;
        }
        let dw = self.preview.w as f32 * self.zoom;
        let dh = self.preview.h as f32 * self.zoom;
        let left = (self.area_w - dw) / 2.0 + self.offset.0;
        let top = (self.area_h - dh) / 2.0 + self.offset.1;
        // 图片左上角在窗口中的位置：x = left，y = 工具栏高 + top。
        let img_left = left;
        let img_top = TOOLBAR_H + top;
        Some((
            (pos.x.as_f32() - img_left) / self.zoom,
            (pos.y.as_f32() - img_top) / self.zoom,
        ))
    }

    /// 点是否落在图片内（含边界）。
    fn in_image(&self, p: (f32, f32)) -> bool {
        p.0 >= 0.0 && p.1 >= 0.0 && p.0 <= self.preview.w as f32 && p.1 <= self.preview.h as f32
    }

    /// 查找最顶层命中指定图片像素点的文字标注索引（从后往前找，后画的在上层）。
    fn hit_test_text(&self, p: (f32, f32)) -> Option<usize> {
        self.shapes
            .iter()
            .enumerate()
            .rev()
            .filter_map(|(i, shape)| {
                if matches!(shape, Shape::Text { .. }) {
                    let bbox = shape.bbox();
                    if bbox.contains(p) { Some(i) } else { None }
                } else {
                    None
                }
            })
            .next()
    }

    /// 左键按下：选择工具走平移，其他工具开始绘制。
    fn on_anno_down(&mut self, ev: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.tool == AnnoTool::Select {
            self.on_pan_down(ev, cx);
            return;
        }
        let img_pt = self.to_image(ev.position);
        let Some(p) = img_pt else {
            return;
        };
        if !self.in_image(p) {
            return;
        }
        // 如果正在编辑文字，先提交当前文字（点击别处 = 确认）。
        if self.text_editing.is_some() {
            self.commit_text(window, cx);
        }

        // ── 文字工具：Ctrl+拖动 / 点击已有文字重新编辑 / 新建文字 ──
        if self.tool == AnnoTool::Text {
            let has_ctrl = ev.modifiers.control;
            if has_ctrl {
                // Ctrl+点击：尝试拖动已有文字。
                if let Some(idx) = self.hit_test_text(p)
                    && let Shape::Text { pos, .. } = &self.shapes[idx]
                {
                    let offset = (p.0 - pos.0, p.1 - pos.1);
                    self.drag_text = Some(DragText { idx, offset });
                    cx.notify();
                    return;
                }
            }
            // 点击已有文字 → 进入重新编辑模式。
            if let Some(idx) = self.hit_test_text(p) {
                let old = self.shapes.remove(idx);
                if let Shape::Text {
                    pos,
                    content,
                    color,
                    ..
                } = old
                {
                    self.color = color;
                    let input = cx.new(|cx| InputState::new(window, cx).placeholder("输入文字"));
                    // 用已有文字填充输入框。
                    input.update(cx, |state, cx| {
                        state.set_value(content.clone(), window, cx);
                    });
                    let container_pos = (pos.0 * self.zoom, pos.1 * self.zoom);
                    self.text_editing = Some(TextEdit {
                        img_pos: pos,
                        input,
                        container_pos,
                        edit_index: Some(idx),
                    });
                    if let Some(editing) = &self.text_editing {
                        editing
                            .input
                            .update(cx, |state, cx| state.focus(window, cx));
                    }
                    cx.notify();
                    return;
                }
            }
            // 普通点击：新建文字。
            self.place_text(p, window, cx);
            return;
        }

        // ── 图形类工具：开始绘制草稿 ──
        let (color, width) = (self.color, self.width);
        self.anchor = Some(p);
        self.draft = Some(match self.tool {
            AnnoTool::Rect => Shape::Rect {
                rect: PixelRect::from_points(p, p),
                color,
                width,
            },
            AnnoTool::Ellipse => Shape::Ellipse {
                rect: PixelRect::from_points(p, p),
                color,
                width,
            },
            AnnoTool::Arrow => Shape::Arrow {
                from: p,
                to: p,
                color,
                width,
            },
            AnnoTool::Pen => Shape::Pen {
                pts: vec![p],
                color,
                width,
            },
            AnnoTool::Mosaic => Shape::Mosaic {
                rect: PixelRect::from_points(p, p),
            },
            AnnoTool::Select | AnnoTool::Text => return,
        });
        cx.notify();
    }

    /// 鼠标移动：Ctrl+拖动文字、选择工具平移、其他工具更新草稿。
    fn on_anno_move(&mut self, ev: &MouseMoveEvent, cx: &mut Context<Self>) {
        // Ctrl+拖动文字：直接更新已有文字坐标。
        if let Some(drag) = &self.drag_text {
            let Some(mut p) = self.to_image(ev.position) else {
                return;
            };
            p.0 = p.0.clamp(0.0, self.preview.w as f32);
            p.1 = p.1.clamp(0.0, self.preview.h as f32);
            if let Some(Shape::Text { pos, .. }) = self.shapes.get_mut(drag.idx) {
                pos.0 = (p.0 - drag.offset.0).max(0.0);
                pos.1 = (p.1 - drag.offset.1).max(0.0);
            }
            cx.notify();
            return;
        }
        // 选择工具：平移。
        if self.tool == AnnoTool::Select {
            self.on_pan_move(ev, cx);
            return;
        }
        let Some(anchor) = self.anchor else {
            return;
        };
        let Some(mut p) = self.to_image(ev.position) else {
            return;
        };
        p.0 = p.0.clamp(0.0, self.preview.w as f32);
        p.1 = p.1.clamp(0.0, self.preview.h as f32);
        match &mut self.draft {
            Some(Shape::Rect { rect, .. })
            | Some(Shape::Ellipse { rect, .. })
            | Some(Shape::Mosaic { rect }) => {
                *rect = PixelRect::from_points(anchor, p);
            }
            Some(Shape::Arrow { to, .. }) => *to = p,
            Some(Shape::Pen { pts, .. }) => {
                if pts.last() != Some(&p) {
                    pts.push(p);
                }
            }
            _ => return,
        }
        cx.notify();
    }

    /// 左键抬起：选择工具结束平移，Ctrl+拖动文字结束，其他工具提交草稿。
    fn on_anno_up(&mut self, cx: &mut Context<Self>) {
        // Ctrl+拖动文字结束。
        if self.drag_text.take().is_some() {
            cx.notify();
            return;
        }
        if self.tool == AnnoTool::Select {
            self.on_pan_up(cx);
            return;
        }
        // 过小的图形视为误触，直接丢弃。
        if let Some(draft) = self.draft.take()
            && draft_valid(&draft)
        {
            self.shapes.push(draft);
            self.redo_stack.clear();
        }
        self.anchor = None;
        cx.notify();
    }

    /// 点击后进入文字编辑模式：在图片上创建 inline 输入框。
    fn place_text(&mut self, img_pos: (f32, f32), window: &mut Window, cx: &mut Context<Self>) {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("输入文字"));
        let container_pos = (img_pos.0 * self.zoom, img_pos.1 * self.zoom);
        self.text_editing = Some(TextEdit {
            img_pos,
            input,
            container_pos,
            edit_index: None,
        });
        if let Some(editing) = &self.text_editing {
            editing
                .input
                .update(cx, |state, cx| state.focus(window, cx));
        }
        cx.notify();
    }

    /// 提交正在编辑的文字：读取内容，放入 shapes。
    fn commit_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editing) = self.text_editing.take() else {
            return;
        };
        let content = editing
            .input
            .read_with(cx, |state, _| state.text().to_string());
        let content = content.trim().to_string();
        if content.is_empty() {
            let fh = self.focus_handle.clone();
            window.focus(&fh, cx);
            cx.notify();
            return;
        }
        let shape = Shape::Text {
            pos: editing.img_pos,
            content,
            color: self.color,
            size: DEFAULT_TEXT_SIZE,
        };
        // 重新编辑时插回原位，否则追加到末尾。
        if let Some(idx) = editing.edit_index {
            let insert_at = idx.min(self.shapes.len());
            self.shapes.insert(insert_at, shape);
        } else {
            self.shapes.push(shape);
            self.redo_stack.clear();
        }
        let fh = self.focus_handle.clone();
        window.focus(&fh, cx);
        cx.notify();
    }

    /// 取消正在编辑的文字。
    fn cancel_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.text_editing.is_some() {
            self.text_editing = None;
            let fh = self.focus_handle.clone();
            window.focus(&fh, cx);
            cx.notify();
        }
    }

    /// 把底图与全部标注烧录为最终图片。
    fn burn_shapes(&mut self) -> Option<RgbaImage> {
        let base = image::open(&self.preview.path).ok()?.into_rgba8();
        if !self.font_loaded {
            self.font = load_system_font();
            self.font_loaded = true;
        }
        let full = PixelRect::new(0.0, 0.0, base.width() as f32, base.height() as f32);
        Some(render_export(&base, full, &self.shapes, &self.font))
    }

    /// 复制烧录图到系统剪贴板。
    fn copy_image(&mut self, cx: &mut Context<Self>) {
        let main = self.main.clone();
        if self.shapes.is_empty() {
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
        } else {
            match self.burn_shapes().and_then(|img| png_bytes(&img)) {
                Some(png) => match copy_png_to_clipboard(cx, &png) {
                    Ok(()) => main.update(cx, |state, cx| {
                        state.set_status("已复制标注图到剪贴板", cx);
                    }),
                    Err(error) => main.update(cx, |state, cx| {
                        state.set_status(&format!("复制失败：{error}"), cx);
                    }),
                },
                None => main.update(cx, |state, cx| {
                    state.set_status("导出图片失败", cx);
                }),
            }
        }
        cx.refresh_windows();
    }

    /// 保存烧录图到桌面并记入最近文件。
    fn save_image(&mut self, cx: &mut Context<Self>) {
        let main = self.main.clone();
        let dst = save_dir().join(format!(
            "rupix-{}.png",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0)
        ));
        let saved = if self.shapes.is_empty() {
            std::fs::copy(&self.preview.path, &dst).is_ok()
        } else {
            self.burn_shapes()
                .and_then(|img| image::DynamicImage::ImageRgba8(img).save(&dst).ok())
                .is_some()
        };
        if saved {
            let msg = format!("已保存 {}", dst.display());
            main.update(cx, |state, cx| {
                state.push_recent(dst, msg, cx);
            });
        } else {
            main.update(cx, |state, cx| {
                state.set_status("保存失败", cx);
            });
        }
        cx.refresh_windows();
    }

    /// 按系数缩放。
    fn zoom_by(&mut self, factor: f32, cx: &mut Context<Self>) {
        self.zoom = (self.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        self.clamp_offset();
        cx.notify();
    }

    /// 钳制平移：图比区域小时强制居中；图大时保证视口中心始终压在图上。
    fn clamp_offset(&mut self) {
        let dw = self.preview.w as f32 * self.zoom;
        let dh = self.preview.h as f32 * self.zoom;
        let (mut ox, mut oy) = self.offset;
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

    /// 左键按下：开始平移。
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
}

/// 保存目录：桌面优先（兼容 OneDrive 接管），其次图片库，最后临时目录。
fn save_dir() -> PathBuf {
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

/// 草稿有效性：过小的图形视为误触，不提交。
fn draft_valid(shape: &Shape) -> bool {
    match shape {
        Shape::Pen { pts, .. } | Shape::Highlighter { pts, .. } => {
            pts.len() >= 2 && pts.first() != pts.last()
        }
        Shape::Rect { rect, .. } | Shape::Ellipse { rect, .. } | Shape::Mosaic { rect } => {
            !rect.is_empty()
        }
        Shape::Arrow { from, to, .. } => (to.0 - from.0).hypot(to.1 - from.1) > 2.0,
        Shape::Text { .. } => true,
    }
}

/// 矢量标注转 canvas 绘制路径。
///
/// `paint_path` 使用窗口坐标，不会自动叠加 canvas 自身的布局偏移。
/// 因此必须使用 rgpui 在绘制回调里传入的 `bounds.origin` 作为图片容器真实左上角，
/// 避免手写工具栏高度或图片区居中偏移时漏算。
fn shape_path(
    shape: &Shape,
    zoom: f32,
    origin_x: f32,
    origin_y: f32,
) -> Option<(Path<Pixels>, Rgba)> {
    let pt = |x: f32, y: f32| point(px(origin_x + x * zoom), px(origin_y + y * zoom));
    let stroker = |width: f32| PathBuilder::stroke(px((width * zoom).max(1.0)));
    match shape {
        Shape::Pen { pts, color, width } | Shape::Highlighter { pts, color, width } => {
            if pts.len() < 2 {
                return None;
            }
            let mut builder = stroker(*width);
            for (i, (x, y)) in pts.iter().enumerate() {
                if i == 0 {
                    builder.move_to(pt(*x, *y));
                } else {
                    builder.line_to(pt(*x, *y));
                }
            }
            let path = builder.build().ok()?;
            let mut c = color.to_rgpui();
            if matches!(shape, Shape::Highlighter { .. }) {
                c.a = 0.4;
            }
            Some((path, c))
        }
        Shape::Rect { rect, color, width } => {
            let mut builder = stroker(*width);
            builder.move_to(pt(rect.x, rect.y));
            builder.line_to(pt(rect.x + rect.w, rect.y));
            builder.line_to(pt(rect.x + rect.w, rect.y + rect.h));
            builder.line_to(pt(rect.x, rect.y + rect.h));
            builder.close();
            Some((builder.build().ok()?, color.to_rgpui()))
        }
        Shape::Ellipse { rect, color, width } => {
            let (cx, cy) = (rect.x + rect.w / 2.0, rect.y + rect.h / 2.0);
            let (rx, ry) = (rect.w / 2.0, rect.h / 2.0);
            if rx <= 0.0 || ry <= 0.0 {
                return None;
            }
            let mut builder = stroker(*width);
            for i in 0..=48 {
                let t = i as f32 / 48.0 * std::f32::consts::TAU;
                let p = pt(cx + rx * t.cos(), cy + ry * t.sin());
                if i == 0 {
                    builder.move_to(p);
                } else {
                    builder.line_to(p);
                }
            }
            Some((builder.build().ok()?, color.to_rgpui()))
        }
        Shape::Arrow {
            from,
            to,
            color,
            width,
        } => {
            let (dx, dy) = (to.0 - from.0, to.1 - from.1);
            let len = dx.hypot(dy);
            if len < 2.0 {
                return None;
            }
            let ang = dy.atan2(dx);
            let head = 12.0 + width * 2.0;
            let wing = 0.42;
            let base = ang + std::f32::consts::PI;
            let w1 = (
                to.0 + head * (base - wing).cos(),
                to.1 + head * (base - wing).sin(),
            );
            let w2 = (
                to.0 + head * (base + wing).cos(),
                to.1 + head * (base + wing).sin(),
            );
            let mut builder = stroker(*width);
            builder.move_to(pt(from.0, from.1));
            builder.line_to(pt(to.0, to.1));
            builder.move_to(pt(w1.0, w1.1));
            builder.line_to(pt(to.0, to.1));
            builder.line_to(pt(w2.0, w2.1));
            Some((builder.build().ok()?, color.to_rgpui()))
        }
        // 马赛克：画半透明矩形 + 十字网格预览。
        Shape::Mosaic { rect } => {
            let (rx, ry) = (rect.w * zoom, rect.h * zoom);
            if rx <= 0.0 || ry <= 0.0 {
                return None;
            }
            let mut builder = PathBuilder::stroke(px(1.0));
            // 半透明外框。
            builder.move_to(pt(rect.x, rect.y));
            builder.line_to(pt(rect.x + rect.w, rect.y));
            builder.line_to(pt(rect.x + rect.w, rect.y + rect.h));
            builder.line_to(pt(rect.x, rect.y + rect.h));
            builder.close();
            let path = builder.build().ok()?;
            let color = Rgba {
                r: 1.0,
                g: 1.0,
                b: 1.0,
                a: 0.35,
            };
            Some((path, color))
        }
        // 文字走 div 层，画布跳过。
        Shape::Text { .. } => None,
    }
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
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                if this.text_editing.is_some() {
                    match ev.keystroke.key.as_str() {
                        "enter" => {
                            this.commit_text(window, cx);
                            return;
                        }
                        "escape" => {
                            this.cancel_text(window, cx);
                            return;
                        }
                        _ => {}
                    }
                }
                let modi = &ev.keystroke.modifiers;
                match ev.keystroke.key.as_str() {
                    "escape" => window.remove_window(),
                    "+" | "=" => this.zoom_by(ZOOM_STEP, cx),
                    "-" | "_" => this.zoom_by(1.0 / ZOOM_STEP, cx),
                    "0" => this.handle(ViewerAction::Fit, window, cx),
                    "z" if modi.control && !modi.shift => {
                        this.handle(ViewerAction::Undo, window, cx);
                    }
                    "z" if modi.control && modi.shift => {
                        this.handle(ViewerAction::Redo, window, cx);
                    }
                    "y" if modi.control => {
                        this.handle(ViewerAction::Redo, window, cx);
                    }
                    _ => {}
                }
            }))
            // 第一排：查看。
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .h(px(ROW_H))
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
                            .child(format!("{w} × {h}（Ctrl+滚轮缩放）")),
                    ),
            )
            // 第二排：标注工具 + 撤销重做。
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .h(px(ROW_H))
                    .px_3()
                    .bg(rgb(0x2D2D2D))
                    .child(self.mini_button(
                        cx,
                        "tool-select",
                        "选择",
                        ViewerAction::Tool(AnnoTool::Select),
                        self.tool == AnnoTool::Select,
                    ))
                    .child(self.mini_button(
                        cx,
                        "tool-rect",
                        "矩形",
                        ViewerAction::Tool(AnnoTool::Rect),
                        self.tool == AnnoTool::Rect,
                    ))
                    .child(self.mini_button(
                        cx,
                        "tool-ellipse",
                        "椭圆",
                        ViewerAction::Tool(AnnoTool::Ellipse),
                        self.tool == AnnoTool::Ellipse,
                    ))
                    .child(self.mini_button(
                        cx,
                        "tool-arrow",
                        "箭头",
                        ViewerAction::Tool(AnnoTool::Arrow),
                        self.tool == AnnoTool::Arrow,
                    ))
                    .child(self.mini_button(
                        cx,
                        "tool-pen",
                        "画笔",
                        ViewerAction::Tool(AnnoTool::Pen),
                        self.tool == AnnoTool::Pen,
                    ))
                    .child(self.mini_button(
                        cx,
                        "tool-mosaic",
                        "马赛克",
                        ViewerAction::Tool(AnnoTool::Mosaic),
                        self.tool == AnnoTool::Mosaic,
                    ))
                    .child(self.mini_button(
                        cx,
                        "tool-text",
                        "文字",
                        ViewerAction::Tool(AnnoTool::Text),
                        self.tool == AnnoTool::Text,
                    ))
                    .child(self.mini_button(cx, "undo", "撤销", ViewerAction::Undo, false))
                    .child(self.mini_button(cx, "redo", "重做", ViewerAction::Redo, false)),
            )
            // 第三排：颜色 + 线宽 + 背景开关 + 提示。
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .h(px(ROW_H))
                    .px_3()
                    .bg(rgb(0x2D2D2D))
                    .child(self.color_swatch(cx, AnnoColor::RED))
                    .child(self.color_swatch(cx, AnnoColor::YELLOW))
                    .child(self.color_swatch(cx, AnnoColor::GREEN))
                    .child(self.color_swatch(cx, AnnoColor::BLUE))
                    .child(self.color_swatch(cx, AnnoColor::WHITE))
                    .child(self.color_swatch(cx, AnnoColor::BLACK))
                    .child(self.color_swatch(cx, AnnoColor::ORANGE))
                    .child(self.color_swatch(cx, AnnoColor::CYAN))
                    .child(self.mini_button(
                        cx,
                        "w-thin",
                        "细",
                        ViewerAction::Width(2.0),
                        self.width == 2.0,
                    ))
                    .child(self.mini_button(
                        cx,
                        "w-mid",
                        "中",
                        ViewerAction::Width(5.0),
                        self.width == 5.0,
                    ))
                    .child(self.mini_button(
                        cx,
                        "w-thick",
                        "粗",
                        ViewerAction::Width(9.0),
                        self.width == 9.0,
                    ))
                    .child(self.mini_button(
                        cx,
                        "text-bg",
                        if self.text_bg {
                            "有底色"
                        } else {
                            "无底色"
                        },
                        ViewerAction::TextBg,
                        self.text_bg,
                    ))
                    .child(
                        if self.tool == AnnoTool::Text && self.text_editing.is_some() {
                            div()
                                .text_xs()
                                .text_color(rgb(0x999999))
                                .child("回车确认 · Esc 取消")
                        } else if self.tool == AnnoTool::Text {
                            div()
                                .text_xs()
                                .text_color(rgb(0x999999))
                                .child("点击新建 · Ctrl+拖动移动 · 点击已有文字编辑")
                        } else {
                            div()
                                .text_xs()
                                .text_color(rgb(0x999999))
                                .child("选择工具后点击图片放置")
                        },
                    ),
            )
            // 图片区。
            .child({
                let left = (self.area_w - dw) / 2.0 + self.offset.0;
                let top = (self.area_h - dh) / 2.0 + self.offset.1;
                let zoom = self.zoom;
                let area = div().flex_1().relative().overflow_hidden();
                let area = match self.tool {
                    AnnoTool::Select => {
                        if self.pan_last.is_some() || self.drag_text.is_some() {
                            area.cursor_grabbing()
                        } else {
                            area.cursor_grab()
                        }
                    }
                    AnnoTool::Text => area.cursor_text(),
                    AnnoTool::Rect
                    | AnnoTool::Ellipse
                    | AnnoTool::Arrow
                    | AnnoTool::Pen
                    | AnnoTool::Mosaic => area.cursor_crosshair(),
                };
                area.on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, ev: &MouseDownEvent, window, cx| {
                        this.on_anno_down(ev, window, cx);
                    }),
                )
                .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _window, cx| {
                    this.on_anno_move(ev, cx);
                }))
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(|this, _ev: &MouseUpEvent, _window, cx| {
                        this.on_anno_up(cx);
                    }),
                )
                .on_mouse_up_out(
                    MouseButton::Left,
                    cx.listener(|this, _ev: &MouseUpEvent, _window, cx| {
                        this.on_anno_up(cx);
                    }),
                )
                .on_scroll_wheel(cx.listener(|this, ev: &ScrollWheelEvent, _window, cx| {
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
                        .w(px(dw))
                        .h(px(dh))
                        .relative()
                        // 底图。
                        .child(img(self.preview.path.clone()).w(px(dw)).h(px(dh)))
                        // 文字标注层。
                        .children({
                            let mut marks: Vec<Div> = Vec::new();
                            let text_bg = self.text_bg;
                            for shape in &self.shapes {
                                if let Shape::Text {
                                    pos,
                                    content,
                                    color,
                                    size,
                                } = shape
                                {
                                    let sz = size * zoom;
                                    let mut d = div()
                                        .absolute()
                                        .left(px(pos.0 * zoom))
                                        .top(px(pos.1 * zoom))
                                        .px_2()
                                        .py_1()
                                        .rounded_sm()
                                        .text_color(color.to_rgpui())
                                        .text_size(px(sz))
                                        .child(content.clone());
                                    if text_bg {
                                        d = d.bg(Rgba {
                                            r: 0.0,
                                            g: 0.0,
                                            b: 0.0,
                                            a: 0.55,
                                        });
                                    }
                                    marks.push(d);
                                }
                            }
                            // 正在编辑的 inline 输入框。
                            if let Some(editing) = &self.text_editing {
                                marks.push(
                                    div()
                                        .absolute()
                                        .left(px(editing.container_pos.0))
                                        .top(px(editing.container_pos.1))
                                        .min_w(px(180.0))
                                        .max_w(px(400.0))
                                        // 阻止输入框点击冒泡到图片区，避免触发 commit + place。
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(|_, _ev, _, cx| {
                                                cx.stop_propagation();
                                            }),
                                        )
                                        .child(Input::new(&editing.input)),
                                );
                            }
                            marks
                        })
                        // 矢量标注画布。
                        .child({
                            let paint_shapes = self.shapes.clone();
                            let paint_draft = self.draft.clone();
                            canvas(
                                move |bounds, _, _| bounds,
                                move |_bounds, paint_bounds, window, _| {
                                    let origin = paint_bounds.origin;
                                    for shape in paint_shapes.iter().chain(paint_draft.iter()) {
                                        if let Some((path, color)) = shape_path(
                                            shape,
                                            zoom,
                                            origin.x.as_f32(),
                                            origin.y.as_f32(),
                                        ) {
                                            window.paint_path(path, color);
                                        }
                                    }
                                },
                            )
                            .absolute()
                            // canvas 必须显式锚定到图片容器左上角，否则会按静态位置排到 img 后面。
                            .left(px(0.0))
                            .top(px(0.0))
                            .w(px(dw))
                            .h(px(dh))
                        }),
                )
            })
    }
}
