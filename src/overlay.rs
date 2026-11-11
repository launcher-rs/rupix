//! 区域选择浮层：先冻结一张全屏图盖住屏幕，再在上面拖拽画矩形裁剪。
//!
//! 与系统截图 / Snipaste 一致：浮层显示的是截取瞬间的静态首帧（1:1 物理像素），
//! 而不是透明活背景——视频等动态内容会被定住，框选与裁剪严格对应同一帧。
//! 窗口吸附：未拖拽时悬停窗口显示高亮，点击即截该窗口，
//! 拖拽则按自由选区裁剪（独立窗口截图入口已移除，统一走区域截图）。
//! 界面只用 `img` + 纯色 `div`（遮罩条 / 边框 / 徽标），不用 `canvas`。

use std::{
    cell::{Cell, RefCell},
    path::PathBuf,
    rc::Rc,
    sync::Arc,
};

use rgpui::{
    AnyWindowHandle, AppContext, Bounds, Context, Entity, FocusHandle, KeyDownEvent, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Render, RenderImage, Window,
    WindowBackgroundAppearance, WindowBounds, WindowKind, WindowOptions, div, img, prelude::*, px,
    rgb, rgba,
};

use crate::{
    app::{PreviewImage, RupixApp, finish_capture},
    capture::{CapturedFrame, cap_log},
    i18n::{tr, tr_fmt},
};

/// 一轮多屏浮层的共享会话：任一处完成 / 取消 / 被系统关闭时，统一关掉同轮其余浮层。
pub struct OverlaySession {
    /// 每块屏的浮层窗口句柄（按打开顺序，未打开的位置为 None）。
    handles: RefCell<Vec<Option<AnyWindowHandle>>>,
    /// 本轮是否已结束（结束后其余浮层只关窗、不再重复收尾）。
    finished: Cell<bool>,
}

impl OverlaySession {
    /// 创建一轮浮层会话。
    pub fn new() -> Rc<Self> {
        Rc::new(Self {
            handles: RefCell::new(Vec::new()),
            finished: Cell::new(false),
        })
    }

    /// 登记第 `index` 块屏的浮层窗口句柄（句柄在开窗返回后才有）。
    pub(crate) fn register(&self, index: usize, handle: AnyWindowHandle) {
        let mut handles = self.handles.borrow_mut();
        if handles.len() <= index {
            handles.resize(index + 1, None);
        }
        handles[index] = Some(handle);
    }

    /// 本轮是否已结束。
    pub fn is_finished(&self) -> bool {
        self.finished.get()
    }

    /// 结束整轮并关闭除 `keep` 以外的浮层；返回是否由本次调用触发结束。
    pub(crate) fn finish<C: AppContext>(&self, keep: Option<usize>, cx: &mut C) -> bool {
        if self.finished.replace(true) {
            return false;
        }
        let handles = self.handles.borrow().clone();
        for (index, handle) in handles.into_iter().enumerate() {
            if Some(index) == keep {
                continue;
            }
            if let Some(handle) = handle {
                // 其余浮层直接移除；句柄失效（已被系统关掉）时忽略错误。
                let _ = handle.update(cx, |_, window, _| window.remove_window());
            }
        }
        true
    }
}

/// 在每块屏上各打开一层区域选择浮层（多显示器可分别在任意屏幕框选）。
///
/// # 参数
/// - `main`: 主视图实体（裁剪完成后回写预览图 / 取消时恢复状态）。
/// - `cx`: 调用方上下文（可解引用为 `App`）。
/// - `frames`: 每块屏的首帧图片与显示器边界。
///
/// # 返回
/// 至少成功打开一层返回 `true`（之后由浮层的完成 / 取消复位状态）；
/// 全部建窗失败返回 `false`，调用方需自行把「截取中」状态复位。
pub fn open_overlays(
    main: Entity<RupixApp>,
    cx: &mut Context<RupixApp>,
    frames: Vec<CapturedFrame>,
) -> bool {
    if frames.is_empty() {
        return false;
    }
    let session = OverlaySession::new();
    let mut opened = 0;
    for (index, frame) in frames.into_iter().enumerate() {
        // 极少数情况下前一块屏的浮层已结束整轮，不再补开。
        if session.is_finished() {
            break;
        }
        if open_overlay(main.clone(), cx, frame, index, session.clone()) {
            opened += 1;
        }
    }
    opened > 0
}

/// 打开单块屏的区域选择浮层。
fn open_overlay(
    main: Entity<RupixApp>,
    cx: &mut Context<RupixApp>,
    frame: CapturedFrame,
    index: usize,
    session: Rc<OverlaySession>,
) -> bool {
    // 图片尺寸兜底：捕获时没读到就现场解码一次。
    let (img_w, img_h) = if frame.img_w > 0 && frame.img_h > 0 {
        (frame.img_w, frame.img_h)
    } else {
        image::image_dimensions(&frame.frame_path).unwrap_or((800, 600))
    };
    let CapturedFrame {
        frame_path,
        display_bounds,
        display_id,
        frame_image,
        ..
    } = frame;
    cap_log(&format!(
        "浮层打开：屏 {index} img={img_w}x{img_h} id={display_id:?} bounds={display_bounds:?}"
    ));
    let session_for_build = session.clone();
    let opened = cx.open_window(
        WindowOptions {
            // 无边框置顶浮层，盖住全屏做选区交互。
            // 背景必须是 Opaque：浮层本就用冻结截图铺满整屏，遮罩、选框、徽标都是在这张
            // 图上做的层内混合，窗口级逐像素 alpha 没有意义；而 X11 下一旦要求 Transparent
            // 就会取 32 位 ARGB visual，在 alpha 合成路径不可用的环境（xrdp / 软件 GL 等）
            // 里写进去的 alpha 恒为 0，整扇浮层被判成全透明——表现为没有遮罩也没有选框。
            // 注意：必须用 Fullscreen 而不是 Windowed——Windowed + Overlay
            // 在 Windows 下 placement 不生效，窗口保持 0x0 不可见。
            titlebar: None,
            window_bounds: Some(WindowBounds::Fullscreen(display_bounds)),
            // 指定目标显示器：Windows 的 Fullscreen 会铺到该显示器的物理边界，
            // 不指定时永远落在主屏，副屏就没有浮层可选。
            display_id,
            window_background: WindowBackgroundAppearance::Opaque,
            kind: WindowKind::Overlay,
            focus: true,
            show: true,
            ..Default::default()
        },
        move |window, cx| {
            // 浮层被系统直接关掉（Alt+F4 等）时也要结束整轮并复位状态机，
            // 否则 capturing 永远为 true，之后所有截图入口都会被挡住。
            let main_for_close = main.clone();
            let session_for_close = session_for_build.clone();
            window.on_window_should_close(cx, move |_, cx| {
                if session_for_close.finish(Some(index), cx) {
                    main_for_close.update(cx, |state, cx| {
                        state.capturing = false;
                        state.set_status(&tr("cap.cancelled"), cx);
                    });
                }
                true
            });
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
                    session_for_build.clone(),
                    index,
                )
            });
            // 浮层打开即聚焦根视图并激活窗口，使 Esc 立即可用。
            let handle = view.read(cx).focus_handle.clone();
            window.focus(&handle, cx);
            window.activate_window();
            view
        },
    );
    match opened {
        Ok(handle) => {
            session.register(index, *handle);
            cap_log("浮层 open_window 成功");
            true
        }
        Err(error) => {
            cap_log(&format!("浮层 open_window 失败：{error:?}"));
            false
        }
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
    /// 本屏在桌面物理坐标系中的原点（像素）。
    /// 窗口枚举返回的是桌面全局物理坐标，副屏命中/高亮必须加上这个偏移，
    /// 否则会被当成主屏同位置的窗口（多屏偏移 bug）。
    origin_px: (f32, f32),
    /// 拖拽起点（视图逻辑像素），None 表示未按下。
    anchor: Option<(f32, f32)>,
    /// 拖拽当前点（视图逻辑像素）。
    current: Option<(f32, f32)>,
    /// 根视图焦点（Esc 取消用）。
    focus_handle: FocusHandle,
    /// 当前吸附高亮的窗口（视图逻辑像素矩形 + 标题），未拖拽时悬停吸附，点击即截该窗口。
    hover: Option<((f32, f32, f32, f32), String)>,
    /// 本轮多屏浮层的共享会话（完成 / 取消时统一关闭其余屏的浮层）。
    session: Rc<OverlaySession>,
    /// 本浮层对应的屏序号（会话内唯一）。
    index: usize,
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
        session: Rc<OverlaySession>,
        index: usize,
    ) -> Self {
        let view_w = display_bounds.size.width.as_f32();
        let view_h = display_bounds.size.height.as_f32();
        // 图片物理像素 / 视图逻辑像素 = 该屏缩放比；逻辑原点乘缩放比即物理原点。
        let sx = img_w as f32 / view_w.max(1.0);
        let sy = img_h as f32 / view_h.max(1.0);
        Self {
            main,
            frame_path,
            frame_image,
            img_w,
            img_h,
            view_w,
            view_h,
            origin_px: (
                display_bounds.origin.x.as_f32() * sx,
                display_bounds.origin.y.as_f32() * sy,
            ),
            anchor: None,
            current: None,
            focus_handle: cx.focus_handle(),
            hover: None,
            session,
            index,
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
        // 视图坐标换算到桌面全局物理像素做命中：先乘缩放比，再加本屏物理原点。
        let (sx, sy) = self.scale();
        let gx = (self.origin_px.0 + p.0 * sx).round() as i32;
        let gy = (self.origin_px.1 + p.1 * sy).round() as i32;
        let hit = crate::window::hit_test(gx, gy).and_then(|w| {
            view_rect_from_physical(w.rect, self.origin_px, (sx, sy), (self.view_w, self.view_h))
                .map(|rect| (rect, w.title))
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

    /// 完成选择：裁剪首帧选区，交统一收尾（保存 / 复制 / 编辑窗 / 提示）后关闭本轮浮层。
    fn finish(&mut self, window: &mut Window, cx: &mut Context<Self>, rect: (f32, f32, f32, f32)) {
        // 本轮已在别的屏上收尾：这里只关自己，避免重复裁剪 / 重复保存。
        if self.session.is_finished() {
            window.remove_window();
            return;
        }
        let (x, y, w, h) = rect;
        // 换算到图片物理像素并钳制在图内。
        let (sx, sy) = self.scale();
        let cx0 = (x * sx).round().clamp(0.0, self.img_w as f32 - 1.0) as u32;
        let cy0 = (y * sy).round().clamp(0.0, self.img_h as f32 - 1.0) as u32;
        let cw = ((w * sx).round() as u32).clamp(1, self.img_w - cx0);
        let ch = ((h * sy).round() as u32).clamp(1, self.img_h - cy0);
        let frame_path = self.frame_path.clone();
        let main = self.main.clone();
        // 先结束整轮，关掉其余屏的浮层；再回到主实体做统一收尾。
        self.session.finish(Some(self.index), cx);
        match crop_frame(&frame_path, cx0, cy0, cw, ch) {
            Ok(preview) => {
                // 统一收尾：自动保存 + 按设置复制 / 弹编辑窗 / 闪提示。
                // 收尾里会按设置开新窗口，放到主实体更新内做，`cx` 才是主视图上下文。
                main.update(cx, |state, cx| {
                    state.capturing = false;
                    let (_, auto_note) = finish_capture(state, cx, &preview);
                    let msg = tr_fmt(
                        "cap.cropped",
                        &[
                            ("w", &cw.to_string()),
                            ("h", &ch.to_string()),
                            ("note", &auto_note),
                        ],
                    );
                    state.set_status(&msg, cx);
                });
            }
            Err(error) => {
                let msg = tr_fmt("cap.crop_fail", &[("error", &error)]);
                main.update(cx, |state, cx| {
                    state.capturing = false;
                    state.set_status(&msg, cx);
                });
            }
        }
        window.remove_window();
        // 强制刷新所有窗口：跨窗口 update 的重画请求可能随浮层关闭丢失，
        // 主窗口按钮颜色 / 状态栏靠这次刷新落定（变灰 bug 即源于此）。
        cx.refresh_windows();
    }

    /// 取消本次截图：复位状态机并关闭本轮所有浮层。
    fn cancel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // 本轮已在别处收尾：只关自己。
        if self.session.is_finished() {
            window.remove_window();
            return;
        }
        self.session.finish(Some(self.index), cx);
        let main = self.main.clone();
        main.update(cx, |state, cx| {
            state.capturing = false;
            state.set_status(&tr("cap.cancelled"), cx);
        });
        window.remove_window();
        // 同 finish：保证主窗口重画，不残留“截取中”灰按钮。
        cx.refresh_windows();
    }
}

/// 桌面全局物理矩形 → 本屏视图坐标矩形（只保留落在本屏内的部分）。
///
/// # 参数
/// - `rect`: 窗口在桌面物理坐标系中的矩形（左、上、右、下）。
/// - `origin_px`: 本屏在桌面物理坐标系中的原点。
/// - `scale`: 视图逻辑像素 → 图片物理像素的缩放比。
/// - `view`: 本屏视图的逻辑宽高。
///
/// # 返回
/// 裁剪到本屏范围内的视图矩形；完全不在本屏时返回 None。
fn view_rect_from_physical(
    rect: (i32, i32, i32, i32),
    origin_px: (f32, f32),
    scale: (f32, f32),
    view: (f32, f32),
) -> Option<(f32, f32, f32, f32)> {
    let (l, t, r, b) = rect;
    let sx = scale.0.max(f32::EPSILON);
    let sy = scale.1.max(f32::EPSILON);
    // 减去本屏原点再除以缩放比；跨屏窗口只保留落在本屏内的部分。
    let x0 = ((l as f32 - origin_px.0) / sx).max(0.0);
    let y0 = ((t as f32 - origin_px.1) / sy).max(0.0);
    let x1 = ((r as f32 - origin_px.0) / sx).min(view.0);
    let y1 = ((b as f32 - origin_px.1) / sy).min(view.1);
    if x1 - x0 < 1.0 || y1 - y0 < 1.0 {
        return None;
    }
    Some((x0, y0, x1 - x0, y1 - y0))
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
                let text = tr_fmt(
                    "overlay.badge_click",
                    &[
                        ("title", title),
                        ("w", &((w * sx).round() as u32).to_string()),
                        ("h", &((h * sy).round() as u32).to_string()),
                    ],
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
                    .child(tr("overlay.hint")),
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

#[cfg(test)]
mod tests {
    //! 浮层坐标换算单测：副屏原点偏移与跨屏裁剪（纯函数，不依赖窗口）。

    use super::*;

    /// 副屏窗口：减去屏物理原点后换算到本屏视图坐标。
    #[test]
    fn secondary_origin_offset() {
        let rect = view_rect_from_physical(
            (3470, 66, 5065, 996),
            (3440.0, 0.0),
            (1.0, 1.0),
            (1920.0, 1080.0),
        );
        assert_eq!(rect, Some((30.0, 66.0, 1595.0, 930.0)));
    }

    /// 主屏 125% 缩放：物理矩形按缩放比换回逻辑视图坐标。
    #[test]
    fn primary_scaled_rect() {
        let rect = view_rect_from_physical(
            (151, 160, 1650, 930),
            (0.0, 0.0),
            (1.25, 1.25),
            (2752.0, 1152.0),
        );
        let (x, y, w, h) = rect.expect("主屏窗口应命中");
        assert!((x - 120.8).abs() < 0.01);
        assert!((y - 128.0).abs() < 0.01);
        assert!((w - 1199.2).abs() < 0.01);
        assert!((h - 616.0).abs() < 0.01);
    }

    /// 跨屏窗口裁剪到本屏范围；完全在屏外的窗口不产生高亮。
    #[test]
    fn cross_display_clamp() {
        let view = (1920.0, 1080.0);
        assert_eq!(
            view_rect_from_physical((3300, 100, 3800, 500), (3440.0, 0.0), (1.0, 1.0), view),
            Some((0.0, 100.0, 360.0, 400.0))
        );
        assert_eq!(
            view_rect_from_physical((100, 100, 500, 500), (3440.0, 0.0), (1.0, 1.0), view),
            None
        );
    }
}
