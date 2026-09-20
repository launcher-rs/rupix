//! 截图捕获模块：从系统屏幕源截取首帧，落地为临时图片后打开标注浮层。
//!
//! rgpui 的屏幕捕获只有持续推流接口（`stream`），没有单帧 API，
//! 因此“截取一张图”实现为：启动推流 → 取第一帧的 RGBA → 存盘 → 停止推流。

use std::{
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use rgpui::{Bounds, Context, Pixels, ScreenCaptureSource, SourceMetadata, Window};

use crate::{
    app::RupixApp,
    overlay::{PendingCapture, open_overlay},
};

/// 开始一次截屏流程（区域截图或全屏截图共用）。
///
/// # 参数
/// - `state`: 主视图可变引用（调用方已持有实体租借，直接改状态，不要再 `update`）。
/// - `window`: 发起截图的窗口，用于挂载后台任务。
/// - `cx`: 主视图上下文。
/// - `fullscreen`: 为 true 时浮层打开即选中全图，跳过手动选区。
pub fn begin_capture(
    state: &mut RupixApp,
    window: &mut Window,
    cx: &mut Context<RupixApp>,
    fullscreen: bool,
) {
    // 同一时间只允许一轮截取，避免重复推流占用系统捕获会话。
    if state.capturing {
        return;
    }
    state.capturing = true;
    state.set_status("正在截取屏幕…", cx);

    // 浮层需要全屏尺寸：优先取主显示器边界，取不到则回退到居中默认尺寸。
    let display_bounds: Bounds<Pixels> = cx
        .primary_display()
        .map(|display| display.bounds())
        .unwrap_or_else(|| {
            Bounds::centered(None, rgpui::size(rgpui::px(1280.), rgpui::px(800.)), cx)
        });

    // 捕获源枚举是异步 oneshot，先在 UI 线程拿到接收端，再挂后台任务等待。
    let sources_rx = cx.screen_capture_sources();
    cx.spawn_in(window, async move |this, cx| {
        // 等待系统返回可捕获的屏幕 / 窗口列表。
        let sources: Vec<Rc<dyn ScreenCaptureSource>> = match sources_rx.await {
            Ok(Ok(sources)) if !sources.is_empty() => sources,
            _ => {
                let _ = this.update(cx, |state, cx| {
                    state.capturing = false;
                    state.set_status("未找到可捕获的屏幕源", cx);
                });
                return;
            }
        };

        // 优先选择主显示器，其次取第一个源，保证点一次按钮必有图。
        let source = pick_main_source(&sources).unwrap_or_else(|| sources[0].clone());

        // 首帧落盘路径：系统临时目录 + 时间戳，避免多轮覆盖。
        let frame_path = std::env::temp_dir().join(format!(
            "rupix-frame-{}.png",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0)
        ));
        let frame_saved = Arc::new(AtomicBool::new(false));

        // 推流回调跑在捕获线程，只能做 Send 操作：转 RGBA 并写文件。
        let callback_path = frame_path.clone();
        let callback_flag = frame_saved.clone();
        let frame_callback: Box<dyn Fn(rgpui::ScreenCaptureFrame) + Send> =
            Box::new(move |frame| {
                // 只要第一帧，后续帧直接丢弃，节省 IO。
                if callback_flag.load(Ordering::Relaxed) {
                    return;
                }
                if let Some(rgba) = frame.to_rgba() {
                    // 宽高留给浮层做 1:1 显示计算。
                    let (width, height) = (rgba.width(), rgba.height());
                    if rgba.save(&callback_path).is_ok() {
                        // 把尺寸编码进文件名，避免跨线程再传一次图片。
                        let _ = std::fs::write(
                            callback_path.with_extension("size"),
                            format!("{width}x{height}"),
                        );
                        callback_flag.store(true, Ordering::Relaxed);
                    }
                }
            });

        // 推流必须在前台执行器上发起（rgpui 约束）。
        let foreground = cx.foreground_executor().clone();
        let stream_rx = source.stream(&foreground, frame_callback);

        // 等待推流真正启动，失败则直接回状态。
        let stream = match stream_rx.await {
            Ok(Ok(stream)) => stream,
            _ => {
                let _ = this.update(cx, |state, cx| {
                    state.capturing = false;
                    state.set_status("启动屏幕捕获失败", cx);
                });
                return;
            }
        };

        // 最多等待 5 秒首帧；超时一般是目标屏幕静止无帧或权限不足。
        let mut waited = 0;
        while !frame_saved.load(Ordering::Relaxed) && waited < 100 {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(50))
                .await;
            waited += 1;
        }
        // 显式释放推流，会话随之关闭（首帧已落盘，无需继续）。
        drop(stream);

        if !frame_saved.load(Ordering::Relaxed) {
            let _ = this.update(cx, |state, cx| {
                state.capturing = false;
                state.set_status("截取超时，请重试", cx);
            });
            return;
        }

        // 从伴随的 .size 文件读回图片尺寸。
        let (img_w, img_h) = read_frame_size(&frame_path).unwrap_or((0, 0));
        let pending = PendingCapture {
            frame_path,
            img_w,
            img_h,
            display_bounds,
            fullscreen,
        };
        // 在 UI 上下文打开全屏标注浮层，并恢复主视图状态。
        // 注意：此处已在实体的单次租借内，直接改 `state`，不要再嵌套 `update`。
        let _ = this.update(cx, |state, cx| {
            state.capturing = false;
            state.set_status("已截取，正在标注…", cx);
            open_overlay(cx.entity(), cx, pending);
        });
    })
    .detach();
}

/// 从候选源中挑选主显示器对应的捕获源。
fn pick_main_source(
    sources: &[Rc<dyn ScreenCaptureSource>],
) -> Option<Rc<dyn ScreenCaptureSource>> {
    // 先找标记为 main 的源。
    for source in sources {
        if let Ok(meta) = source.metadata() {
            let meta: SourceMetadata = meta;
            if meta.is_main.unwrap_or(false) {
                return Some(source.clone());
            }
        }
    }
    None
}

/// 读取首帧伴随的尺寸文件（`宽x高` 文本）。
fn read_frame_size(frame_path: &std::path::Path) -> Option<(u32, u32)> {
    let text = std::fs::read_to_string(frame_path.with_extension("size")).ok()?;
    let (w, h) = text.trim().split_once('x')?;
    Some((w.parse().ok()?, h.parse().ok()?))
}
