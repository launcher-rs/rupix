//! 截图捕获模块：从系统屏幕源截取首帧，落地为临时图片后打开标注浮层。
//!
//! rgpui 的屏幕捕获只有持续推流接口（`stream`），没有单帧 API，
//! 因此“截取一张图”实现为：启动推流 → 取第一帧的 RGBA → 存盘 → 停止推流。
//!
//! 多显示器：每块屏各取一帧，并记下与 rgpui 显示器 ID 的对应关系。
//! 区域截图 / 取色按「一块屏一个浮层」打开，全屏截图按物理坐标拼成虚拟桌面并集。

use std::{
    path::PathBuf,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use rgpui::{
    AsyncWindowContext, Bounds, Context, DisplayId, Pixels, RenderImage, ScreenCaptureSource,
    SourceMetadata, WeakEntity, Window,
};

use crate::{
    app::{PreviewImage, RupixApp, finish_capture},
    i18n::{tr, tr_fmt},
    overlay::open_overlays,
};

/// 单块显示器上的首帧捕获结果（区域截图 / 取色 / 全屏共用）。
pub struct CapturedFrame {
    /// 首帧落盘的临时图片路径（裁剪与采样的像素来源）。
    pub frame_path: PathBuf,
    /// 全图宽度（图片物理像素）。
    pub img_w: u32,
    /// 全图高度（图片物理像素）。
    pub img_h: u32,
    /// 该显示器逻辑边界（浮层窗口定位用）。
    pub display_bounds: Bounds<Pixels>,
    /// 对应 rgpui 显示器 ID（无显示器信息时为 None）。
    pub display_id: Option<DisplayId>,
    /// 预解码的首帧（同步图片源用；None 时回退路径异步加载）。
    pub frame_image: Option<Arc<RenderImage>>,
}

/// 显示器与系统捕获源的配对结果（异步阶段逐屏取首帧用）。
struct CaptureTarget {
    /// 对应 rgpui 显示器 ID（无显示器信息时为 None）。
    display_id: Option<DisplayId>,
    /// 显示器逻辑边界（浮层窗口定位用）。
    display_bounds: Bounds<Pixels>,
    /// 对应的系统捕获源。
    source: Rc<dyn ScreenCaptureSource>,
}

/// 单块屏捕获失败原因（仅用于日志与整体失败时的状态文案）。
#[derive(Clone, Copy)]
enum CaptureFailure {
    /// 推流启动失败。
    StreamStart,
    /// 推流启动但等不到首帧。
    Timeout,
}

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
    state.set_status(&tr("cap.capturing"), cx);

    // 显示器列表与主屏 ID 必须在主线程取好，再进异步捕获流程。
    let displays = display_targets(cx);
    let primary_id = cx.primary_display().map(|display| display.id());
    cap_log(&format!(
        "任务启动：显示器 {} 台，主屏 {:?}",
        displays.len(),
        primary_id
    ));

    cx.spawn_in(window, async move |this, cx| {
        // 首帧捕获失败已在内部回状态，这里只处理成功后的分流。
        let Some(frames) = capture_displays(&this, cx, displays, primary_id).await else {
            return;
        };
        // 在 UI 上下文恢复主视图状态，并按模式分流。
        // 注意：此处已在实体的单次租借内，直接改 `state`，不要再嵌套 `update`。
        let updated = this
            .update(cx, |state, cx| {
                if fullscreen {
                    // 全屏截图：多屏按物理坐标拼成并集，再统一收尾。
                    state.capturing = false;
                    match compose_desktop(&frames) {
                        Ok(preview) => {
                            let (_, auto_note) = finish_capture(state, cx, &preview);
                            let msg = tr_fmt(
                                "cap.full_done",
                                &[
                                    ("w", &preview.w.to_string()),
                                    ("h", &preview.h.to_string()),
                                    ("note", &auto_note),
                                ],
                            );
                            state.set_status(&msg, cx);
                        }
                        Err(error) => {
                            let msg = tr_fmt("cap.full_fail", &[("error", &error)]);
                            state.set_status(&msg, cx);
                        }
                    }
                } else {
                    // 区域截图：每块屏各开一层遮罩浮层，可在任意屏幕上框选。
                    // 浮层打开期间保持 capturing = true，阻止热键再次触发导致浮层叠加；
                    // 完成 / 取消时由浮层复位（见 overlay::finish / cancel）。
                    state.set_status(&tr("cap.region_hint"), cx);
                    if !open_overlays(cx.entity(), cx, frames) {
                        // 浮层没打开就没有人来复位状态，这里补上，避免后续截图全被挡住。
                        state.capturing = false;
                        state.set_status(&tr("cap.overlay_fail"), cx);
                    }
                }
            })
            .is_ok();
        cap_log(&format!("截图后续处理完成：update_ok={updated}"));
    })
    .detach();
}

/// 开始一次取色流程（托盘 / 热键 / 设置页共用入口）。
///
/// # 参数
/// - `state`: 主视图可变引用（调用方已持有实体租借，直接改状态，不要再 `update`）。
/// - `window`: 发起取色的窗口，用于挂载后台任务。
/// - `cx`: 主视图上下文。
pub fn begin_picker(state: &mut RupixApp, window: &mut Window, cx: &mut Context<RupixApp>) {
    // 同一时间只允许一轮操作，避免重复推流占用系统捕获会话。
    if state.capturing {
        return;
    }
    state.capturing = true;
    state.set_status(&tr("cap.picking"), cx);

    // 取色同样逐屏捕获，每块屏各开一层冻结帧浮层。
    let displays = display_targets(cx);
    let primary_id = cx.primary_display().map(|display| display.id());

    cap_log("取色任务启动，等待屏幕源列表");
    cx.spawn_in(window, async move |this, cx| {
        let Some(frames) = capture_displays(&this, cx, displays, primary_id).await else {
            return;
        };
        let updated = this
            .update(cx, |state, cx| {
                // 与区域截图同理：取色浮层打开期间保持 capturing，防止热键叠开第二轮；
                // 取色完成 / 取消时由浮层复位（见 picker::on_pick / cancel）。
                state.set_status(&tr("cap.picker_hint"), cx);
                if !crate::picker::open_pickers(cx.entity(), cx, frames) {
                    state.capturing = false;
                    state.set_status(&tr("cap.overlay_fail"), cx);
                }
            })
            .is_ok();
        cap_log(&format!("取色后续处理完成：update_ok={updated}"));
    })
    .detach();
}

/// 在当前线程枚举显示器（ID + 逻辑边界），无显示器信息时回退默认尺寸。
fn display_targets(cx: &rgpui::App) -> Vec<(Option<DisplayId>, Bounds<Pixels>)> {
    let displays = cx.displays();
    if displays.is_empty() {
        let fallback = Bounds::centered(None, rgpui::size(rgpui::px(1280.), rgpui::px(800.)), cx);
        return vec![(None, fallback)];
    }
    displays
        .iter()
        .map(|display| (Some(display.id()), display.bounds()))
        .collect()
}

/// 逐屏捕获首帧并落盘（区域 / 全屏 / 取色共享）。
///
/// 失败时内部已复位状态机并写清中文状态，返回 `None`；
/// 成功返回每块屏一帧的数据，调用方负责后续分流（浮层 / 拼接 / 取色）。
async fn capture_displays(
    this: &WeakEntity<RupixApp>,
    cx: &mut AsyncWindowContext,
    displays: Vec<(Option<DisplayId>, Bounds<Pixels>)>,
    primary_id: Option<DisplayId>,
) -> Option<Vec<CapturedFrame>> {
    // 捕获源枚举是异步 oneshot：AsyncWindowContext 没有 App 的屏幕源接口，
    // 经窗口更新借 App 拿到接收端，再挂后台任务等待。
    let sources_rx = match cx.update(|_, cx| cx.screen_capture_sources()) {
        Ok(rx) => rx,
        Err(_) => {
            let _ = this.update(cx, |state, cx| {
                state.capturing = false;
                state.set_status(&tr("cap.no_source"), cx);
            });
            return None;
        }
    };
    // 等待系统返回可捕获的屏幕 / 窗口列表。
    let sources: Vec<Rc<dyn ScreenCaptureSource>> = match sources_rx.await {
        Ok(Ok(sources)) if !sources.is_empty() => sources,
        other => {
            cap_log(&format!(
                "屏幕源获取失败或为空：{}",
                sources_result_desc(&other)
            ));
            let _ = this.update(cx, |state, cx| {
                state.capturing = false;
                state.set_status(&tr("cap.no_source"), cx);
            });
            return None;
        }
    };
    cap_log(&format!("拿到屏幕源 {} 个", sources.len()));

    let targets = pair_targets(&displays, primary_id, &sources);
    if targets.is_empty() {
        cap_log("没有可配对的显示器捕获源");
        let _ = this.update(cx, |state, cx| {
            state.capturing = false;
            state.set_status(&tr("cap.no_source"), cx);
        });
        return None;
    }

    // 逐屏取首帧：某块屏失败不拖累其余屏，至少拿到一帧就能继续。
    let mut frames = Vec::with_capacity(targets.len());
    let mut failures: Vec<CaptureFailure> = Vec::new();
    for (index, target) in targets.iter().enumerate() {
        match capture_target(target, index, cx).await {
            Ok(frame) => {
                cap_log(&format!(
                    "屏幕 {index} 首帧就绪：{}x{}",
                    frame.img_w, frame.img_h
                ));
                frames.push(frame);
            }
            Err(failure) => {
                cap_log(&format!("屏幕 {index} 捕获失败：{}", failure.desc()));
                failures.push(failure);
            }
        }
    }

    if frames.is_empty() {
        // 有任意一块屏推流启动过就按超时提示（更可能是权限 / 黑帧），否则报启动失败。
        let key = if failures
            .iter()
            .any(|f| matches!(f, CaptureFailure::Timeout))
        {
            "cap.timeout"
        } else {
            "cap.stream_fail"
        };
        let _ = this.update(cx, |state, cx| {
            state.capturing = false;
            state.set_status(&tr(key), cx);
        });
        return None;
    }
    Some(frames)
}

/// 捕获单个显示器源的首帧。
async fn capture_target(
    target: &CaptureTarget,
    index: usize,
    cx: &mut AsyncWindowContext,
) -> Result<CapturedFrame, CaptureFailure> {
    // 首帧落盘路径：系统临时目录 + 时间戳 + 屏序号，避免多轮 / 多屏互相覆盖。
    let frame_path = std::env::temp_dir().join(format!("rupix-frame-{}-{index}.png", temp_stamp()));
    let frame_saved = Arc::new(AtomicBool::new(false));

    // 推流回调跑在捕获线程，只能做 Send 操作：转 RGBA 并写文件。
    let callback_path = frame_path.clone();
    let callback_flag = frame_saved.clone();
    let frame_callback: Box<dyn Fn(rgpui::ScreenCaptureFrame) + Send> = Box::new(move |frame| {
        // 只要第一帧，后续帧直接丢弃，节省 IO。
        if callback_flag.load(Ordering::Relaxed) {
            return;
        }
        if let Some(rgba) = frame.to_rgba() {
            // 宽高留给浮层做缩放显示与裁剪换算。
            let (width, height) = (rgba.width(), rgba.height());
            if rgba.save(&callback_path).is_ok() {
                // 把尺寸编码进伴随文件，避免跨线程再传一次图片。
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
    let stream_rx = target.source.stream(&foreground, frame_callback);

    // 等待推流真正启动，失败则直接返回。
    let stream = match stream_rx.await {
        Ok(Ok(stream)) => stream,
        _ => return Err(CaptureFailure::StreamStart),
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
        return Err(CaptureFailure::Timeout);
    }

    // 从伴随的 .size 文件读回图片尺寸。
    let (img_w, img_h) = read_frame_size(&frame_path).unwrap_or((0, 0));
    // 首帧提前解码为 RenderImage（耗时挪到“正在截取”阶段），浮层首帧立现。
    let frame_image = decode_frame_image(&frame_path);
    Ok(CapturedFrame {
        frame_path,
        img_w,
        img_h,
        display_bounds: target.display_bounds,
        display_id: target.display_id,
        frame_image,
    })
}

/// 把显示器与系统捕获源配对（优先 ID，其次主屏标记，最后按枚举顺序兜底）。
fn pair_targets(
    displays: &[(Option<DisplayId>, Bounds<Pixels>)],
    primary_id: Option<DisplayId>,
    sources: &[Rc<dyn ScreenCaptureSource>],
) -> Vec<CaptureTarget> {
    let metas: Vec<Option<SourceMetadata>> = sources.iter().map(|s| s.metadata().ok()).collect();
    let mut used = vec![false; sources.len()];
    let mut assigned: Vec<Option<usize>> = vec![None; displays.len()];

    // 1) 显示器 ID 精确配对：Windows（HMONITOR）与 macOS（CGDirectDisplayID）可直接对上。
    for (display_index, (id, _)) in displays.iter().enumerate() {
        let Some(id) = id else { continue };
        let found = metas
            .iter()
            .enumerate()
            .find(|(source_index, meta)| !used[*source_index] && source_matches_display(meta, *id))
            .map(|(source_index, _)| source_index);
        if let Some(source_index) = found {
            used[source_index] = true;
            assigned[display_index] = Some(source_index);
        }
    }

    // 2) 主屏兜底：部分平台只提供 is_main 标记（如 macOS 的某些源）。
    if let Some(primary_id) = primary_id
        && let Some(display_index) = displays
            .iter()
            .enumerate()
            .find(|(index, (id, _))| assigned[*index].is_none() && *id == Some(primary_id))
            .map(|(index, _)| index)
    {
        let found = metas
            .iter()
            .enumerate()
            .find(|(source_index, meta)| {
                !used[*source_index] && meta.as_ref().and_then(|meta| meta.is_main).unwrap_or(false)
            })
            .map(|(source_index, _)| source_index);
        if let Some(source_index) = found {
            used[source_index] = true;
            assigned[display_index] = Some(source_index);
        }
    }

    // 3) 剩余按枚举顺序配对：X11 等平台两套 ID 体系不一致，靠顺序尽量对齐。
    for slot in assigned.iter_mut() {
        if slot.is_some() {
            continue;
        }
        if let Some(source_index) = used.iter().position(|used| !used) {
            used[source_index] = true;
            *slot = Some(source_index);
        }
    }

    // 组装并记录配对结果，便于副屏问题取证。
    let mut targets = Vec::new();
    for (display_index, (id, bounds)) in displays.iter().enumerate() {
        let Some(source_index) = assigned[display_index] else {
            continue;
        };
        let source_id = metas[source_index]
            .as_ref()
            .map(|meta| meta.id.to_string())
            .unwrap_or_else(|| "?".to_string());
        cap_log(&format!(
            "配对：显示器 {display_index}（{id:?}）→ 源 {source_id}"
        ));
        targets.push(CaptureTarget {
            display_id: *id,
            display_bounds: *bounds,
            source: sources[source_index].clone(),
        });
    }
    if targets.len() < displays.len() {
        cap_log(&format!(
            "有 {} 块屏没有可用捕获源，已跳过",
            displays.len() - targets.len()
        ));
    }
    targets
}

/// 判断捕获源是否属于指定显示器。
///
/// Windows 的 scap 源 ID 是 `HMONITOR` 截断到 u32，而 rgpui 保留完整 u64；
/// 句柄值超出 32 位时按低 32 位比较，否则副屏会永远匹配不上。
fn source_matches_display(meta: &Option<SourceMetadata>, display_id: DisplayId) -> bool {
    let Some(meta) = meta else { return false };
    let raw: u64 = display_id.into();
    raw == meta.id || (raw > u32::MAX as u64 && raw as u32 == meta.id as u32)
}

impl CaptureFailure {
    /// 失败原因（日志用中文描述）。
    fn desc(self) -> &'static str {
        match self {
            CaptureFailure::StreamStart => "推流启动失败",
            CaptureFailure::Timeout => "等待首帧超时",
        }
    }
}

/// 全屏截图：单屏直接复用首帧，多屏按物理坐标拼成虚拟桌面并集图。
fn compose_desktop(frames: &[CapturedFrame]) -> Result<PreviewImage, String> {
    if let [frame] = frames {
        return Ok(PreviewImage {
            path: frame.frame_path.clone(),
            w: frame.img_w,
            h: frame.img_h,
        });
    }

    // 显示器逻辑边界 × 该屏缩放比（由帧宽与逻辑宽反推）得到物理原点，
    // 混合 DPI 下也能把各屏摆回虚拟桌面的真实位置。
    let rects: Vec<(i64, i64)> = frames
        .iter()
        .map(|frame| {
            let scale = frame.img_w as f32 / frame.display_bounds.size.width.as_f32().max(1.0);
            (
                (frame.display_bounds.origin.x.as_f32() * scale).round() as i64,
                (frame.display_bounds.origin.y.as_f32() * scale).round() as i64,
            )
        })
        .collect();
    let min_x = rects.iter().map(|(x, _)| *x).min().unwrap_or(0);
    let min_y = rects.iter().map(|(_, y)| *y).min().unwrap_or(0);
    let max_x = frames
        .iter()
        .zip(&rects)
        .map(|(frame, (x, _))| x + frame.img_w as i64)
        .max()
        .unwrap_or(0);
    let max_y = frames
        .iter()
        .zip(&rects)
        .map(|(frame, (_, y))| y + frame.img_h as i64)
        .max()
        .unwrap_or(0);
    let (out_w, out_h) = ((max_x - min_x).max(1) as u32, (max_y - min_y).max(1) as u32);

    let mut canvas = image::RgbaImage::new(out_w, out_h);
    for (frame, (x, y)) in frames.iter().zip(rects) {
        let img = image::open(&frame.frame_path)
            .map_err(|error| format!("读取首帧失败：{error}"))?
            .into_rgba8();
        // 各屏帧都是物理像素，直接按物理坐标叠放，缩放差异已体现在帧尺寸里。
        image::imageops::overlay(&mut canvas, &img, x - min_x, y - min_y);
    }

    let dst = std::env::temp_dir().join(format!("rupix-full-{}.png", temp_stamp()));
    canvas
        .save(&dst)
        .map_err(|error| format!("保存全屏合成图失败：{error}"))?;
    Ok(PreviewImage {
        path: dst,
        w: out_w,
        h: out_h,
    })
}

/// 毫秒时间戳（临时文件名用）。
fn temp_stamp() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0)
}

/// 截图诊断日志：追加到系统临时目录，便于复现卡死时取证。
pub(crate) fn cap_log(msg: &str) {
    // 日志写失败不影响截图流程，直接忽略。
    let path = std::env::temp_dir().join("rupix-cap.log");
    let line = format!(
        "[pid={} {}] {msg}\n",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0)
    );
    use std::io::Write as _;
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = file.write_all(line.as_bytes());
    }
}

/// 描述屏幕源获取结果（日志用，不暴露内部错误细节给用户）。
// 类型复杂是 oneshot 嵌套 Result 的固有形状，日志拼接用，暂不拆分。
#[allow(clippy::type_complexity)]
fn sources_result_desc(
    result: &Result<
        Result<Vec<Rc<dyn ScreenCaptureSource>>, impl std::fmt::Debug>,
        impl std::fmt::Debug,
    >,
) -> String {
    match result {
        Ok(Ok(sources)) => format!("空列表（{} 个源）", sources.len()),
        Ok(Err(error)) => format!("系统枚举失败：{error:?}"),
        Err(error) => format!("通道失败：{error:?}"),
    }
}

/// 把首帧 PNG 解码为可直接渲染的 `RenderImage`（与 rgpui 内置 img 解码一致）。
///
/// 开窗前在后台做掉：解码耗时（debug 下约半秒）发生在“正在截取”阶段，
/// 浮层首帧即可呈现冻结图，不会出现先透活屏、半秒后才变灰的闪烁。
/// 解码失败返回 None，调用方回退到路径异步加载。
fn decode_frame_image(frame_path: &std::path::Path) -> Option<Arc<rgpui::RenderImage>> {
    // 读盘失败或格式非法都按 None 处理，不阻断截图流程。
    let bytes = std::fs::read(frame_path).ok()?;
    let mut rgba = image::load_from_memory(&bytes).ok()?.into_rgba8();
    // 与 rgpui img 解码一致：RGBA 转 BGRA。
    let (chunks, _) = rgba.as_chunks_mut::<4>();
    for pixel in chunks {
        pixel.swap(0, 2);
    }
    // Vec<Frame> 经 From 直接转 SmallVec，不引入新依赖。
    Some(Arc::new(rgpui::RenderImage::new(vec![image::Frame::new(
        rgba,
    )])))
}

/// 读取首帧伴随的尺寸文件（`宽x高` 文本）。
fn read_frame_size(frame_path: &std::path::Path) -> Option<(u32, u32)> {
    let text = std::fs::read_to_string(frame_path.with_extension("size")).ok()?;
    let (w, h) = text.trim().split_once('x')?;
    Some((w.parse().ok()?, h.parse().ok()?))
}
