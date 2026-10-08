//! 屏幕取色模块：在冻结首帧上移动取色，点击复制 HEX，右键 / Esc 取消。
//!
//! 与区域截图同一帧来源：看到的是截取瞬间的静态首帧（1:1 物理像素），
//! 采样坐标严格按“视图逻辑像素 × 缩放比 = 图片物理像素”换算。

use std::sync::Arc;

use image::RgbaImage;
use rgpui::{
    Bounds, ClipboardItem, Context, Entity, FocusHandle, KeyDownEvent, MouseButton, MouseDownEvent,
    MouseMoveEvent, Pixels, Render, Rgba, Window, WindowBackgroundAppearance, WindowBounds,
    WindowKind, WindowOptions, canvas, div, fill, img, point, prelude::*, px, rgb, rgba, size,
};

use crate::{
    app::RupixApp,
    capture::{CapturedFrame, cap_log},
    i18n::{tr, tr_fmt},
};

/// 放大镜边长像素数（奇数，保证中心唯一）。
const MAG_N: i32 = 9;
/// 放大镜单个图片像素的显示边长（逻辑像素）。
const MAG_Z: f32 = 14.0;
/// 放大镜面板总边长（逻辑像素）。
const MAG_W: f32 = MAG_N as f32 * MAG_Z;

/// 打开取色浮层。
///
/// # 参数
/// - `main`: 设置窗口实体（复制成功后回写状态）。
/// - `cx`: 调用方上下文。
/// - `frame`: 首帧图片与窗口尺寸。
pub fn open_picker(main: Entity<RupixApp>, cx: &mut Context<RupixApp>, frame: CapturedFrame) {
    // 图片尺寸兜底：捕获时没读到就现场解码一次。
    let (img_w, img_h) = if frame.img_w > 0 && frame.img_h > 0 {
        (frame.img_w, frame.img_h)
    } else {
        image::image_dimensions(&frame.frame_path).unwrap_or((800, 600))
    };
    let display_bounds = frame.display_bounds;
    let frame_path = frame.frame_path;
    let frame_image = frame.frame_image;
    cap_log(&format!("取色浮层打开：img={img_w}x{img_h}"));
    let opened = cx.open_window(
        WindowOptions {
            // 无边框置顶浮层，与区域选择浮层同规格（Windows 下必须 Fullscreen）。
            // 背景同样取 Opaque：底色是铺满全屏的冻结截图，不需要窗口级 alpha，
            // 而 X11 上的 ARGB visual 在 alpha 合成不可用时会让浮层整体不可见。
            titlebar: None,
            window_bounds: Some(WindowBounds::Fullscreen(display_bounds)),
            window_background: WindowBackgroundAppearance::Opaque,
            kind: WindowKind::Overlay,
            focus: true,
            show: true,
            ..Default::default()
        },
        move |window, cx| {
            let view = cx.new(|cx| {
                PickerView::new(
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
            // 打开即聚焦根视图，使 Esc 立即可用。
            let handle = view.read(cx).focus_handle.clone();
            window.focus(&handle, cx);
            view
        },
    );
    if let Err(error) = opened {
        cap_log(&format!("取色浮层打开失败：{error:?}"));
    }
}

/// 取色浮层视图：冻结帧 + 十字线 + 放大镜，点击复制。
struct PickerView {
    /// 设置窗口实体（回写复制状态）。
    main: Entity<RupixApp>,
    /// 首帧图片路径（浮层背景显示用）。
    frame_path: std::path::PathBuf,
    /// 预解码的首帧（背景同步显示用）。
    frame_image: Option<Arc<rgpui::RenderImage>>,
    /// 采样用底图像素（None 时取色不可用，直接取消）。
    img: Option<Arc<RgbaImage>>,
    /// 全图宽（图片像素）。
    img_w: u32,
    /// 全图高（图片像素）。
    img_h: u32,
    /// 浮层逻辑宽（显示器逻辑像素，鼠标坐标空间）。
    view_w: f32,
    /// 浮层逻辑高。
    view_h: f32,
    /// 当前鼠标位置（视图逻辑像素），None 表示尚未移入。
    cursor: Option<(f32, f32)>,
    /// 根视图焦点（Esc 取消用）。
    focus_handle: FocusHandle,
}

impl PickerView {
    /// 创建取色视图。
    #[allow(clippy::too_many_arguments)]
    fn new(
        _window: &mut Window,
        cx: &mut Context<Self>,
        main: Entity<RupixApp>,
        frame_path: std::path::PathBuf,
        img_w: u32,
        img_h: u32,
        display_bounds: Bounds<Pixels>,
        frame_image: Option<Arc<rgpui::RenderImage>>,
    ) -> Self {
        // 采样直接读底图像素；读不到则取色无意义，打开后第一次移动即提示并关闭。
        let img = image::open(&frame_path)
            .ok()
            .map(|i| Arc::new(i.into_rgba8()));
        Self {
            main,
            frame_path,
            frame_image,
            img,
            img_w,
            img_h,
            view_w: display_bounds.size.width.as_f32(),
            view_h: display_bounds.size.height.as_f32(),
            cursor: None,
            focus_handle: cx.focus_handle(),
        }
    }

    /// 视图逻辑像素 → 图片物理像素的缩放比。
    fn scale(&self) -> (f32, f32) {
        (
            self.img_w as f32 / self.view_w.max(1.0),
            self.img_h as f32 / self.view_h.max(1.0),
        )
    }

    /// 当前光标处的采样颜色（越界返回 None）。
    fn sample(&self) -> Option<(u8, u8, u8)> {
        let img = self.img.as_ref()?;
        let (cx, cy) = self.cursor?;
        let (sx, sy) = self.scale();
        let x = (cx * sx).round() as i32;
        let y = (cy * sy).round() as i32;
        sample_at(img, x, y)
    }

    /// 鼠标移动：更新光标位置。
    fn on_move(&mut self, ev: &MouseMoveEvent, cx: &mut Context<Self>) {
        let p = (ev.position.x.as_f32(), ev.position.y.as_f32());
        self.cursor = Some(p);
        cx.notify();
    }

    /// 左键点击：复制当前颜色 HEX 到剪贴板并关闭。
    fn on_pick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let main = self.main.clone();
        match self.sample() {
            Some((r, g, b)) => {
                let hex = rgb_to_hex(r, g, b);
                cx.write_to_clipboard(ClipboardItem::new_string(hex.clone()));
                main.update(cx, |state, cx| {
                    let msg = tr_fmt("picker.copied", &[("hex", &hex)]);
                    state.set_status(&msg, cx);
                });
            }
            None => {
                main.update(cx, |state, cx| {
                    state.set_status(&tr("picker.fail"), cx);
                });
            }
        }
        window.remove_window();
        // 跨窗口 update 的重画请求随浮层关闭可能丢失，强制刷新落定。
        cx.refresh_windows();
    }

    /// 取消取色：复位状态机并关闭浮层。
    fn cancel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let main = self.main.clone();
        main.update(cx, |state, cx| {
            state.capturing = false;
            state.set_status(&tr("cap.cancelled"), cx);
        });
        window.remove_window();
        cx.refresh_windows();
    }
}

/// 采样图片指定物理像素（越界返回 None）。
fn sample_at(img: &RgbaImage, x: i32, y: i32) -> Option<(u8, u8, u8)> {
    if x < 0 || y < 0 || x >= img.width() as i32 || y >= img.height() as i32 {
        return None;
    }
    let p = img.get_pixel(x as u32, y as u32);
    Some((p[0], p[1], p[2]))
}

/// RGB 转 `#RRGGBB` 大写 HEX。
fn rgb_to_hex(r: u8, g: u8, b: u8) -> String {
    format!("#{r:02X}{g:02X}{b:02X}")
}

/// 放大镜某格的颜色（中心为光标像素，向外扩散，越界按边缘钳制）。
fn mag_cell(img: &RgbaImage, cx: i32, cy: i32, dx: i32, dy: i32) -> Rgba {
    let x = (cx + dx).clamp(0, img.width() as i32 - 1);
    let y = (cy + dy).clamp(0, img.height() as i32 - 1);
    let p = img.get_pixel(x as u32, y as u32);
    Rgba {
        r: p[0] as f32 / 255.0,
        g: p[1] as f32 / 255.0,
        b: p[2] as f32 / 255.0,
        a: 1.0,
    }
}

impl Render for PickerView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // 顶栏提示 + 当前颜色（无光标时显示占位）。
        let (hex, rgb_text, swatch) = match self.sample() {
            Some((r, g, b)) => (
                rgb_to_hex(r, g, b),
                format!("RGB({r}, {g}, {b})"),
                Rgba {
                    r: r as f32 / 255.0,
                    g: g as f32 / 255.0,
                    b: b as f32 / 255.0,
                    a: 1.0,
                },
            ),
            None => (
                tr("picker.move_hint"),
                tr("picker.click_hint"),
                Rgba {
                    r: 0.0,
                    g: 0.0,
                    b: 0.0,
                    a: 0.0,
                },
            ),
        };

        let mut root = div()
            .size_full()
            .relative()
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
            // 根视图可聚焦，使 Esc 立即可用。
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                if ev.keystroke.key.as_str() == "escape" {
                    this.cancel(window, cx);
                }
            }))
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _window, cx| {
                this.on_move(ev, cx);
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _ev: &MouseDownEvent, window, cx| {
                    this.on_pick(window, cx);
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _ev: &MouseDownEvent, window, cx| {
                    this.cancel(window, cx);
                }),
            )
            // 顶部信息条：色块 + HEX + RGB。
            .child(
                div()
                    .absolute()
                    .left(px(12.))
                    .top(px(12.))
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .bg(rgb(0x000000))
                    .child(div().w(px(22.)).h(px(22.)).rounded_sm().bg(swatch))
                    .child(div().text_sm().text_color(rgb(0xFFFFFF)).child(hex))
                    .child(div().text_xs().text_color(rgb(0xBBBBBB)).child(rgb_text)),
            );

        // 十字线 + 放大镜（有光标时才画）。
        if let Some((mx, my)) = self.cursor
            && let Some(img) = self.img.clone()
        {
            let (sx, sy) = self.scale();
            let cx_img = (mx * sx).round() as i32;
            let cy_img = (my * sy).round() as i32;
            // 放大镜面板贴在光标右下方，越界则翻到左上方。
            let mut px0 = mx + 24.0;
            let mut py0 = my + 24.0;
            if px0 + MAG_W + 16.0 > self.view_w {
                px0 = mx - 24.0 - MAG_W - 16.0;
            }
            if py0 + MAG_W + 16.0 > self.view_h {
                py0 = my - 24.0 - MAG_W - 16.0;
            }
            // 9×9 格颜色提前算好，画布闭包只做绘制（闭包需 'static，用 owned 数据）。
            let mut cells = Vec::with_capacity((MAG_N * MAG_N) as usize);
            let half = MAG_N / 2;
            for dy in -half..=half {
                for dx in -half..=half {
                    cells.push(mag_cell(&img, cx_img, cy_img, dx, dy));
                }
            }
            root = root
                .child(
                    div()
                        .absolute()
                        .left(px(0.))
                        .top(px(my))
                        .w(px(self.view_w))
                        .h(px(1.))
                        .bg(rgba(0xFFFFFF80)),
                )
                .child(
                    div()
                        .absolute()
                        .left(px(mx))
                        .top(px(0.))
                        .w(px(1.))
                        .h(px(self.view_h))
                        .bg(rgba(0xFFFFFF80)),
                )
                .child(
                    div()
                        .absolute()
                        .left(px(px0))
                        .top(px(py0))
                        .w(px(MAG_W + 16.0))
                        .rounded_md()
                        .bg(rgb(0x000000))
                        .p_2()
                        .child(canvas(
                            move |bounds, _, _| bounds,
                            move |_bounds, paint_bounds, window, _| {
                                let origin = paint_bounds.origin;
                                let n = MAG_N as usize;
                                for (i, color) in cells.iter().enumerate() {
                                    let gx = (i % n) as f32;
                                    let gy = (i / n) as f32;
                                    let bounds = Bounds::new(
                                        point(
                                            px(origin.x.as_f32() + gx * MAG_Z),
                                            px(origin.y.as_f32() + gy * MAG_Z),
                                        ),
                                        size(px(MAG_Z), px(MAG_Z)),
                                    );
                                    window.paint_quad(fill(bounds, *color));
                                }
                                // 中心格白色描边（2px 四条）。
                                let c0 = MAG_N as f32 / 2.0 * MAG_Z;
                                let ox = origin.x.as_f32();
                                let oy = origin.y.as_f32();
                                let white = Rgba {
                                    r: 1.0,
                                    g: 1.0,
                                    b: 1.0,
                                    a: 1.0,
                                };
                                let bars = [
                                    (c0, c0, MAG_Z, 2.0),
                                    (c0, c0 + MAG_Z - 2.0, MAG_Z, 2.0),
                                    (c0, c0, 2.0, MAG_Z),
                                    (c0 + MAG_Z - 2.0, c0, 2.0, MAG_Z),
                                ];
                                for (bx, by, bw, bh) in bars {
                                    window.paint_quad(fill(
                                        Bounds::new(
                                            point(px(ox + bx), px(oy + by)),
                                            size(px(bw), px(bh)),
                                        ),
                                        white,
                                    ));
                                }
                            },
                        ))
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(0xBBBBBB))
                                .child(format!("{cx_img}, {cy_img}")),
                        ),
                );
        }
        root
    }
}

#[cfg(test)]
mod tests {
    //! 取色模块单测：采样与 HEX 格式化（纯函数，不依赖窗口）。

    use super::*;

    /// 采样越界与 HEX 格式断言。
    #[test]
    fn sample_and_hex() {
        let mut img = RgbaImage::new(3, 2);
        img.put_pixel(1, 0, image::Rgba([255, 0, 0, 255]));
        assert_eq!(sample_at(&img, 1, 0), Some((255, 0, 0)));
        assert_eq!(sample_at(&img, 0, 0), Some((0, 0, 0)));
        assert_eq!(sample_at(&img, 3, 0), None);
        assert_eq!(sample_at(&img, -1, 0), None);
        assert_eq!(rgb_to_hex(255, 0, 0), "#FF0000");
        assert_eq!(rgb_to_hex(1, 2, 3), "#010203");
    }

    /// 放大镜越界钳制：角落采样不 panic 且返回边缘色。
    #[test]
    fn mag_cell_clamp() {
        let mut img = RgbaImage::new(2, 2);
        img.put_pixel(0, 0, image::Rgba([10, 20, 30, 255]));
        let c = mag_cell(&img, 0, 0, -4, -4);
        assert!((c.r - 10.0 / 255.0).abs() < 1e-6);
    }
}
