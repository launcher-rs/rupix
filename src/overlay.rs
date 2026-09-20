//! 全屏标注浮层：Snipaste 式“拖拽选区 → 右下工具条标注 → 保存 / 复制”。
//!
//! 交互形态对照 `temp/1.png`（Snipaste 截图态）：
//! - 全屏显示截取的首帧，选区外自动变暗，选区带八手柄与 `宽 × 高 px` 徽标；
//! - 工具条吸附在选区右下方：选择 / 矩形 / 椭圆 / 箭头 / 画笔 / 荧光笔 /
//!   马赛克 / 文字 / 橡皮擦 / 撤销 / 重做 / 取消 / 保存 / 复制；
//! - 右键任意处取消本次截图。

use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

use ab_glyph::FontVec;
use rgpui::{
    Bounds, Context, Entity, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels,
    Point, Render, SharedString, Window, WindowBackgroundAppearance, WindowBounds, WindowKind,
    WindowOptions, canvas, div, img,
    input_ui::{Input, InputState},
    point,
    prelude::*,
    px, quad, rgb,
};

use crate::{
    app::RupixApp,
    export::{
        DEFAULT_TEXT_SIZE, copy_png_to_clipboard, load_rgba, load_system_font, png_bytes,
        render_export,
    },
    shapes::{AnnoColor, PixelRect, Shape},
};

/// 首帧捕获完成后传给浮层的数据。
pub struct PendingCapture {
    /// 首帧落盘的临时图片路径。
    pub frame_path: PathBuf,
    /// 全图宽度（图片像素）。
    pub img_w: u32,
    /// 全图高度（图片像素）。
    pub img_h: u32,
    /// 浮层窗口尺寸（取主显示器边界）。
    pub display_bounds: Bounds<Pixels>,
    /// 是否全屏截图（打开即全选）。
    pub fullscreen: bool,
}

/// 当前标注工具（工具条第一排）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Tool {
    /// 选择 / 移动选区。
    Select,
    /// 矩形框。
    Rect,
    /// 椭圆框。
    Ellipse,
    /// 箭头。
    Arrow,
    /// 自由画笔。
    Pen,
    /// 荧光笔（半透明粗线）。
    Highlighter,
    /// 马赛克。
    Mosaic,
    /// 文字。
    Text,
    /// 橡皮擦（点删标注）。
    Eraser,
}

/// 选区八手柄位置。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Handle {
    /// 左上。
    NW,
    /// 上中。
    N,
    /// 右上。
    NE,
    /// 右中。
    E,
    /// 右下。
    SE,
    /// 下中。
    S,
    /// 左下。
    SW,
    /// 左中。
    W,
}

/// 正在进行的鼠标操作。
enum DragOp {
    /// 新建选区（锚点，图片像素）。
    NewSel { anchor: (f32, f32) },
    /// 移动选区（鼠标点与选区左上的偏移）。
    MoveSel { dx: f32, dy: f32 },
    /// 拖拽手柄调整选区。
    ResizeSel { handle: Handle },
    /// 正在绘制图形（起点，图片像素）。
    Draw { start: (f32, f32) },
}

/// 打开全屏标注浮层。
///
/// # 参数
/// - `main`: 主视图实体（保存后回写最近文件、复制后提示状态用）。
/// - `cx`: 调用方上下文（可解引用为 `App`）。
/// - `pending`: 首帧图片与窗口尺寸。
pub fn open_overlay(main: Entity<RupixApp>, cx: &mut Context<RupixApp>, pending: PendingCapture) {
    // 图片尺寸兜底：捕获时没读到就现场解码一次。
    let (img_w, img_h) = if pending.img_w > 0 && pending.img_h > 0 {
        (pending.img_w, pending.img_h)
    } else {
        load_rgba(&pending.frame_path)
            .map(|img| (img.width(), img.height()))
            .unwrap_or((800, 600))
    };
    let PendingCapture {
        frame_path,
        display_bounds,
        fullscreen,
        ..
    } = pending;
    cx.open_window(
        WindowOptions {
            // 无边框置顶透明浮层，盖住全屏做选区交互（参考 desktop_pet 示例）。
            titlebar: None,
            window_bounds: Some(WindowBounds::Windowed(display_bounds)),
            window_background: WindowBackgroundAppearance::Transparent,
            kind: WindowKind::Overlay,
            focus: true,
            ..Default::default()
        },
        move |window, cx| {
            cx.new(|cx| {
                OverlayView::new(
                    window,
                    cx,
                    main.clone(),
                    frame_path.clone(),
                    img_w,
                    img_h,
                    fullscreen,
                )
            })
        },
    )
    .ok();
}

/// 全屏标注浮层视图。
struct OverlayView {
    /// 主视图实体（回写状态 / 最近文件）。
    main: Entity<RupixApp>,
    /// 底图路径（`img` 元素直接显示）。
    frame_path: PathBuf,
    /// 全图宽（图片像素）。
    img_w: u32,
    /// 全图高（图片像素）。
    img_h: u32,
    /// 当前选区（图片像素），None 表示尚未框选。
    sel: Option<PixelRect>,
    /// 正在进行的鼠标操作。
    drag: Option<DragOp>,
    /// 当前工具。
    tool: Tool,
    /// 已提交的标注。
    shapes: Vec<Shape>,
    /// 重做栈（撤销的标注暂存于此）。
    redo_stack: Vec<Shape>,
    /// 正在绘制的图形（抬起鼠标才提交）。
    cur_shape: Option<Shape>,
    /// 当前颜色。
    color: AnnoColor,
    /// 当前线宽（图片像素）。
    width: f32,
    /// 文字内容输入框。
    text_input: Entity<InputState>,
    /// 最近一帧画布的窗口坐标（鼠标换算与绘制共用）。
    canvas_bounds: Arc<Mutex<Bounds<Pixels>>>,
    /// 工具条矩形（窗口坐标，工具条上的点击不触发选区操作）。
    toolbar_rect: Option<Bounds<Pixels>>,
    /// 缓存的系统字体（文字导出用，懒加载）。
    font: Option<FontVec>,
    /// 字体是否已尝试加载（避免每次保存都读盘）。
    font_loaded: bool,
    /// 创建浮层时的设备像素比（高 DPI 下底图按物理像素 1:1 显示）。
    device_scale: f32,
    /// 状态提示。
    status: SharedString,
}

impl OverlayView {
    /// 创建浮层视图。
    #[allow(clippy::too_many_arguments)]
    fn new(
        window: &mut Window,
        cx: &mut Context<Self>,
        main: Entity<RupixApp>,
        frame_path: PathBuf,
        img_w: u32,
        img_h: u32,
        fullscreen: bool,
    ) -> Self {
        // 全屏截图直接全选，跳过手动框选。
        let sel = fullscreen.then(|| PixelRect::new(0.0, 0.0, img_w as f32, img_h as f32));
        let text_input = cx.new(|cx| InputState::new(window, cx).placeholder("文字内容，点图放置"));
        Self {
            main,
            frame_path,
            img_w,
            img_h,
            sel,
            drag: None,
            tool: Tool::Select,
            shapes: Vec::new(),
            redo_stack: Vec::new(),
            cur_shape: None,
            color: AnnoColor::RED,
            width: 3.0,
            text_input,
            canvas_bounds: Arc::new(Mutex::new(Bounds::default())),
            toolbar_rect: None,
            font: None,
            font_loaded: false,
            // 设备像素比决定容器逻辑尺寸，保证图片物理像素 1:1 显示。
            device_scale: window.scale_factor().max(1.0),
            status: SharedString::from("拖拽框选区域，右键取消"),
        }
    }

    /// 当前画布缩放：逻辑像素 / 图片像素（高 DPI 下 img 显示会被放大）。
    fn scale(&self) -> f32 {
        let bounds = self.canvas_bounds.lock().map(|b| *b).unwrap_or_default();
        if self.img_w == 0 || bounds.size.width.as_f32() <= 0.0 {
            1.0
        } else {
            (bounds.size.width.as_f32() / self.img_w as f32).max(0.01)
        }
    }

    /// 窗口坐标转图片像素坐标。
    fn to_image(&self, p: Point<Pixels>) -> (f32, f32) {
        let bounds = self.canvas_bounds.lock().map(|b| *b).unwrap_or_default();
        let scale = self.scale();
        (
            (p.x.as_f32() - bounds.origin.x.as_f32()) / scale,
            (p.y.as_f32() - bounds.origin.y.as_f32()) / scale,
        )
    }

    /// 鼠标左键按下：选区操作或开始绘制。
    fn on_left_down(&mut self, ev: &MouseDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        // 落在工具条上的是按钮点击，不做选区 / 绘制。
        if let Some(rect) = self.toolbar_rect
            && ev.position.x.as_f32() >= rect.origin.x.as_f32()
            && ev.position.y.as_f32() >= rect.origin.y.as_f32()
            && ev.position.x.as_f32() <= rect.origin.x.as_f32() + rect.size.width.as_f32()
            && ev.position.y.as_f32() <= rect.origin.y.as_f32() + rect.size.height.as_f32()
        {
            return;
        }
        let p = self.to_image(ev.position);
        // 越界点击直接忽略（图片容器外的暗部）。
        if p.0 < 0.0 || p.1 < 0.0 || p.0 > self.img_w as f32 || p.1 > self.img_h as f32 {
            return;
        }
        match self.tool {
            Tool::Select => {
                // 先看手柄，再看选区内部，最后才是新建。
                if let Some(sel) = self.sel {
                    if let Some(handle) = self.hit_handle(p) {
                        self.drag = Some(DragOp::ResizeSel { handle });
                        cx.notify();
                        return;
                    }
                    if sel.contains(p) {
                        self.drag = Some(DragOp::MoveSel {
                            dx: p.0 - sel.x,
                            dy: p.1 - sel.y,
                        });
                        cx.notify();
                        return;
                    }
                }
                self.drag = Some(DragOp::NewSel { anchor: p });
                self.sel = Some(PixelRect::from_points(p, p));
                cx.notify();
            }
            Tool::Text => {
                // 文字工具单击即放置，不需要拖拽。
                let content = self
                    .text_input
                    .read_with(cx, |state, _| state.text().to_string())
                    .trim()
                    .to_string();
                let content = if content.is_empty() {
                    "文字".to_string()
                } else {
                    content
                };
                self.push_shape(Shape::Text {
                    pos: p,
                    content,
                    color: self.color,
                    size: DEFAULT_TEXT_SIZE,
                });
                self.status = SharedString::from("已放置文字，可继续标注");
                cx.notify();
            }
            Tool::Eraser => {
                // 自顶向下命中测试，删掉第一个包住点击点的标注。
                if let Some(idx) = self.shapes.iter().rposition(|s| s.bbox().contains(p)) {
                    let removed = self.shapes.remove(idx);
                    // 被删的进重做栈，按“重做”可恢复。
                    self.redo_stack.push(removed);
                    self.status = SharedString::from("已删除一个标注");
                    cx.notify();
                }
            }
            _ => {
                self.drag = Some(DragOp::Draw { start: p });
                self.cur_shape = self.fresh_shape(p, p);
                cx.notify();
            }
        }
    }

    /// 鼠标移动：更新选区或刷新绘制中图形。
    fn on_mouse_move(&mut self, ev: &MouseMoveEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let p = self.to_image(ev.position);
        // 钳制到图片范围内，避免选区飞出底图。
        let p = (
            p.0.clamp(0.0, self.img_w as f32),
            p.1.clamp(0.0, self.img_h as f32),
        );
        match self.drag.take() {
            None => {}
            Some(DragOp::NewSel { anchor }) => {
                self.sel = Some(PixelRect::from_points(anchor, p));
                self.drag = Some(DragOp::NewSel { anchor });
                cx.notify();
            }
            Some(DragOp::MoveSel { dx, dy }) => {
                if let Some(sel) = self.sel {
                    let (mut x, mut y) = (p.0 - dx, p.1 - dy);
                    // 移动时保证选区整体不出图。
                    x = x.clamp(0.0, (self.img_w as f32 - sel.w).max(0.0));
                    y = y.clamp(0.0, (self.img_h as f32 - sel.h).max(0.0));
                    self.sel = Some(PixelRect::new(x, y, sel.w, sel.h));
                }
                self.drag = Some(DragOp::MoveSel { dx, dy });
                cx.notify();
            }
            Some(DragOp::ResizeSel { handle }) => {
                if let Some(sel) = self.sel {
                    self.sel = Some(resize_rect(
                        sel,
                        handle,
                        p,
                        self.img_w as f32,
                        self.img_h as f32,
                    ));
                }
                self.drag = Some(DragOp::ResizeSel { handle });
                cx.notify();
            }
            Some(DragOp::Draw { start }) => {
                self.cur_shape = self.fresh_shape(start, p);
                self.drag = Some(DragOp::Draw { start });
                cx.notify();
            }
        }
    }

    /// 鼠标左键抬起：提交选区或图形。
    fn on_left_up(&mut self, _ev: &MouseUpEvent, _window: &mut Window, cx: &mut Context<Self>) {
        match self.drag.take() {
            // 空选区直接丢弃，避免后续导出切出 1px 图。
            Some(DragOp::NewSel { .. }) | Some(DragOp::ResizeSel { .. }) => {
                if self.sel.is_some_and(|sel| sel.is_empty()) {
                    self.sel = None;
                }
                cx.notify();
            }
            Some(DragOp::Draw { .. }) => {
                if let Some(shape) = self.cur_shape.take() {
                    // 点一下没拖出长度的图形直接丢弃。
                    if !shape_is_empty(&shape) {
                        self.push_shape(shape);
                    }
                }
                cx.notify();
            }
            _ => {}
        }
    }

    /// 右键按下：取消本次截图并关闭浮层。
    fn on_right_down(&mut self, _ev: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.main.update(cx, |state, cx| {
            state.set_status("已取消截图", cx);
        });
        window.remove_window();
    }

    /// 提交一个标注，同时清空重做栈（新操作使重做失效）。
    fn push_shape(&mut self, shape: Shape) {
        self.shapes.push(shape);
        self.redo_stack.clear();
    }

    /// 按当前工具与起止点构造新图形。
    fn fresh_shape(&self, start: (f32, f32), end: (f32, f32)) -> Option<Shape> {
        match self.tool {
            Tool::Pen => Some(Shape::Pen {
                pts: vec![start, end],
                color: self.color,
                width: self.width,
            }),
            Tool::Highlighter => Some(Shape::Highlighter {
                pts: vec![start, end],
                color: self.color,
                width: self.width,
            }),
            Tool::Rect => Some(Shape::Rect {
                rect: PixelRect::from_points(start, end),
                color: self.color,
                width: self.width,
            }),
            Tool::Ellipse => Some(Shape::Ellipse {
                rect: PixelRect::from_points(start, end),
                color: self.color,
                width: self.width,
            }),
            Tool::Arrow => Some(Shape::Arrow {
                from: start,
                to: end,
                color: self.color,
                width: self.width,
            }),
            Tool::Mosaic => Some(Shape::Mosaic {
                rect: PixelRect::from_points(start, end),
            }),
            // 选择 / 文字 / 橡皮擦不走绘制流程。
            Tool::Select | Tool::Text | Tool::Eraser => None,
        }
    }

    /// 绘制中追加轨迹点（画笔 / 荧光笔需要连续点列）。
    fn extend_stroke(&mut self, p: (f32, f32)) {
        match self.cur_shape.as_mut() {
            Some(Shape::Pen { pts, .. }) | Some(Shape::Highlighter { pts, .. }) => pts.push(p),
            Some(Shape::Rect { rect, .. })
            | Some(Shape::Ellipse { rect, .. })
            | Some(Shape::Mosaic { rect }) => {
                // 矩形类以 drag 起点为锚更新对角。
                if let Some(DragOp::Draw { start }) = self.drag {
                    *rect = PixelRect::from_points(start, p);
                }
            }
            Some(Shape::Arrow { to, .. }) => *to = p,
            _ => {}
        }
    }

    /// 八手柄命中测试（半径 8 逻辑像素换算到图片像素）。
    fn hit_handle(&self, p: (f32, f32)) -> Option<Handle> {
        let sel = self.sel?;
        let r = 8.0 / self.scale();
        for (handle, hp) in handle_points(sel) {
            if (hp.0 - p.0).abs() <= r && (hp.1 - p.1).abs() <= r {
                return Some(handle);
            }
        }
        None
    }

    /// 撤销：弹出最后一个标注进重做栈。
    fn undo(&mut self, cx: &mut Context<Self>) {
        if let Some(shape) = self.shapes.pop() {
            self.redo_stack.push(shape);
            self.status = SharedString::from("已撤销");
            cx.notify();
        }
    }

    /// 重做：把撤销的标注拿回来。
    fn redo(&mut self, cx: &mut Context<Self>) {
        if let Some(shape) = self.redo_stack.pop() {
            self.shapes.push(shape);
            self.status = SharedString::from("已重做");
            cx.notify();
        }
    }

    /// 保存：裁剪选区 + 烧录标注 → 另存对话框 → 写盘。
    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let out = self.render_current();
        let Some(png) = png_bytes(&out) else {
            self.status = SharedString::from("导出图片失败");
            cx.notify();
            return;
        };
        // 默认保存到程序所在目录，文件名带时间戳。
        let dir = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|p| p.to_path_buf()))
            .unwrap_or_else(std::env::temp_dir);
        let name = format!(
            "rupix-{}.png",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0)
        );
        let prompt_rx = cx.prompt_for_new_path(&dir, Some(&name));
        let this = cx.entity();
        let main = self.main.clone();
        cx.spawn_in(window, async move |_this, cx| {
            // 只有“用户选了路径”才写盘，取消对话框则静默返回，不打扰标注流程。
            if let Ok(Ok(Some(path))) = prompt_rx.await {
                // 写盘在后台线程做，避免大图卡 UI。
                let saved = cx
                    .background_executor()
                    .spawn(async move { std::fs::write(&path, &png).map(|_| path) })
                    .await;
                match saved {
                    Ok(path) => {
                        let msg = format!("已保存：{}", path.display());
                        this.update(cx, |view, cx| {
                            view.status = SharedString::from(msg.clone());
                            cx.notify();
                        });
                        main.update(cx, |state, cx| {
                            state.push_recent(path, msg, cx);
                        });
                    }
                    Err(error) => {
                        this.update(cx, |view, cx| {
                            view.status = SharedString::from(format!("保存失败：{error}"));
                            cx.notify();
                        });
                    }
                }
            }
        })
        .detach();
    }

    /// 复制：导出 PNG → 剪贴板 → 关闭浮层。
    fn copy_and_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let out = self.render_current();
        match png_bytes(&out) {
            Some(png) => match copy_png_to_clipboard(cx, &png) {
                Ok(()) => {
                    self.main.update(cx, |state, cx| {
                        state.set_status("已复制到剪贴板", cx);
                    });
                    window.remove_window();
                }
                Err(error) => {
                    self.status = SharedString::from(error);
                    cx.notify();
                }
            },
            None => {
                self.status = SharedString::from("导出图片失败");
                cx.notify();
            }
        }
    }

    /// 当前选区（无选区视为全图）+ 全部标注合成为位图。
    fn render_current(&mut self) -> image::RgbaImage {
        // 字体懒加载一次，多次保存不重复读盘。
        if !self.font_loaded {
            self.font = load_system_font();
            self.font_loaded = true;
        }
        let full = PixelRect::new(0.0, 0.0, self.img_w as f32, self.img_h as f32);
        let sel = self.sel.unwrap_or(full);
        // 底图读不到时返回空白图，保证流程不 panic。
        let base = load_rgba(&self.frame_path).unwrap_or_else(|| {
            image::RgbaImage::from_pixel(sel.w as u32, sel.h as u32, image::Rgba([20, 20, 20, 255]))
        });
        render_export(&base, sel, &self.shapes, &self.font)
    }
}

impl Render for OverlayView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let scale = self.scale();
        let img_w = self.img_w as f32;
        let img_h = self.img_h as f32;
        // 容器逻辑尺寸 = 物理像素 / 设备比，底图按物理像素 1:1 显示。
        let cont_w = img_w / self.device_scale;
        let cont_h = img_h / self.device_scale;
        let sel = self.sel;
        let status = self.status.clone();
        let shapes = self.shapes.clone();
        let cur_shape = self.cur_shape.clone();
        let bounds_shared = self.canvas_bounds.clone();

        // 工具条吸附在选区右下方（Snipaste 式），坐标为容器相对坐标。
        let (tb_left, tb_top) = toolbar_position(sel, scale, cont_w, cont_h);
        // 换算到窗口坐标存下来，供鼠标按下时做工具条点击过滤。
        let canvas_origin = self
            .canvas_bounds
            .lock()
            .map(|b| b.origin)
            .unwrap_or_default();
        self.toolbar_rect = Some(Bounds {
            origin: point(
                px(canvas_origin.x.as_f32() + tb_left),
                px(canvas_origin.y.as_f32() + tb_top),
            ),
            size: rgpui::size(px(660.), px(84.)),
        });

        let view = cx.entity();
        let paint_img_w = self.img_w;
        let paint_shapes = shapes.clone();
        let paint_cur = cur_shape.clone();
        let paint_sel = sel;
        let paint_color_sel = rgb(0x1E90FF);

        div()
            .size_full()
            .relative()
            // 状态提示悬浮在左上角。
            .child(
                div()
                    .absolute()
                    .left(px(12.))
                    .top(px(12.))
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .bg(rgb(0x000000))
                    .text_color(rgb(0xFFFFFF))
                    .text_xs()
                    .child(status.clone()),
            )
            .child(
                div()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .relative()
                            .w(px(cont_w))
                            .h(px(cont_h))
                            // 底图：截取的首帧。
                            .child(img(self.frame_path.clone()).w(px(cont_w)).h(px(cont_h)))
                            // 标注画布：选区变暗 + 边框手柄 + 矢量图形。
                            .child(
                                canvas(
                                    move |bounds, _, _| {
                                        // 每帧回传画布窗口坐标，供鼠标换算用。
                                        *bounds_shared.lock().unwrap() = bounds;
                                        bounds
                                    },
                                    move |bounds, _, window, _| {
                                        paint_overlay(
                                            window,
                                            bounds,
                                            paint_img_w,
                                            paint_sel,
                                            &paint_shapes,
                                            &paint_cur,
                                            paint_color_sel,
                                        );
                                    },
                                )
                                .absolute()
                                .size_full(),
                            )
                            // 文字标注：绝对定位的文本（位置与导出烧录一致）。
                            .children({
                                let mut marks = Vec::new();
                                for shape in &shapes {
                                    if let Shape::Text {
                                        pos,
                                        content,
                                        color,
                                        size,
                                    } = shape
                                    {
                                        marks.push(
                                            div()
                                                .absolute()
                                                .left(px(pos.0 * scale))
                                                .top(px(pos.1 * scale))
                                                .text_color(color.to_rgpui())
                                                .text_size(px(size * scale))
                                                .child(content.clone()),
                                        );
                                    }
                                }
                                marks
                            })
                            // 尺寸徽标：选区左上 `宽 × 高 px`（对标 Snipaste 黑色徽标）。
                            .when_some(sel, |el, sel| {
                                el.child(
                                    div()
                                        .absolute()
                                        .left(px(sel.x * scale))
                                        .top(px((sel.y * scale - 30.0).max(0.0)))
                                        .px_2()
                                        .py_1()
                                        .rounded_md()
                                        .bg(rgb(0x000000))
                                        .text_color(rgb(0xFFFFFF))
                                        .text_xs()
                                        .child(format!("{} × {} px", sel.w as i32, sel.h as i32)),
                                )
                            })
                            // 工具条：吸附选区右下。
                            .when_some(
                                sel.or(Some(PixelRect::new(0.0, 0.0, img_w, img_h))),
                                |el, _| el.child(self.toolbar(view.clone(), tb_left, tb_top)),
                            ),
                    )
                    // 鼠标交互挂在图片容器上（坐标换算以画布 bounds 为准）。
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, ev: &MouseDownEvent, window, cx| {
                            this.on_left_down(ev, window, cx);
                        }),
                    )
                    .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, window, cx| {
                        // 绘制中追加轨迹点，否则按选区拖拽处理。
                        if this.drag.is_some()
                            && matches!(
                                this.cur_shape,
                                Some(Shape::Pen { .. }) | Some(Shape::Highlighter { .. })
                            )
                        {
                            let p = this.to_image(ev.position);
                            this.extend_stroke(p);
                            cx.notify();
                        } else {
                            this.on_mouse_move(ev, window, cx);
                        }
                    }))
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this, ev: &MouseUpEvent, window, cx| {
                            this.on_left_up(ev, window, cx);
                        }),
                    ),
            )
            // 右键任意处取消（挂在最外层，暗部也能点）。
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, ev: &MouseDownEvent, window, cx| {
                    this.on_right_down(ev, window, cx);
                }),
            )
    }
}

/// 画布绘制：选区外变暗 + 选区边框手柄 + 全部矢量图形。
#[allow(clippy::too_many_arguments)]
fn paint_overlay(
    window: &mut Window,
    bounds: Bounds<Pixels>,
    img_w: u32,
    sel: Option<PixelRect>,
    shapes: &[Shape],
    cur_shape: &Option<Shape>,
    sel_blue: rgpui::Rgba,
) {
    // 缩放：画布逻辑宽度 / 图片像素宽度。
    let scale = if img_w == 0 {
        1.0
    } else {
        bounds.size.width.as_f32() / img_w as f32
    }
    .max(0.01);
    // 图片像素转画布窗口坐标。
    let to_paint = |x: f32, y: f32| {
        point(
            px(bounds.origin.x.as_f32() + x * scale),
            px(bounds.origin.y.as_f32() + y * scale),
        )
    };

    // 选区外变暗（Snipaste 式遮罩）：无选区则整图半暗。
    let mut dim = rgb(0x000000);
    dim.a = 0.45;
    let full = Bounds {
        origin: bounds.origin,
        size: bounds.size,
    };
    match sel {
        Some(sel) => {
            let (sx, sy) = (
                bounds.origin.x.as_f32() + sel.x * scale,
                bounds.origin.y.as_f32() + sel.y * scale,
            );
            let (sw, sh) = (sel.w * scale, sel.h * scale);
            // 上 / 下 / 左 / 右四块遮罩。
            let rects = [
                Bounds {
                    origin: bounds.origin,
                    size: rgpui::size(bounds.size.width, px(sy - bounds.origin.y.as_f32())),
                },
                Bounds {
                    origin: point(bounds.origin.x, px(sy + sh)),
                    size: rgpui::size(
                        bounds.size.width,
                        px(bounds.origin.y.as_f32() + bounds.size.height.as_f32() - (sy + sh)),
                    ),
                },
                Bounds {
                    origin: point(bounds.origin.x, px(sy)),
                    size: rgpui::size(px(sx - bounds.origin.x.as_f32()), px(sh)),
                },
                Bounds {
                    origin: point(px(sx + sw), px(sy)),
                    size: rgpui::size(
                        px(bounds.origin.x.as_f32() + bounds.size.width.as_f32() - (sx + sw)),
                        px(sh),
                    ),
                },
            ];
            for rect in rects {
                if rect.size.width.as_f32() > 0.5 && rect.size.height.as_f32() > 0.5 {
                    window.paint_quad(quad(rect, px(0.), dim, px(0.), dim, Default::default()));
                }
            }
        }
        None => {
            window.paint_quad(quad(full, px(0.), dim, px(0.), dim, Default::default()));
        }
    }

    // 全部已提交图形 + 绘制中图形。
    for shape in shapes.iter().chain(cur_shape.iter()) {
        paint_shape(window, shape, &to_paint, scale);
    }

    // 选区边框 + 八手柄。
    if let Some(sel) = sel {
        let p0 = to_paint(sel.x, sel.y);
        let p1 = to_paint(sel.x + sel.w, sel.y + sel.h);
        let mut builder = rgpui::PathBuilder::stroke(px(2.));
        builder.move_to(p0);
        builder.line_to(point(p1.x, p0.y));
        builder.line_to(p1);
        builder.line_to(point(p0.x, p1.y));
        builder.close();
        if let Ok(path) = builder.build() {
            window.paint_path(path, sel_blue);
        }
        // 手柄：白底蓝边小方块。
        for (_, hp) in handle_points(sel) {
            let c = to_paint(hp.0, hp.1);
            let rect = Bounds {
                origin: point(px(c.x.as_f32() - 5.0), px(c.y.as_f32() - 5.0)),
                size: rgpui::size(px(10.), px(10.)),
            };
            window.paint_quad(quad(
                rect,
                px(2.),
                rgb(0xFFFFFF),
                px(1.5),
                sel_blue,
                Default::default(),
            ));
        }
    }
}

/// 绘制单个矢量图形到画布。
fn paint_shape(
    window: &mut Window,
    shape: &Shape,
    to_paint: &dyn Fn(f32, f32) -> Point<Pixels>,
    scale: f32,
) {
    // 画布线宽：图片像素线宽 × 缩放，最小 1 逻辑像素。
    let lw = |w: f32| px((w * scale).max(1.0));
    match shape {
        Shape::Pen { pts, color, width } => {
            stroke_points(window, pts, to_paint, lw(*width), color.to_rgpui());
        }
        Shape::Highlighter { pts, color, width } => {
            // 屏幕预览同样半透明加粗，与导出一致。
            let translucent = AnnoColor(color.0, color.1, color.2, 110);
            stroke_points(
                window,
                pts,
                to_paint,
                lw(width * 3.0),
                translucent.to_rgpui(),
            );
        }
        Shape::Rect { rect, color, width } => {
            let p0 = to_paint(rect.x, rect.y);
            let p1 = to_paint(rect.x + rect.w, rect.y + rect.h);
            let mut builder = rgpui::PathBuilder::stroke(lw(*width));
            builder.move_to(p0);
            builder.line_to(point(p1.x, p0.y));
            builder.line_to(p1);
            builder.line_to(point(p0.x, p1.y));
            builder.close();
            if let Ok(path) = builder.build() {
                window.paint_path(path, color.to_rgpui());
            }
        }
        Shape::Ellipse { rect, color, width } => {
            let pts = ellipse_points(*rect, 64);
            stroke_points(window, &pts, to_paint, lw(*width), color.to_rgpui());
        }
        Shape::Arrow {
            from,
            to,
            color,
            width,
        } => {
            stroke_points(
                window,
                &[*from, *to],
                to_paint,
                lw(*width),
                color.to_rgpui(),
            );
            // 头部：与导出同构（25 度夹角）。
            let dx = to.0 - from.0;
            let dy = to.1 - from.1;
            let len = (dx * dx + dy * dy).sqrt();
            if len >= 2.0 {
                let head = (12.0 + width * 2.0).min(len);
                let angle = dy.atan2(dx);
                let spread = 25.0_f32.to_radians();
                for sign in [-1.0, 1.0] {
                    let a = angle + std::f32::consts::PI + sign * spread;
                    let tip = (to.0 + head * a.cos(), to.1 + head * a.sin());
                    stroke_points(window, &[*to, tip], to_paint, lw(*width), color.to_rgpui());
                }
            }
        }
        Shape::Mosaic { rect } => {
            // 屏幕预览用半透明白灰块表示打码，导出时才是真马赛克。
            let p0 = to_paint(rect.x, rect.y);
            let p1 = to_paint(rect.x + rect.w, rect.y + rect.h);
            let mut builder = rgpui::PathBuilder::fill();
            builder.move_to(p0);
            builder.line_to(point(p1.x, p0.y));
            builder.line_to(p1);
            builder.line_to(point(p0.x, p1.y));
            builder.close();
            if let Ok(path) = builder.build() {
                let mut gray = rgb(0x808080);
                gray.a = 0.85;
                window.paint_path(path, gray);
            }
        }
        // 文字走绝对定位 div，不在画布绘制。
        Shape::Text { .. } => {}
    }
}

/// 折线描边绘制（点列经坐标换算后连线）。
fn stroke_points(
    window: &mut Window,
    pts: &[(f32, f32)],
    to_paint: &dyn Fn(f32, f32) -> Point<Pixels>,
    width: Pixels,
    color: impl Into<rgpui::Background>,
) {
    if pts.len() < 2 {
        return;
    }
    let mut builder = rgpui::PathBuilder::stroke(width);
    for (i, (x, y)) in pts.iter().enumerate() {
        let p = to_paint(*x, *y);
        if i == 0 {
            builder.move_to(p);
        } else {
            builder.line_to(p);
        }
    }
    // 闭合椭圆等多边形需要首尾相接。
    if pts.len() > 2 {
        let (x, y) = pts[0];
        let first = to_paint(x, y);
        let last = to_paint(pts[pts.len() - 1].0, pts[pts.len() - 1].1);
        if (first.x.as_f32() - last.x.as_f32()).abs() < 0.5
            && (first.y.as_f32() - last.y.as_f32()).abs() < 0.5
        {
            builder.close();
        }
    }
    if let Ok(path) = builder.build() {
        window.paint_path(path, color);
    }
}

impl OverlayView {
    /// 工具条（两排）：第一排工具 + 撤销重做 + 取消保存复制，第二排颜色 + 线宽 + 文字框。
    ///
    /// 位置由调用方算好（吸附选区右下），此处只负责内容与点击行为。
    fn toolbar(&self, view: Entity<Self>, left: f32, top: f32) -> impl IntoElement {
        let active_tool = self.tool;
        let active_color = self.color;
        let active_width = self.width;
        // 工具按钮小工厂：高亮当前工具，点击切换。
        let tool_btn = |label: &str, tool: Tool| {
            let v = view.clone();
            let active = active_tool == tool;
            div()
                .id(format!("rupix-tool-{label}"))
                .px_2()
                .py_1()
                .rounded_md()
                .text_xs()
                .cursor_pointer()
                .bg(if active { rgb(0x1E90FF) } else { rgb(0xF0F0F0) })
                .text_color(if active { rgb(0xFFFFFF) } else { rgb(0x404040) })
                .child(label.to_string())
                .on_click(move |_, _, cx| {
                    v.update(cx, |this, cx| {
                        this.tool = tool;
                        this.status = SharedString::from(match tool {
                            Tool::Select => "选择模式：拖拽新建 / 移动 / 调整选区",
                            Tool::Text => "文字模式：在图上单击放置文字",
                            Tool::Eraser => "橡皮擦：单击删除命中的标注",
                            _ => "标注模式：在选区内拖拽绘制",
                        });
                        cx.notify();
                    });
                })
        };
        // 颜色小工厂。
        let color_btn = |label: &str, color: AnnoColor| {
            let v = view.clone();
            let active =
                active_color.0 == color.0 && active_color.1 == color.1 && active_color.2 == color.2;
            div()
                .id(format!("rupix-color-{label}"))
                .w(px(20.))
                .h(px(20.))
                .rounded_md()
                .cursor_pointer()
                .bg(color.to_rgpui())
                .border_1()
                .border_color(if active { rgb(0x1E90FF) } else { rgb(0xCCCCCC) })
                .on_click(move |_, _, cx| {
                    v.update(cx, |this, cx| {
                        this.color = color;
                        cx.notify();
                    });
                })
        };
        // 线宽小工厂。
        let width_btn = |label: &str, width: f32| {
            let v = view.clone();
            let active = (active_width - width).abs() < 0.1;
            div()
                .id(format!("rupix-width-{label}"))
                .px_2()
                .py_1()
                .rounded_md()
                .text_xs()
                .cursor_pointer()
                .bg(if active { rgb(0x1E90FF) } else { rgb(0xF0F0F0) })
                .text_color(if active { rgb(0xFFFFFF) } else { rgb(0x404040) })
                .child(label.to_string())
                .on_click(move |_, _, cx| {
                    v.update(cx, |this, cx| {
                        this.width = width;
                        cx.notify();
                    });
                })
        };

        // 动作按钮（撤销 / 重做 / 取消 / 保存 / 复制）各自绑定视图方法。
        let action_btn =
            |id: &str,
             label: &str,
             action: fn(&mut OverlayView, &mut Window, &mut Context<OverlayView>)| {
                let v = view.clone();
                div()
                    .id(format!("rupix-action-{id}"))
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .text_xs()
                    .cursor_pointer()
                    .bg(rgb(0xF0F0F0))
                    .text_color(rgb(0x404040))
                    .child(label.to_string())
                    .on_click(move |_, window, cx| {
                        v.update(cx, |this, cx| action(this, window, cx));
                    })
            };

        div()
            .absolute()
            .left(px(left))
            .top(px(top))
            .flex()
            .flex_col()
            .gap_1()
            .p_2()
            .rounded_md()
            .bg(rgb(0xFFFFFF))
            .border_1()
            .border_color(rgb(0x1E90FF))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(tool_btn("选择", Tool::Select))
                    .child(tool_btn("矩形", Tool::Rect))
                    .child(tool_btn("椭圆", Tool::Ellipse))
                    .child(tool_btn("箭头", Tool::Arrow))
                    .child(tool_btn("画笔", Tool::Pen))
                    .child(tool_btn("荧光", Tool::Highlighter))
                    .child(tool_btn("马赛克", Tool::Mosaic))
                    .child(tool_btn("文字", Tool::Text))
                    .child(tool_btn("橡皮", Tool::Eraser))
                    .child(action_btn("undo", "撤销", OverlayView::undo_with_window))
                    .child(action_btn("redo", "重做", OverlayView::redo_with_window))
                    .child(action_btn("cancel", "取消", OverlayView::cancel))
                    .child(action_btn("save", "保存", OverlayView::save))
                    .child(action_btn("copy", "复制", OverlayView::copy_and_close)),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(color_btn("红", AnnoColor::RED))
                    .child(color_btn("黄", AnnoColor::YELLOW))
                    .child(color_btn("绿", AnnoColor::GREEN))
                    .child(color_btn("蓝", AnnoColor::BLUE))
                    .child(color_btn("黑", AnnoColor::BLACK))
                    .child(color_btn("白", AnnoColor::WHITE))
                    .child(width_btn("细", 2.0))
                    .child(width_btn("中", 4.0))
                    .child(width_btn("粗", 8.0))
                    .child(Input::new(&self.text_input)),
            )
    }

    /// 撤销（工具条回调适配：忽略窗口参数）。
    fn undo_with_window(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.undo(cx);
    }

    /// 重做（工具条回调适配：忽略窗口参数）。
    fn redo_with_window(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.redo(cx);
    }

    /// 取消：回主视图状态并关闭浮层。
    fn cancel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.main.update(cx, |state, cx| {
            state.set_status("已取消截图", cx);
        });
        window.remove_window();
    }
}

/// 工具条位置（容器相对坐标）：吸附选区右下，放不下则挪到选区上方。
fn toolbar_position(sel: Option<PixelRect>, scale: f32, cont_w: f32, cont_h: f32) -> (f32, f32) {
    // 工具条估算尺寸（两排按钮 + 内边距），命中过滤共用该估计。
    const TB_W: f32 = 700.0;
    const TB_H: f32 = 96.0;
    match sel {
        Some(sel) => {
            // 右对齐选区右 edge，超出容器则左移钳制。
            let left =
                (sel.x * scale + sel.w * scale - TB_W).clamp(8.0, (cont_w - TB_W - 8.0).max(8.0));
            // 默认放选区下方，底部空间不足则翻到上方。
            let below = sel.y * scale + sel.h * scale + 8.0;
            let top = if below + TB_H > cont_h {
                (sel.y * scale - TB_H - 8.0).max(8.0)
            } else {
                below
            };
            (left, top)
        }
        // 无选区时底部居中悬浮，引导用户先框选。
        None => (
            ((cont_w - TB_W) / 2.0).max(8.0),
            (cont_h - TB_H - 16.0).max(8.0),
        ),
    }
}

/// 选区八手柄的圖片像素坐标（四角 + 四边中点）。
fn handle_points(sel: PixelRect) -> [(Handle, (f32, f32)); 8] {
    let (x0, y0) = (sel.x, sel.y);
    let (x1, y1) = (sel.x + sel.w, sel.y + sel.h);
    let (mx, my) = (sel.x + sel.w / 2.0, sel.y + sel.h / 2.0);
    [
        (Handle::NW, (x0, y0)),
        (Handle::N, (mx, y0)),
        (Handle::NE, (x1, y0)),
        (Handle::E, (x1, my)),
        (Handle::SE, (x1, y1)),
        (Handle::S, (mx, y1)),
        (Handle::SW, (x0, y1)),
        (Handle::W, (x0, my)),
    ]
}

/// 按手柄拖拽调整选区（最小 4px，全程钳制在图片内）。
fn resize_rect(sel: PixelRect, handle: Handle, p: (f32, f32), max_w: f32, max_h: f32) -> PixelRect {
    let (mut x0, mut y0) = (sel.x, sel.y);
    let (mut x1, mut y1) = (sel.x + sel.w, sel.y + sel.h);
    // 各手柄只动对应的边：角手柄动两条边，边手柄动一条边。
    match handle {
        Handle::NW => (x0, y0) = p,
        Handle::N => y0 = p.1,
        Handle::NE => (x1, y0) = p,
        Handle::E => x1 = p.0,
        Handle::SE => (x1, y1) = p,
        Handle::S => y1 = p.1,
        Handle::SW => (x0, y1) = p,
        Handle::W => x0 = p.0,
    }
    // 钳制 + 保序（拖过对边时固定在最小尺寸）。
    const MIN: f32 = 4.0;
    x0 = x0.clamp(0.0, max_w - MIN);
    x1 = x1.clamp(MIN, max_w);
    y0 = y0.clamp(0.0, max_h - MIN);
    y1 = y1.clamp(MIN, max_h);
    if x1 - x0 < MIN {
        if matches!(handle, Handle::NW | Handle::W | Handle::SW) {
            x0 = x1 - MIN;
        } else {
            x1 = x0 + MIN;
        }
    }
    if y1 - y0 < MIN {
        if matches!(handle, Handle::NW | Handle::N | Handle::NE) {
            y0 = y1 - MIN;
        } else {
            y1 = y0 + MIN;
        }
    }
    PixelRect::new(x0, y0, x1 - x0, y1 - y0)
}

/// 椭圆参数方程取点（首尾相接，供描边闭合判断）。
fn ellipse_points(rect: PixelRect, n: usize) -> Vec<(f32, f32)> {
    let (rx, ry) = (rect.w / 2.0, rect.h / 2.0);
    let (cx, cy) = (rect.x + rx, rect.y + ry);
    (0..=n)
        .map(|i| {
            let theta = i as f32 / n as f32 * std::f32::consts::TAU;
            (cx + rx * theta.cos(), cy + ry * theta.sin())
        })
        .collect()
}

/// 绘制中图形是否实质为空（点一下没拖出长度则丢弃）。
fn shape_is_empty(shape: &Shape) -> bool {
    match shape {
        Shape::Pen { pts, .. } | Shape::Highlighter { pts, .. } => {
            // 全部点重合视为空（单击无拖拽）。
            pts.windows(2).all(|pair| {
                (pair[0].0 - pair[1].0).abs() < 0.5 && (pair[0].1 - pair[1].1).abs() < 0.5
            })
        }
        Shape::Rect { rect, .. } | Shape::Ellipse { rect, .. } | Shape::Mosaic { rect } => {
            rect.w < 1.0 || rect.h < 1.0
        }
        Shape::Arrow { from, to, .. } => {
            ((to.0 - from.0).powi(2) + (to.1 - from.1).powi(2)).sqrt() < 2.0
        }
        // 文字落点即有效。
        Shape::Text { .. } => false,
    }
}
