//! 区域选择浮层：先冻结一张全屏图盖住屏幕，再在上面拖拽画矩形裁剪。
//!
//! 与系统截图 / Snipaste 一致：浮层显示的是截取瞬间的静态首帧（1:1 物理像素），
//! 而不是透明活背景——视频等动态内容会被定住，框选与裁剪严格对应同一帧。
//! 窗口吸附：未拖拽时悬停窗口显示高亮，点击即截该窗口，
//! 拖拽则按自由选区裁剪（独立窗口截图入口已移除，统一走区域截图）。
//! 界面只用 `img` + 纯色 `div`（遮罩条 / 边框 / 徽标），不用 `canvas`。

use std::{path::PathBuf, sync::Arc};

use rgpui::{
    Bounds, Context, Entity, FocusHandle, KeyDownEvent, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Pixels, Render, RenderImage, Window, WindowBackgroundAppearance,
    WindowBounds, WindowKind, WindowOptions, div, img, prelude::*, px, rgb, rgba,
};

use crate::{
    app::{PreviewImage, RupixApp, autosave_screenshot},
    capture::cap_log,
    viewer::open_viewer,
};

/// 首帧捕获完成后传给浮层的数据。
pub struct PendingCapture {
    /// 首帧落盘的临时图片路径（裁剪的像素来源）。
    pub frame_path: PathBuf,
    /// 全图宽度（图片像素）。
    pub img_w: u32,
    /// 全图高度（图片像素）。
    pub img_h: u32,
    /// 浮层窗口尺寸（取主显示器边界）。
    pub display_bounds: Bounds<Pixels>,
    /// 预解码的首帧（同步图片源用；None 时回退路径异步加载）。
    pub frame_image: Option<Arc<RenderImage>>,
}

/// 打开区域选择浮层。
///
/// # 参数
/// - `main`: 主视图实体（裁剪完成后回写预览图 / 取消时恢复状态）。
/// - `cx`: 调用方上下文（可解引用为 `App`）。
/// - `pending`: 首帧图片与窗口尺寸。
pub fn open_overlay(main: Entity<RupixApp>, cx: &mut Context<RupixApp>, pending: PendingCapture) {
    // 图片尺寸兜底：捕获时没读到就现场解码一次。
    let (img_w, img_h) = if pending.img_w > 0 && pending.img_h > 0 {
        (pending.img_w, pending.img_h)
    } else {
        image::image_dimensions(&pending.frame_path).unwrap_or((800, 600))
    };
    let PendingCapture {
        frame_path,
        display_bounds,
        frame_image,
        ..
    } = pending;
    cap_log(&format!(
        "浮层打开：img={img_w}x{img_h} bounds={display_bounds:?}"
    ));
    let opened = cx.open_window(
        WindowOptions {
            // 无边框置顶透明浮层，盖住全屏做选区交互。
            // 注意：必须用 Fullscreen 而不是 Windowed——Windowed + Overlay
            // 在 Windows 下 placement 不生效，窗口保持 0x0 不可见。
            titlebar: None,
            window_bounds: Some(WindowBounds::Fullscreen(display_bounds)),
            window_background: WindowBackgroundAppearance::Transparent,
            kind: WindowKind::Overlay,
            focus: true,
            show: true,
            ..Default::default()
        },
        move |window, cx| {
            let view = cx.new(|cx| {
                SelectView::new(
                    window,
                    cx,
                    main.clone(),
                    frame_path.clone(),
                    img_w,
                    img_h,
                    display_bounds,
                    frame_image.clone(),
                )
            });
            // 浮层打开即聚焦根视图，使 Esc 立即可用。
            let handle = view.read(cx).focus_handle.clone();
            window.focus(&handle, cx);
            view
        },
    );
    match opened {
        Ok(_) => cap_log("浮层 open_window 成功"),
        Err(error) => cap_log(&format!("浮层 open_window 失败：{error:?}")),
    }
}

/// 区域选择浮层视图：薄遮罩 + 拖拽矩形，抬手裁剪；未拖拽时悬停窗口吸附高亮，点击截取。
struct SelectView {
    /// 主视图实体（回写预览 / 状态）。
    main: Entity<RupixApp>,
    /// 首帧图片路径（裁剪来源）。
    frame_path: PathBuf,
    /// 预解码的首帧（浮层背景同步显示用）。
    frame_image: Option<Arc<RenderImage>>,
    /// 全图宽（图片像素）。
    img_w: u32,
    /// 全图高（图片像素）。
    img_h: u32,
    /// 浮层逻辑宽（显示器逻辑像素，鼠标坐标空间）。
    view_w: f32,
    /// 浮层逻辑高。
    view_h: f32,
    /// 拖拽起点（视图逻辑像素），None 表示未按下。
    anchor: Option<(f32, f32)>,
    /// 拖拽当前点（视图逻辑像素）。
    current: Option<(f32, f32)>,
    /// 根视图焦点（Esc 取消用）。
    focus_handle: FocusHandle,
    /// 当前吸附高亮的窗口（视图逻辑像素矩形 + 标题），未拖拽时悬停吸附，点击即截该窗口。
    hover: Option<((f32, f32, f32, f32), String)>,
}

impl SelectView {
    /// 创建选择视图。
    #[allow(clippy::too_many_arguments)]
    fn new(
        _window: &mut Window,
        cx: &mut Context<Self>,
        main: Entity<RupixApp>,
        frame_path: PathBuf,
        img_w: u32,
        img_h: u32,
        display_bounds: Bounds<Pixels>,
        frame_image: Option<Arc<RenderImage>>,
    ) -> Self {
        Self {
            main,
            frame_path,
            frame_image,
            img_w,
            img_h,
            view_w: display_bounds.size.width.as_f32(),
            view_h: display_bounds.size.height.as_f32(),
            anchor: None,
            current: None,
            focus_handle: cx.focus_handle(),
            hover: None,
        }
    }

    /// 当前选区（视图逻辑像素，起点终点归一化），未拖拽时为 None。
    fn sel_rect(&self) -> Option<(f32, f32, f32, f32)> {
        norm_rect(self.anchor?, self.current?, self.view_w, self.view_h)
    }

    /// 视图逻辑像素 → 图片物理像素的缩放比。
    fn scale(&self) -> (f32, f32) {
        (
            self.img_w as f32 / self.view_w.max(1.0),
            self.img_h as f32 / self.view_h.max(1.0),
        )
    }

    /// 左键按下：开始新的拖拽选区。
    fn on_left_down(&mut self, ev: &MouseDownEvent, cx: &mut Context<Self>) {
        // 鼠标坐标即窗口坐标，根视图铺满窗口，直接可用。
        let p = (ev.position.x.as_f32(), ev.position.y.as_f32());
        self.anchor = Some(p);
        self.current = Some(p);
        cx.notify();
    }

    /// 鼠标移动：未拖拽时吸附窗口更新高亮，拖拽中更新选区当前点。
    fn on_mouse_move(&mut self, ev: &MouseMoveEvent, cx: &mut Context<Self>) {
        let p = (ev.position.x.as_f32(), ev.position.y.as_f32());
        // 未按下时悬停吸附窗口（点击即截该窗口），拖拽中只更新选区。
        if self.anchor.is_none() {
            self.update_hover(p, cx);
            return;
        }
        self.current = Some(p);
        cx.notify();
    }

    /// 按视图坐标更新窗口吸附高亮（未拖拽时悬停调用）。
    fn update_hover(&mut self, p: (f32, f32), cx: &mut Context<Self>) {
        // 视图坐标换算到物理像素做命中，高亮矩形换回视图坐标展示。
        let (sx, sy) = self.scale();
        let hit = crate::window::hit_test((p.0 * sx) as i32, (p.1 * sy) as i32).map(|w| {
            let (l, t, _, _) = w.rect;
            (
                (
                    l as f32 / sx,
                    t as f32 / sy,
                    w.width() as f32 / sx,
                    w.height() as f32 / sy,
                ),
                w.title,
            )
        });
        // 高亮不变时不刷新，避免每帧重排。
        if hit.as_ref().map(|(r, _)| *r) != self.hover.as_ref().map(|(r, _)| *r) {
            self.hover = hit;
            cx.notify();
        }
    }

    /// 左键抬起：拖拽出选区则裁剪，点击窗口则截该窗口。
    fn on_left_up(&mut self, ev: &MouseUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        // anchor 取走后置空：拖出窗口松开会再来一次 mouse_up_out，
        // 不清空会导致 finish 重复执行（双裁剪、双预览窗口）。
        let Some(anchor) = self.anchor.take() else {
            return;
        };
        let cur = (ev.position.x.as_f32(), ev.position.y.as_f32());
        self.current = Some(cur);
        // 用取出的起点直接算矩形（anchor 已清空防重入，不走 sel_rect）。
        let Some((x, y, w, h)) = norm_rect(anchor, cur, self.view_w, self.view_h) else {
            // 纯点击无位移：悬停在窗口上则截该窗口，否则不关闭允许重新拖拽。
            if let Some(((hx, hy, hw, hh), _)) = self.hover.clone() {
                self.finish(window, cx, (hx, hy, hw, hh));
            } else {
                self.current = None;
                cx.notify();
            }
            return;
        };
        // 极小拖动视为点击：悬停在窗口上则直接截该窗口（区域模式吸附窗口）；
        // 空白处点按仍视为误触，不关闭，允许重新拖拽。
        if w < 5.0 || h < 5.0 {
            if let Some(((hx, hy, hw, hh), _)) = self.hover.clone() {
                self.finish(window, cx, (hx, hy, hw, hh));
            } else {
                self.current = None;
                cx.notify();
            }
            return;
        }
        self.finish(window, cx, (x, y, w, h));
    }

    /// 完成选择：裁剪首帧选区，回写主窗口预览后关闭浮层。
    fn finish(&mut self, window: &mut Window, cx: &mut Context<Self>, rect: (f32, f32, f32, f32)) {
        let (x, y, w, h) = rect;
        // 换算到图片物理像素并钳制在图内。
        let (sx, sy) = self.scale();
        let cx0 = (x * sx).round().clamp(0.0, self.img_w as f32 - 1.0) as u32;
        let cy0 = (y * sy).round().clamp(0.0, self.img_h as f32 - 1.0) as u32;
        let cw = ((w * sx).round() as u32).clamp(1, self.img_w - cx0);
        let ch = ((h * sy).round() as u32).clamp(1, self.img_h - cy0);
        let frame_path = self.frame_path.clone();
        let main = self.main.clone();
        match crop_frame(&frame_path, cx0, cy0, cw, ch) {
            Ok(preview) => {
                // 截图先自动存一份原图到默认保存目录，viewer 里的保存会另存新文件。
                let (preview, auto_note) = match autosave_screenshot(&preview.path) {
                    Some(saved) => (
                        PreviewImage {
                            path: saved,
                            w: preview.w,
                            h: preview.h,
                        },
                        "原图已自动保存",
                    ),
                    None => (preview, "自动保存失败，仅预览"),
                };
                let msg = format!("已截取 {cw} × {ch}（{auto_note}），见预览窗口");
                main.update(cx, |state, cx| {
                    state.capturing = false;
                    state.set_status(&msg, cx);
                });
                // 预览窗口独立展示，支持缩放（后续标注也加在这里）。
                open_viewer(main, cx, preview);
            }
            Err(error) => {
                main.update(cx, |state, cx| {
                    state.capturing = false;
                    state.set_status(&format!("裁剪失败：{error}"), cx);
                });
            }
        }
        window.remove_window();
        // 强制刷新所有窗口：跨窗口 update 的重画请求可能随浮层关闭丢失，
        // 主窗口按钮颜色 / 状态栏靠这次刷新落定（变灰 bug 即源于此）。
        cx.refresh_windows();
    }

    /// 取消本次截图：复位状态机并关闭浮层。
    fn cancel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let main = self.main.clone();
        main.update(cx, |state, cx| {
            state.capturing = false;
            state.set_status("已取消", cx);
        });
        window.remove_window();
        // 同 finish：保证主窗口重画，不残留“截取中”灰按钮。
        cx.refresh_windows();
    }
}

/// 两点归一化为视图内矩形（起点终点任意顺序），退化时返回 None。
fn norm_rect(a: (f32, f32), b: (f32, f32), vw: f32, vh: f32) -> Option<(f32, f32, f32, f32)> {
    let x = a.0.min(b.0).clamp(0.0, vw);
    let y = a.1.min(b.1).clamp(0.0, vh);
    let w = (a.0 - b.0).abs().min(vw - x);
    let h = (a.1 - b.1).abs().min(vh - y);
    if w < 1.0 || h < 1.0 {
        return None;
    }
    Some((x, y, w, h))
}

/// 从首帧图中裁剪矩形区域，存为新的临时文件。
///
/// # 参数
/// - `src`: 首帧图片路径。
/// - `x/y/w/h`: 图片物理像素矩形（调用方已钳制）。
///
/// # 返回
/// 裁剪图的预览信息（路径 + 实际尺寸）。
fn crop_frame(
    src: &std::path::Path,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
) -> Result<PreviewImage, String> {
    // image 0.25 的 crop_imm 做纯内存裁剪，不改原图。
    let img = image::open(src).map_err(|e| format!("读取首帧失败：{e}"))?;
    let cropped = img.crop_imm(x, y, w, h);
    let dst = std::env::temp_dir().join(format!(
        "rupix-crop-{}.png",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0)
    ));
    cropped
        .save(&dst)
        .map_err(|e| format!("保存裁剪图失败：{e}"))?;
    Ok(PreviewImage {
        path: dst,
        w: cropped.width(),
        h: cropped.height(),
    })
}

impl Render for SelectView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // 薄遮罩：透出真实屏幕，用户能看清又能感知是截图态。
        let mask = rgba(0x00000059);
        let white = rgb(0xFFFFFF);
        let sel = self.sel_rect();
        // 徽标显示图片物理像素尺寸（与裁剪结果一致）。
        let (sx, sy) = self.scale();
        // 拖拽中显示选区；未拖拽且悬停窗口时显示吸附高亮（点击即截该窗口）。
        let (show, badge) = match (sel, &self.hover) {
            (Some((x, y, w, h)), _) => {
                let badge = format!("{} × {}", (w * sx).round() as u32, (h * sy).round() as u32);
                (Some((x, y, w, h)), Some(badge))
            }
            (None, Some(((x, y, w, h), title))) => {
                let text = format!(
                    "{title}  {} × {} · 点击截取",
                    (w * sx).round() as u32,
                    (h * sy).round() as u32
                );
                (Some((*x, *y, *w, *h)), Some(text))
            }
            (None, None) => (None, None),
        };

        let mut root = div()
            .size_full()
            .relative()
            // 底层：冻结的首帧全屏图（逻辑尺寸铺满窗口即 1:1 物理像素，
            // 鼠标坐标 × scale 即图片像素，无近似）。
            // 预解码同步源首帧立现；解码失败才回退路径异步加载（会闪一下）。
            .child(
                div()
                    .absolute()
                    .size_full()
                    .child(match self.frame_image.clone() {
                        Some(pre) => img(pre).w(px(self.view_w)).h(px(self.view_h)),
                        None => img(self.frame_path.clone())
                            .w(px(self.view_w))
                            .h(px(self.view_h)),
                    }),
            )
            // 根视图可聚焦，使 Esc 在浮层打开后立即可用。
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                if ev.keystroke.key.as_str() == "escape" {
                    this.cancel(window, cx);
                }
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseDownEvent, _window, cx| {
                    this.on_left_down(ev, cx);
                }),
            )
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _window, cx| {
                this.on_mouse_move(ev, cx);
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseUpEvent, window, cx| {
                    this.on_left_up(ev, window, cx);
                }),
            )
            // 拖出窗口松开时 `on_mouse_up` 可能收不到，兜底提交。
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseUpEvent, window, cx| {
                    this.on_left_up(ev, window, cx);
                }),
            )
            // 右键任意处取消。
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _ev: &MouseDownEvent, window, cx| {
                    this.cancel(window, cx);
                }),
            )
            // 顶部操作提示。
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
                    .child("拖拽选区 / 点击窗口截取；右键 / Esc 取消"),
            );

        match show {
            None => {
                // 尚未框选：整屏薄遮罩。
                root = root.child(div().absolute().size_full().bg(mask));
            }
            Some((x, y, w, h)) => {
                // 选区外四条遮罩，选区内透出真实屏幕。
                root = root
                    .child(
                        div()
                            .absolute()
                            .left(px(0.))
                            .top(px(0.))
                            .w(px(self.view_w))
                            .h(px(y))
                            .bg(mask),
                    )
                    .child(
                        div()
                            .absolute()
                            .left(px(0.))
                            .top(px(y + h))
                            .w(px(self.view_w))
                            .h(px(self.view_h - y - h))
                            .bg(mask),
                    )
                    .child(
                        div()
                            .absolute()
                            .left(px(0.))
                            .top(px(y))
                            .w(px(x))
                            .h(px(h))
                            .bg(mask),
                    )
                    .child(
                        div()
                            .absolute()
                            .left(px(x + w))
                            .top(px(y))
                            .w(px(self.view_w - x - w))
                            .h(px(h))
                            .bg(mask),
                    )
                    // 选区白色描边（2px，四条）。
                    .child(
                        div()
                            .absolute()
                            .left(px(x))
                            .top(px(y))
                            .w(px(w))
                            .h(px(2.))
                            .bg(white),
                    )
                    .child(
                        div()
                            .absolute()
                            .left(px(x))
                            .top(px(y + h - 2.))
                            .w(px(w))
                            .h(px(2.))
                            .bg(white),
                    )
                    .child(
                        div()
                            .absolute()
                            .left(px(x))
                            .top(px(y))
                            .w(px(2.))
                            .h(px(h))
                            .bg(white),
                    )
                    .child(
                        div()
                            .absolute()
                            .left(px(x + w - 2.))
                            .top(px(y))
                            .w(px(2.))
                            .h(px(h))
                            .bg(white),
                    );
                // 尺寸徽标：贴顶时翻到选区内部，避免被屏幕边缘裁掉。
                if let Some(text) = badge {
                    let badge_top = if y < 30.0 { y + 4.0 } else { y - 26.0 };
                    root = root.child(
                        div()
                            .absolute()
                            .left(px(x))
                            .top(px(badge_top))
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .bg(rgb(0x000000))
                            .text_color(rgb(0xFFFFFF))
                            .text_xs()
                            .child(text),
                    );
                }
            }
        }
        root
    }
}
