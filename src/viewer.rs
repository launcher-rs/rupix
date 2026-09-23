//! 图片预览与标注窗口：截图完成后独立展示，像图片浏览器一样查看并标注。
//!
//! - 打开时窗口大小固定（跟随显示器约 75%），图片在图片区内居中显示，
//!   大图按比例缩小完整落入，小图居中 100% 显示；之后窗口固定不再变化；
//! - 缩放只改变图片大小（图片居中，超出可拖拽平移），窗口不再跟随变化；
//! - 系统标题栏显示窗口名与尺寸，客户区顶部压缩为两排工具条：
//!   - 操作排：缩小 / 放大 / 适应 / 撤销 / 重做 / 线宽 / 文字底色 / 复制 / 保存；
//!   - 标注排：选择 / 矩形 / 椭圆 / 箭头 / 画笔 / 荧光笔 / 橡皮擦 / 马赛克 / 文字 /
//!     预设颜色（精简，自定义色走取色器）/ 操作提示；
//! - 标注画在覆盖图片的 `canvas` 上实时预览，
//!   保存 / 复制时经 `export::render_export` 烧录为同一坐标系的最终图；
//! - 文字工具支持：点击新建、点击已有文字重新编辑、Ctrl+拖动移动；
//!   每条文字底色独立，底色块点开取色器可设底色（默认跟随字色自适应）。
//! - 默认矩形工具（打开即画，单击不成形）；空格按住 / 中键拖拽可临时平移，与工具无关。
//! - `Ctrl + 滚轮`、`+` / `-` / `0`、`V` / `1-8` 切工具、Esc 快捷键齐全。
//! - 操作排“识别”按钮后台跑 OCR（PP-OCRv5 mobile）：图上编号矩形框标出行位置，
//!   右侧面板默认列出全部文字，点框 / 点行双向选中（文本进剪贴板），确认后可落为标注。

use std::{path::PathBuf, sync::Arc};

use ab_glyph::FontVec;
use image::RgbaImage;
use rgpui::{
    Bounds, Context, CursorStyle, Div, Entity, FocusHandle, KeyDownEvent, KeyUpEvent, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, Path, PathBuilder, Pixels, Render, Rgba,
    ScrollDelta, ScrollWheelEvent, SharedString, Slider, SliderEvent, SliderState, Subscription,
    TitlebarOptions, Window, WindowBounds, WindowOptions, canvas,
    components::ColorPickerState,
    div, fill, img,
    input_ui::{Input, InputState},
    point,
    prelude::*,
    px, rgb, size,
};

use crate::{
    app::{PreviewImage, RupixApp, load_save_dir},
    capture::cap_log,
    export::{
        copy_image_and_file_to_clipboard, copy_png_to_clipboard, load_rgba, load_system_font,
        measure_text_width, png_bytes, render_export,
    },
    i18n::{tr, tr_fmt},
    ocr,
    shapes::{AnnoColor, PixelRect, Shape},
};

/// 最小缩放（10%）。
const MIN_ZOOM: f32 = 0.1;
/// 最大缩放（800%）。
const MAX_ZOOM: f32 = 8.0;
/// 单次按钮缩放步进（×1.25）。
const ZOOM_STEP: f32 = 1.25;
/// 工具条单排高度（逻辑像素）。
const ROW_H: f32 = 40.0;
/// 顶部工具条总高度（客户区两排：操作 / 标注，系统标题栏在其外不计入）。
const TOOLBAR_H: f32 = ROW_H * 2.0;
/// 底部状态栏高度（逻辑像素）。
const STATUS_H: f32 = 30.0;
/// 预览窗口最小宽（逻辑像素，固定窗口下限，保证两排工具条不拥挤）。
const MIN_VIEWER_W: f32 = 720.0;
/// 预览窗口最小总高（逻辑像素，含两排工具条与状态栏）。
const MIN_VIEWER_H: f32 = 520.0;
/// 画笔默认线宽（图片像素）。
const DEFAULT_WIDTH: f32 = 4.0;
/// 橡皮擦半径（图片像素，画笔类额外叠加自身半宽）。
const ERASER_RADIUS: f32 = 8.0;
/// 文字标注默认字号（图片像素）。
const DEFAULT_TEXT_SIZE: f32 = 28.0;
/// 线宽最小值（图片像素，滑杆下限）。
const WIDTH_MIN: f32 = 1.0;
/// 线宽最大值（图片像素，滑杆上限）。
const WIDTH_MAX: f32 = 30.0;

/// 规范化线宽：钳制到滑杆范围并取整（图片像素，滑杆回调与步进共用）。
fn normalize_width(w: f32) -> f32 {
    w.clamp(WIDTH_MIN, WIDTH_MAX).round()
}

/// 标注工具（工具条标注排）。
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
    /// 荧光笔：半透明粗线高亮（导出时自动半透明加粗）。
    Highlighter,
    /// 橡皮擦：点击 / 拖动删除命中的标注。
    Eraser,
    /// 马赛克：矩形区域打码。
    Mosaic,
    /// 文字（点选落点，内容取输入框）。
    Text,
}

impl AnnoTool {
    /// 快捷键切工具：`V` 选择，`1-8` 按标注排顺序（矩形 / 椭圆 / 箭头 / 画笔 / 荧光笔 / 橡皮擦 / 马赛克 / 文字）。
    fn from_shortcut(key: &str) -> Option<Self> {
        match key {
            "v" => Some(Self::Select),
            "1" => Some(Self::Rect),
            "2" => Some(Self::Ellipse),
            "3" => Some(Self::Arrow),
            "4" => Some(Self::Pen),
            "5" => Some(Self::Highlighter),
            "6" => Some(Self::Eraser),
            "7" => Some(Self::Mosaic),
            "8" => Some(Self::Text),
            _ => None,
        }
    }
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
    /// 切换标注工具。
    Tool(AnnoTool),
    /// 撤销一个标注。
    Undo,
    /// 重做一个标注。
    Redo,
    /// 切换标注颜色。
    Color(AnnoColor),
    /// 步进调节线宽（图片像素，滑杆旁 − / + 与 [ ] 键共用）。
    WidthNudge(f32),
    /// 切换文字背景显示。
    TextBg,
    /// 识别图中文字（后台跑 OCR，结果进面板，可复制 / 落为标注）。
    Ocr,
    /// 复制面板里全部识别文本。
    OcrCopyAll,
    /// 点框 / 点行选中的识别行：两侧同步高亮，文本进剪贴板（状态栏回显确认）。
    OcrSelectLine(usize),
    /// 把面板里的识别行落成文字标注。
    OcrApply,
    /// 关闭 OCR 结果面板。
    CloseOcrPanel,
    /// 打开取色器并设为底色目标（点底色块用；取色只改字色）。
    PickBg,
    /// 展开 / 收起自定义颜色选择器。
    ToggleColorPicker,
    /// 打开最近一次保存的文件（系统默认程序）。
    OpenSavedFile,
    /// 打开最近一次保存文件所在目录（系统文件管理器）。
    OpenSavedDir,
    /// 关闭确认：保存并关闭（有未保存修改时）。
    ConfirmSaveClose,
    /// 关闭确认：不保存，直接关闭（有未保存修改时）。
    ConfirmDiscardClose,
    /// 关闭确认：取消关闭，继续编辑。
    CancelCloseConfirm,
}

/// 预览窗口初始布局：窗口大小固定（跟随显示器），图片居中、仅缩小适配。
///
/// 小截图不再撑出小窗口，而是在固定图片区内居中 100% 显示；
/// 大截图按比例缩小，保证完整落入图片区（之后可用 + / Ctrl+滚轮放大查看）。
///
/// # 参数
/// - `disp_w` / `disp_h`：主显示器逻辑尺寸。
/// - `img_w` / `img_h`：图片像素尺寸（宽高均大于 0）。
///
/// # 返回
/// `(图片区宽, 图片区高, 适应缩放)`，缩放只缩小不放大。
fn viewer_layout(disp_w: f32, disp_h: f32, img_w: u32, img_h: u32) -> (f32, f32, f32) {
    // 窗口跟随显示器固定大小：宽约 75%、总高约 72%，上下左右留边框余量；
    // max/min 链式写法避免极小显示器上 clamp 出现 min > max 的 panic。
    let win_w = (disp_w * 0.75)
        .round()
        .max(MIN_VIEWER_W)
        .min(disp_w - 20.0)
        .max(480.0);
    let win_h = (disp_h * 0.72)
        .round()
        .max(MIN_VIEWER_H)
        .min(disp_h - 20.0)
        .max(360.0);
    let area_w = win_w;
    let area_h = (win_h - TOOLBAR_H - STATUS_H).max(200.0);
    // 适应缩放：完整落入图片区，只缩小不放大（小图居中 100% 显示更清晰）。
    let fit = (area_w / img_w as f32)
        .min(area_h / img_h as f32)
        .clamp(MIN_ZOOM, 1.0);
    (area_w, area_h, fit)
}

/// 打开图片预览窗口。
///
/// # 参数
/// - `main`: 主视图实体（复制 / 保存后回写状态用）。
/// - `cx`: 调用方上下文（全屏截图与区域浮层各调一次，泛型兼容）。
/// - `preview`: 要展示的图片（路径 + 原图尺寸）。
pub fn open_viewer<T: 'static>(main: Entity<RupixApp>, cx: &mut Context<T>, preview: PreviewImage) {
    // 固定窗口：按显示器算出图片区，图片居中、仅缩小适配。
    let (disp_w, disp_h) = cx
        .primary_display()
        .map(|d| {
            let s = d.bounds().size;
            (s.width.as_f32(), s.height.as_f32())
        })
        .unwrap_or((1920.0, 1080.0));
    let (area_w, area_h, fit) = viewer_layout(disp_w, disp_h, preview.w, preview.h);
    cap_log(&format!(
        "预览窗口打开：img={}x{} area={}x{} fit={fit:.2}",
        preview.w, preview.h, area_w as u32, area_h as u32
    ));
    // 窗口高度 = 两排工具条 + 图片区 + 底部状态栏。
    let bounds = Bounds::centered(
        None,
        size(px(area_w), px(area_h + TOOLBAR_H + STATUS_H)),
        cx,
    );
    let opened = cx.open_window(
        viewer_window_options(bounds, &preview),
        move |window, cx| {
            let view = cx.new(|cx| {
                ImageViewer::new(
                    window,
                    cx,
                    main.clone(),
                    preview.clone(),
                    fit,
                    area_w,
                    area_h,
                )
            });
            // 打开即聚焦，Esc / 快捷键立即可用。
            let handle = view.read(cx).focus_handle.clone();
            window.focus(&handle, cx);
            // 系统 X 也要过脏检查，否则有标注点 X 会静默丢失。
            guard_viewer_close(window, cx, view.clone());
            view
        },
    );
    if let Err(error) = opened {
        cap_log(&format!("预览窗口打开失败：{error:?}"));
    }
}

/// 注册系统标题栏 X 关闭拦截：之前只有 Esc 走脏检查，点 X 会静默丢弃未保存标注。
///
/// 有未保存修改时先提交输入中的文字、丢弃未成形草稿，展开窗内确认浮层并取消本次系统关闭；
/// 无修改直接放行；确认浮层里的“不保存”走 `remove_window`（不触发本回调），不会自锁。
fn guard_viewer_close(window: &mut Window, cx: &mut rgpui::App, view: Entity<ImageViewer>) {
    window.on_window_should_close(cx, move |window, cx| {
        // `Entity::update` 直接返回值（`WeakEntity` 才包 `Result`），这里无需拆包。
        let veto = view.update(cx, |this, cx| {
            // 输入中的文字先落为标注，避免点 X 把刚打的字静默丢掉。
            if this.text_editing.is_some() {
                this.commit_text(window, cx);
            }
            // 未成形草稿本就不会进保存结果，这里直接丢弃，只看已提交的脏标记。
            this.draft = None;
            this.anchor = None;
            if this.dirty {
                this.confirm_close = true;
                cx.notify();
                true
            } else {
                false
            }
        });
        !veto
    });
}

/// 构造预览窗口选项：系统标题栏显示带图片尺寸的窗口名。
///
/// # 参数
/// - `bounds`: 窗口初始边界（含两排工具条与状态栏占高）。
/// - `preview`: 要展示的图片（取宽高写入窗口 / 任务栏标题）。
fn viewer_window_options(bounds: Bounds<Pixels>, preview: &PreviewImage) -> WindowOptions {
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions {
            title: Some(SharedString::from(tr_fmt(
                "viewer.title",
                &[("w", &preview.w.to_string()), ("h", &preview.h.to_string())],
            ))),
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// 从本地图片路径构建预览信息（校验文件有效性）。
///
/// # 参数
/// - `path`: 用户选择的图片文件路径。
///
/// # 返回
/// 有效图片返回 `Some(PreviewImage)`，非图片 / 不存在返回 `None`。
pub fn preview_from_path(path: &std::path::Path) -> Option<PreviewImage> {
    // 先确认是文件，再读图片尺寸（不支持的格式直接返回 None）。
    if !path.is_file() {
        return None;
    }
    let (w, h) = image::image_dimensions(path).ok()?;
    if w == 0 || h == 0 {
        return None;
    }
    Some(PreviewImage {
        path: path.to_path_buf(),
        w,
        h,
    })
}

/// 从 `App` 上下文打开本地图片编辑（托盘菜单入口用）。
///
/// 原生文件对话框选图后，用预览窗口打开做标注；保存时另存新文件，不覆盖原图。
///
/// # 参数
/// - `main`: 设置窗口实体（保存历史回写用）。
/// - `cx`: 应用上下文。
pub fn open_image_for_edit_in_app(main: Entity<RupixApp>, cx: &mut rgpui::App) {
    // 原生打开对话框：只选单个图片文件。
    let receiver = cx.prompt_for_paths(rgpui::PathPromptOptions {
        files: true,
        directories: false,
        multiple: false,
        prompt: Some(tr("image.pick_prompt").into()),
    });
    cx.spawn(async move |cx| {
        let Ok(Ok(Some(paths))) = receiver.await else {
            return;
        };
        let Some(path) = paths.into_iter().next() else {
            return;
        };
        let Some(preview) = preview_from_path(&path) else {
            // 非图片：回写设置窗口状态，提示用户。
            cx.update(|cx| {
                cx.update_entity(&main, |state, cx| {
                    state.set_status(&tr("image.invalid"), cx);
                });
            });
            return;
        };
        cx.update(|cx| {
            open_viewer_in_app(main.clone(), cx, preview);
        });
    })
    .detach();
}

/// 从 `App` 上下文直接按路径打开预览窗口（异步选图任务内部用）。
fn open_viewer_in_app(main: Entity<RupixApp>, cx: &mut rgpui::App, preview: PreviewImage) {
    // 固定窗口布局与 `open_viewer` 一致：窗口跟随显示器，图片居中适配。
    let (disp_w, disp_h) = cx
        .primary_display()
        .map(|d| {
            let s = d.bounds().size;
            (s.width.as_f32(), s.height.as_f32())
        })
        .unwrap_or((1920.0, 1080.0));
    let (area_w, area_h, fit) = viewer_layout(disp_w, disp_h, preview.w, preview.h);
    cap_log(&format!(
        "预览窗口打开（本地图片）：img={}x{} area={}x{} fit={fit:.2}",
        preview.w, preview.h, area_w as u32, area_h as u32
    ));
    let bounds = Bounds::centered(
        None,
        size(px(area_w), px(area_h + TOOLBAR_H + STATUS_H)),
        cx,
    );
    let opened = cx.open_window(
        viewer_window_options(bounds, &preview),
        move |window, cx| {
            let view = cx.new(|cx| {
                ImageViewer::new(
                    window,
                    cx,
                    main.clone(),
                    preview.clone(),
                    fit,
                    area_w,
                    area_h,
                )
            });
            let handle = view.read(cx).focus_handle.clone();
            window.focus(&handle, cx);
            // 本地图片入口同样拦截系统 X，避免未保存标注被静默丢弃。
            guard_viewer_close(window, cx, view.clone());
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

/// 选择工具拖动标注时的临时状态。
struct DragShape {
    /// 被拖动的 shapes 索引。
    idx: usize,
    /// 上一次鼠标所在的图片像素坐标。
    last: (f32, f32),
}

/// 图片预览视图。
struct ImageViewer {
    /// 主视图实体（回写状态 / 最近文件）。
    main: Entity<RupixApp>,
    /// 展示的图片。
    preview: PreviewImage,
    /// 预览用底图像素（马赛克实时预览采样用）。
    base_image: Option<Arc<RgbaImage>>,
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
    /// 空格是否按住（按住时左键拖拽临时平移，与当前工具无关）。
    space_held: bool,
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
    /// 自定义颜色选择器状态。
    color_picker: Entity<ColorPickerState>,
    /// 是否展开颜色选择器面板。
    show_color_picker: bool,
    /// 当前线宽（图片像素）。
    width: f32,
    /// 线宽滑杆状态（操作排，拖动在 1–30px 内自由调节）。
    width_slider: Entity<SliderState>,
    /// 线宽滑杆订阅（持有以保持有效；字段本身无需读取）。
    #[allow(dead_code)]
    width_subscription: Subscription,
    /// 正在编辑的文字标注（点击后出现，回车/失焦提交）。
    text_editing: Option<TextEdit>,
    /// 当前文字底色：编辑中实时预览；未编辑时切换最近一条并作为新建默认（旧文字各自独立）。
    text_bg: bool,
    /// 当前底色颜色（取色器底色目标改这里；未手动指定时跟随字色自适应）。
    text_bg_color: AnnoColor,
    /// 底色是否被手动指定过（指定后换字色不再自动适配底色）。
    bg_color_custom: bool,
    /// 取色器当前目标：`false` 改字色，`true` 改底色。
    picker_for_bg: bool,
    /// Ctrl+拖动文字的临时状态。
    drag_text: Option<DragText>,
    /// 选择工具拖动普通标注的临时状态。
    drag_shape: Option<DragShape>,
    /// 选择工具悬停时是否命中已有标注（用于切换鼠标形状）。
    hover_shape: bool,
    /// 橡皮擦拖动中（抬起鼠标结束，期间移动持续删除）。
    erasing: bool,
    /// 缓存的系统字体（文字烧录用，懒加载）。
    font: Option<FontVec>,
    /// 字体是否已尝试加载（避免每次保存都读盘）。
    font_loaded: bool,
    /// 根视图焦点（键盘快捷键用）。
    focus_handle: FocusHandle,
    /// 底部状态栏文本（复制 / 保存结果就近显示，不依赖设置窗口）。
    status: SharedString,
    /// 最近一次保存的文件路径（状态栏“打开文件 / 打开目录”用）。
    last_saved: Option<PathBuf>,
    /// 是否有未保存的标注修改（Esc 关窗前据此决定是否弹出确认）。
    dirty: bool,
    /// OCR 是否正在后台运行（跑着时再点识别直接提示，不重开任务）。
    ocr_running: bool,
    /// 最近一次识别的行（面板展示用；落为标注只是快照，之后改标注不影响这里）。
    ocr_lines: Vec<ocr::OcrLine>,
    /// 是否展开 OCR 结果面板。
    show_ocr_panel: bool,
    /// OCR 双向选中的行号（点框 / 点行互相同步高亮，None 表示未选中）。
    selected_ocr: Option<usize>,
    /// 是否正在显示关闭确认浮层（有未保存修改时 Esc 弹出三选一）。
    confirm_close: bool,
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
        let view = cx.entity();
        let color_picker = cx.new(|cx| {
            ColorPickerState::new(_window, cx)
                .color(AnnoColor::RED.to_hsla())
                .on_change(move |color, _window, cx| {
                    let anno = AnnoColor::from_hsla(color);
                    view.update(cx, |this, cx| {
                        // 取色器双目标：底色模式改底色，默认改字色。
                        if this.picker_for_bg {
                            this.apply_bg_color(anno, cx);
                        } else {
                            this.apply_anno_color(anno);
                        }
                        cx.notify();
                    });
                })
        });
        // 线宽滑杆：1–30px 自由调节，拖动实时同步当前线宽。
        let width_slider = cx.new(|_| {
            SliderState::new()
                .min(WIDTH_MIN)
                .max(WIDTH_MAX)
                .step(1.0)
                .default_value(DEFAULT_WIDTH)
        });
        let width_subscription = cx.subscribe(
            &width_slider,
            |this: &mut Self, _state, event: &SliderEvent, cx| match event {
                SliderEvent::Change(v) | SliderEvent::Release(v) => {
                    this.width = normalize_width(v.start());
                    cx.notify();
                }
            },
        );
        // 马赛克预览直接采样底图；读取失败时仍保留导出能力，只是不显示实时像素化预览。
        let base_image = image::open(&preview.path)
            .ok()
            .map(|img| Arc::new(img.into_rgba8()));
        Self {
            main,
            preview,
            base_image,
            zoom: fit_zoom,
            fit_zoom,
            area_w,
            area_h,
            offset: (0.0, 0.0),
            pan_last: None,
            space_held: false,
            // 默认矩形：打开即画（单击不成形，误触成本低），纯查看按 V 切回选择。
            tool: AnnoTool::Rect,
            shapes: Vec::new(),
            redo_stack: Vec::new(),
            draft: None,
            anchor: None,
            color: AnnoColor::RED,
            color_picker,
            show_color_picker: false,
            width: DEFAULT_WIDTH,
            width_slider,
            width_subscription,
            text_editing: None,
            text_bg: true,
            // 默认红字配自适应底（浅色字黑底、深色字白底），用户取过底色后不再自动跟随。
            text_bg_color: AnnoColor::RED.auto_bg(),
            bg_color_custom: false,
            picker_for_bg: false,
            drag_text: None,
            drag_shape: None,
            hover_shape: false,
            erasing: false,
            font: None,
            font_loaded: false,
            focus_handle: cx.focus_handle(),
            status: SharedString::from(tr("viewer.ready")),
            last_saved: None,
            // 新窗口无标注，自然是干净状态。
            dirty: false,
            ocr_running: false,
            ocr_lines: Vec::new(),
            show_ocr_panel: false,
            selected_ocr: None,
            confirm_close: false,
        }
    }

    /// 设置预览窗口底部状态栏并刷新（复制 / 保存结果就近显示）。
    fn set_viewer_status(&mut self, msg: &str, cx: &mut Context<Self>) {
        self.status = SharedString::from(msg.to_string());
        cx.notify();
    }

    /// 是否有未保存的标注修改（Esc 关窗前检查用）。
    ///
    /// 草稿是未提交的绘制，同样视为未保存，避免 Esc 直接丢弃。
    fn has_unsaved_changes(&self) -> bool {
        self.dirty || self.draft.is_some()
    }

    /// 保存并关闭：保存成功才关窗，失败则留在窗口看状态栏原因。
    fn save_and_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.save_image(cx) {
            window.remove_window();
        }
    }

    /// 标题栏按钮（文案 + 动作，id 保证唯一，带悬停反馈）。
    fn bar_button(
        &self,
        cx: &mut Context<Self>,
        label: impl Into<String>,
        action: ViewerAction,
    ) -> impl IntoElement {
        let view = cx.entity();
        let label: String = label.into();
        div()
            .id(format!("viewer-btn-{label}"))
            .px_2()
            .py_1()
            .rounded_md()
            .text_sm()
            .cursor_pointer()
            .bg(rgb(0x3A3A3A))
            .text_color(rgb(0xFFFFFF))
            .hover(|style| style.bg(rgb(0x4A4A4A)))
            .child(label)
            .on_click(move |_, window, cx| {
                view.update(cx, |this, cx| this.handle(action, window, cx));
            })
    }

    /// 小按钮（操作 / 标注排用，选中态高亮，id 保证唯一；压缩占宽用 px_1）。
    fn mini_button(
        &self,
        cx: &mut Context<Self>,
        id: &str,
        label: impl Into<String>,
        action: ViewerAction,
        selected: bool,
    ) -> impl IntoElement {
        let view = cx.entity();
        let label: String = label.into();
        div()
            .id(format!("viewer-mini-{id}"))
            .px_1()
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

    /// 颜色块（标注排，选中态白色描边）。
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

    /// 底色块（操作排底色开关旁：显示当前底色 + hex，点击用取色器设置底色）。
    fn bg_swatch(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        let c = self.text_bg_color.as_text_bg_rgpui();
        div()
            .id("viewer-bg-swatch")
            .flex()
            .items_center()
            .gap_1()
            .cursor_pointer()
            .child(
                div()
                    .w(px(22.))
                    .h(px(22.))
                    .rounded_sm()
                    .bg(c)
                    .border_2()
                    .border_color(if self.picker_for_bg && self.show_color_picker {
                        rgb(0xFFFFFF)
                    } else {
                        rgb(0x3A3A3A)
                    }),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(0xCCCCCC))
                    .child(self.text_bg_color.to_hex()),
            )
            .on_click(move |_, window, cx| {
                view.update(cx, |this, cx| this.handle(ViewerAction::PickBg, window, cx));
            })
    }

    /// 应用标注字色：未手动指定过底色时，底色跟随字色自适应（黑字不配黑底）。
    fn apply_anno_color(&mut self, anno: AnnoColor) {
        self.color = anno;
        if !self.bg_color_custom {
            self.text_bg_color = anno.auto_bg();
        }
    }

    /// 应用文字底色：记为手动指定；编辑中靠提交落盘，未编辑直接写进最近一条并打开底色。
    fn apply_bg_color(&mut self, anno: AnnoColor, cx: &mut Context<Self>) {
        self.text_bg_color = anno;
        self.bg_color_custom = true;
        if self.text_editing.is_none()
            && let Some(idx) = latest_text_index(&self.shapes)
        {
            if let Shape::Text { bg, bg_color, .. } = &mut self.shapes[idx] {
                *bg_color = anno;
                *bg = true;
            }
            self.text_bg = true;
            self.redo_stack.clear();
            // 底色已变更，后续 Esc / 关窗需提示保存。
            self.dirty = true;
            let msg = tr("viewer.bg_color_applied");
            self.set_viewer_status(&msg, cx);
        }
        cx.notify();
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
            ViewerAction::Ocr => self.run_ocr(cx),
            ViewerAction::OcrCopyAll => self.copy_ocr_lines(cx),
            ViewerAction::OcrSelectLine(i) => self.select_ocr_line(i, cx),
            ViewerAction::OcrApply => self.apply_ocr_lines(cx),
            ViewerAction::CloseOcrPanel => {
                // 收起面板与提取层，选中一并清除，再点识别会被新结果替换。
                self.show_ocr_panel = false;
                self.selected_ocr = None;
                cx.notify();
            }
            ViewerAction::Save => {
                // 工具条保存只管落盘，返回值（成功与否）由状态栏反馈。
                let _ = self.save_image(cx);
            }
            ViewerAction::Tool(tool) => {
                // 切换工具时丢弃未提交的草稿，提交文字编辑，避免残留。
                if self.text_editing.is_some() {
                    self.commit_text(window, cx);
                }
                self.tool = tool;
                self.draft = None;
                self.anchor = None;
                self.drag_text = None;
                self.drag_shape = None;
                self.erasing = false;
                self.hover_shape = false;
                cx.notify();
            }
            ViewerAction::Undo => self.undo(cx),
            ViewerAction::Redo => self.redo(cx),
            ViewerAction::Color(color) => {
                self.apply_anno_color(color);
                cx.notify();
            }
            ViewerAction::WidthNudge(delta) => self.nudge_width(delta, window, cx),
            ViewerAction::TextBg => {
                // 按条独立，但保证每次点击都可见生效：
                // 编辑中只翻本次提交用的开关（编辑框实时预览）；
                // 未编辑则同步翻转最近一条文字的底色（旧文字互不影响），无文字时只切新建默认。
                self.text_bg = !self.text_bg;
                if self.text_editing.is_none()
                    && let Some(idx) = latest_text_index(&self.shapes)
                    && let Shape::Text { bg, .. } = &mut self.shapes[idx]
                {
                    *bg = self.text_bg;
                    self.redo_stack.clear();
                    // 底色已变更，后续 Esc / 关窗需提示保存。
                    self.dirty = true;
                    let msg = tr("viewer.text_bg_latest");
                    self.set_viewer_status(&msg, cx);
                } else if self.text_editing.is_none() {
                    let msg = tr("viewer.text_bg_default");
                    self.set_viewer_status(&msg, cx);
                }
                cx.notify();
            }
            ViewerAction::ToggleColorPicker => {
                // 取色按钮固定是字色目标，底色走底色块。
                self.picker_for_bg = false;
                self.show_color_picker = !self.show_color_picker;
                cx.notify();
            }
            ViewerAction::PickBg => {
                // 底色块：取色器切到底色目标并展开，拖动实时改底色。
                self.picker_for_bg = true;
                self.show_color_picker = true;
                cx.notify();
            }
            ViewerAction::OpenSavedFile => {
                // 用系统默认程序打开最近保存的文件。
                if let Some(path) = self.last_saved.clone() {
                    cx.open_with_system(&path);
                }
            }
            ViewerAction::OpenSavedDir => {
                // 打开所在目录：文件管理器选中文件需平台命令，这里打开目录即可。
                if let Some(path) = self.last_saved.clone()
                    && let Some(dir) = path.parent()
                {
                    cx.open_with_system(dir);
                }
            }
            ViewerAction::ConfirmSaveClose => self.save_and_close(window, cx),
            ViewerAction::ConfirmDiscardClose => window.remove_window(),
            ViewerAction::CancelCloseConfirm => {
                // 取消关闭：收起确认浮层，留在窗口继续编辑。
                self.confirm_close = false;
                cx.notify();
            }
        }
    }

    /// 撤销一个标注（进重做栈）。
    fn undo(&mut self, cx: &mut Context<Self>) {
        if let Some(shape) = self.shapes.pop() {
            self.redo_stack.push(shape);
            // 标注集合已变更，后续 Esc 关窗需提示保存。
            self.dirty = true;
            cx.notify();
        }
    }

    /// 重做一个标注（从重做栈取回）。
    fn redo(&mut self, cx: &mut Context<Self>) {
        if let Some(shape) = self.redo_stack.pop() {
            self.shapes.push(shape);
            // 标注集合已变更，后续 Esc 关窗需提示保存。
            self.dirty = true;
            cx.notify();
        }
    }

    /// 橡皮擦删除落点命中的标注（一次删光重叠的，删后清空重做栈）。
    fn erase_at(&mut self, p: (f32, f32), cx: &mut Context<Self>) {
        let mut removed = false;
        while let Some(idx) = erase_hit(&self.shapes, p, ERASER_RADIUS) {
            self.shapes.remove(idx);
            removed = true;
        }
        if removed {
            self.redo_stack.clear();
            // 删掉了已提交的标注，后续 Esc 关窗需提示保存。
            self.dirty = true;
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

    /// 当前缩放下图片是否可平移。
    fn can_pan(&self) -> bool {
        self.preview.w as f32 * self.zoom > self.area_w
            || self.preview.h as f32 * self.zoom > self.area_h
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

    /// 查找最顶层命中指定图片像素点的标注索引（从后往前找，后画的在上层）。
    fn hit_test_shape(&self, p: (f32, f32)) -> Option<usize> {
        self.shapes
            .iter()
            .enumerate()
            .rev()
            .find_map(|(i, shape)| shape.bbox().contains(p).then_some(i))
    }

    /// 左键按下：空格平移优先，选择工具优先移动标注，空白处平移；其他工具开始绘制。
    fn on_anno_down(&mut self, ev: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        // 空格按住时左键拖拽一律平移，与当前工具无关（默认矩形下放大看图用）。
        if self.space_held {
            self.on_pan_down(ev, cx);
            return;
        }
        if self.tool == AnnoTool::Select {
            // 如果点中已有标注，选择工具用于移动标注；点空白处才平移图片。
            if let Some(p) = self.to_image(ev.position).filter(|p| self.in_image(*p))
                && let Some(idx) = self.hit_test_shape(p)
            {
                if self.text_editing.is_some() {
                    self.commit_text(window, cx);
                }
                self.drag_shape = Some(DragShape { idx, last: p });
                self.redo_stack.clear();
                cx.notify();
                return;
            }
            self.on_pan_down(ev, cx);
            return;
        }
        // 橡皮擦：按下即删落点标注，拖动持续删除（抬起结束）。
        if self.tool == AnnoTool::Eraser {
            if let Some(p) = self.to_image(ev.position).filter(|p| self.in_image(*p)) {
                self.erasing = true;
                self.erase_at(p, cx);
                return;
            }
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
                // 原文字已从画布取下（取消编辑也会丢失），先记为脏。
                self.dirty = true;
                if let Shape::Text {
                    pos,
                    content,
                    color,
                    bg,
                    bg_color,
                    ..
                } = old
                {
                    self.color = color;
                    // 开关与底色跟随该条文字的旧值，改完提交只影响这一条。
                    self.text_bg = bg;
                    self.text_bg_color = bg_color;
                    self.bg_color_custom = true;
                    let input = cx.new(|cx| {
                        InputState::new(window, cx).placeholder(tr("viewer.text_placeholder"))
                    });
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
            AnnoTool::Highlighter => Shape::Highlighter {
                pts: vec![p],
                color,
                width,
            },
            AnnoTool::Mosaic => Shape::Mosaic {
                rect: PixelRect::from_points(p, p),
            },
            AnnoTool::Select | AnnoTool::Text | AnnoTool::Eraser => return,
        });
        cx.notify();
    }

    /// 鼠标移动：移动标注、Ctrl+拖动文字、平移（选择 / 空格 / 中键）、其他工具更新草稿。
    fn on_anno_move(&mut self, ev: &MouseMoveEvent, cx: &mut Context<Self>) {
        // 橡皮擦拖动中：沿途删除命中的标注。
        if self.erasing {
            if let Some(p) = self.to_image(ev.position).filter(|p| self.in_image(*p)) {
                self.erase_at(p, cx);
            }
            return;
        }
        // 选择工具拖动标注：按图片像素增量平移，并尽量限制在图内。
        if self.drag_shape.is_some() {
            let Some(mut p) = self.to_image(ev.position) else {
                return;
            };
            p.0 = p.0.clamp(0.0, self.preview.w as f32);
            p.1 = p.1.clamp(0.0, self.preview.h as f32);
            let Some(drag) = &mut self.drag_shape else {
                return;
            };
            let delta = (p.0 - drag.last.0, p.1 - drag.last.1);
            if delta.0 != 0.0 || delta.1 != 0.0 {
                if let Some(shape) = self.shapes.get_mut(drag.idx) {
                    move_shape_clamped(shape, delta, self.preview.w as f32, self.preview.h as f32);
                }
                drag.last = p;
                // 标注位置已变更，后续 Esc 关窗需提示保存。
                self.dirty = true;
                cx.notify();
            }
            return;
        }
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
                // 文字位置已变更，后续 Esc 关窗需提示保存。
                self.dirty = true;
            }
            cx.notify();
            return;
        }
        // 平移中（选择工具空白拖拽 / 空格临时平移 / 中键拖拽）：按位移累加偏移，与工具无关。
        if self.pan_last.is_some() {
            self.on_pan_move(ev, cx);
            return;
        }
        // 选择工具：未按下时仅更新悬停命中，用于显示“可移动标注”的鼠标形状。
        if self.tool == AnnoTool::Select {
            let hover_shape = self
                .to_image(ev.position)
                .filter(|p| self.in_image(*p))
                .and_then(|p| self.hit_test_shape(p))
                .is_some();
            if self.hover_shape != hover_shape {
                self.hover_shape = hover_shape;
                cx.notify();
            }
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
            Some(Shape::Pen { pts, .. }) | Some(Shape::Highlighter { pts, .. }) => {
                if pts.last() != Some(&p) {
                    pts.push(p);
                }
            }
            _ => return,
        }
        cx.notify();
    }

    /// 左键抬起：结束移动 / 平移（选择 / 空格 / 中键），Ctrl+拖动文字结束，其他工具提交草稿。
    fn on_anno_up(&mut self, cx: &mut Context<Self>) {
        // 橡皮擦拖动结束。
        if self.erasing {
            self.erasing = false;
            cx.notify();
            return;
        }
        // 选择工具移动标注结束。
        if self.drag_shape.take().is_some() {
            cx.notify();
            return;
        }
        // Ctrl+拖动文字结束。
        if self.drag_text.take().is_some() {
            cx.notify();
            return;
        }
        // 平移结束（左键或中键发起的都在这里收尾，绘制草稿时 pan_last 为空，不影响提交）。
        if self.pan_last.is_some() {
            self.on_pan_up(cx);
            return;
        }
        if self.tool == AnnoTool::Select {
            return;
        }
        // 过小的图形视为误触，直接丢弃。
        if let Some(draft) = self.draft.take()
            && draft_valid(&draft)
        {
            self.shapes.push(draft);
            self.redo_stack.clear();
            // 新标注已提交，后续 Esc 关窗需提示保存。
            self.dirty = true;
        }
        self.anchor = None;
        cx.notify();
    }

    /// 点击后进入文字编辑模式：在图片上创建 inline 输入框。
    fn place_text(&mut self, img_pos: (f32, f32), window: &mut Window, cx: &mut Context<Self>) {
        let input =
            cx.new(|cx| InputState::new(window, cx).placeholder(tr("viewer.text_placeholder")));
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
            // 提交时快照当前开关，新旧文字各自独立，之后再切开关不影响已提交的。
            bg: self.text_bg,
            bg_color: self.text_bg_color,
        };
        // 重新编辑时插回原位，否则追加到末尾。
        if let Some(idx) = editing.edit_index {
            let insert_at = idx.min(self.shapes.len());
            self.shapes.insert(insert_at, shape);
        } else {
            self.shapes.push(shape);
            self.redo_stack.clear();
        }
        // 文字已落盘为标注，后续 Esc 关窗需提示保存。
        self.dirty = true;
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

    /// 开始 OCR：后台线程识别整张底图，完成后落成文字标注并复制文本。
    ///
    /// 推理阻塞约 0.6s，绝不能占 UI 线程；经 `background_executor` 跑，
    /// 回来用弱引用更新（窗口已关则静默丢弃）。
    fn run_ocr(&mut self, cx: &mut Context<Self>) {
        // 重复点击直接提示，不叠任务。
        if self.ocr_running {
            let msg = tr("viewer.ocr_busy");
            self.set_viewer_status(&msg, cx);
            return;
        }
        // 模型缺失先指路，不进任务。
        if !ocr::models_available() {
            let msg = tr_fmt(
                "viewer.ocr_no_models",
                &[("dir", &ocr::models_hint().display().to_string())],
            );
            self.set_viewer_status(&msg, cx);
            return;
        }
        // 底图读内存（后台线程只传像素，不碰文件与视图）。
        let Some(base) = load_rgba(&self.preview.path) else {
            self.set_viewer_status(&tr("viewer.export_fail"), cx);
            return;
        };
        self.ocr_running = true;
        let msg = tr("viewer.ocr_running");
        self.set_viewer_status(&msg, cx);
        cx.spawn(async move |weak, cx| {
            // 推理在后台池阻塞跑，主线程只等结果。
            let out = cx
                .background_executor()
                .spawn(async move { ocr::recognize_image(&base) })
                .await;
            // 窗口已关则丢弃，不报错。
            let _ = weak.update(cx, |this, cx| this.finish_ocr(out, cx));
        })
        .detach();
    }

    /// OCR 完成：行存进面板展示，文本进剪贴板；**不自动落标注**。
    ///
    /// 自动 overlay 几个问题：重排字体与原图对不齐看着别扭，
    /// 长行点进编辑框会被固定尺寸截断。改为面板里看全文，
    /// 用户确认后再点“落为标注”（见 [`Self::apply_ocr_lines`]）。
    fn finish_ocr(&mut self, out: anyhow::Result<Vec<ocr::OcrLine>>, cx: &mut Context<Self>) {
        self.ocr_running = false;
        let lines = match out {
            Err(error) => {
                let msg = tr_fmt("viewer.ocr_fail", &[("error", &error.to_string())]);
                self.set_viewer_status(&msg, cx);
                return;
            }
            Ok(lines) => lines,
        };
        // 存快照并展开提取层（空结果就不打扰了），选中清零。
        self.ocr_lines = lines;
        self.show_ocr_panel = !self.ocr_lines.is_empty();
        self.selected_ocr = None;
        if self.ocr_lines.is_empty() {
            self.set_viewer_status(&tr("viewer.ocr_empty"), cx);
            return;
        }
        // 文本同时进剪贴板，方便直接粘贴到别处；面板里还能再复制。
        cx.write_to_clipboard(rgpui::ClipboardItem::new_string(ocr::lines_text(
            &self.ocr_lines,
        )));
        let msg = tr_fmt(
            "viewer.ocr_done",
            &[("n", &self.ocr_lines.len().to_string())],
        );
        self.set_viewer_status(&msg, cx);
    }

    /// 复制面板里全部识别文本（识别完成已自动复制一次，这里供稍后手动补拷）。
    fn copy_ocr_lines(&mut self, cx: &mut Context<Self>) {
        if self.ocr_lines.is_empty() {
            return;
        }
        cx.write_to_clipboard(rgpui::ClipboardItem::new_string(ocr::lines_text(
            &self.ocr_lines,
        )));
        let msg = tr_fmt(
            "viewer.ocr_copied",
            &[("n", &self.ocr_lines.len().to_string())],
        );
        self.set_viewer_status(&msg, cx);
    }

    /// 点框 / 点行选中识别行：两侧同步高亮，文本进剪贴板，状态栏回显前 30 字确认。
    fn select_ocr_line(&mut self, idx: usize, cx: &mut Context<Self>) {
        let Some(line) = self.ocr_lines.get(idx) else {
            return;
        };
        self.selected_ocr = Some(idx);
        cx.write_to_clipboard(rgpui::ClipboardItem::new_string(line.text.clone()));
        // 状态栏只回显前 30 字，长行不撑爆。
        let snippet: String = line.text.chars().take(30).collect();
        let msg = tr_fmt("viewer.ocr_copy_one", &[("text", &snippet)]);
        self.set_viewer_status(&msg, cx);
    }

    /// 把面板里的识别行落成文字标注（位置取检测框左上，字号按框宽拟合）。
    ///
    /// 拟合保证 overlay 上看着对齐，落成标注还是同一尺寸，所见即所得；
    /// 无字体时回退框高（`ocr::line_text_size`）。
    /// overlay 是近似排版（字体与原图不同，不会像素级重合），只在用户确认后执行；
    /// 一次多条，撤销需逐条退。
    fn apply_ocr_lines(&mut self, cx: &mut Context<Self>) {
        if self.ocr_lines.is_empty() {
            return;
        }
        // 字体懒加载（烧录同款），量字宽拟合用；失败也不阻断，最多不对齐。
        if !self.font_loaded {
            self.font = load_system_font();
            self.font_loaded = true;
        }
        for line in &self.ocr_lines {
            let base = line.rect.h;
            let size = match &self.font {
                Some(font) => ocr::fit_text_size(
                    measure_text_width(font, &line.text, base),
                    line.rect.w,
                    base,
                ),
                None => ocr::line_text_size(base),
            };
            self.shapes.push(Shape::Text {
                pos: (line.rect.x, line.rect.y),
                content: line.text.clone(),
                color: self.color,
                size,
                bg: self.text_bg,
                bg_color: self.text_bg_color,
            });
        }
        self.redo_stack.clear();
        // 新标注已提交，后续 Esc 关窗需提示保存。
        self.dirty = true;
        // 落完收起提取层并清选中，标注区清爽，继续用选择工具微调。
        self.show_ocr_panel = false;
        self.selected_ocr = None;
        let msg = tr_fmt(
            "viewer.ocr_applied",
            &[("n", &self.ocr_lines.len().to_string())],
        );
        self.set_viewer_status(&msg, cx);
    }

    /// 复制烧录图到系统剪贴板：图片与文件双格式，Word 粘贴为图片、文件夹粘贴为文件。
    fn copy_image(&mut self, cx: &mut Context<Self>) {
        let main = self.main.clone();
        // 先拿到 PNG 字节与对应的文件路径：无标注直接读底图文件，
        // 有标注则烧录后落到剪贴板缓存文件（HDROP 目标必须是一个真实存在的文件）。
        let (png, file_path) = if self.shapes.is_empty() {
            match std::fs::read(&self.preview.path) {
                Ok(bytes) => (Some(bytes), self.preview.path.clone()),
                Err(error) => {
                    let msg = tr_fmt("viewer.copy_read_fail", &[("error", &error.to_string())]);
                    self.set_viewer_status(&msg, cx);
                    return;
                }
            }
        } else {
            let Some(img) = self.burn_shapes() else {
                self.set_viewer_status(&tr("viewer.export_fail"), cx);
                return;
            };
            let Some(bytes) = png_bytes(&img) else {
                self.set_viewer_status(&tr("viewer.export_fail"), cx);
                return;
            };
            let path = clipboard_png_path();
            if let Some(parent) = path.parent()
                && let Err(error) = std::fs::create_dir_all(parent)
            {
                let msg = tr_fmt("viewer.cache_dir_fail", &[("error", &error.to_string())]);
                self.set_viewer_status(&msg, cx);
                return;
            }
            if let Err(error) = std::fs::write(&path, &bytes) {
                let msg = tr_fmt("viewer.cache_write_fail", &[("error", &error.to_string())]);
                self.set_viewer_status(&msg, cx);
                return;
            }
            (Some(bytes), path)
        };
        let png = png.expect("PNG 字节应在上一步就绪");

        // 双格式写入：一次搞定图片粘贴（Word / 画图）与文件粘贴（资源管理器）。
        match copy_image_and_file_to_clipboard(&png, &file_path) {
            Ok(()) => {
                let msg = tr("viewer.copy_ok");
                self.set_viewer_status(&msg, cx);
                main.update(cx, |state, cx| state.set_status(&msg, cx));
            }
            Err(file_error) => {
                // 双格式失败时回退到纯图片复制，保证 Word 粘贴仍可用。
                match copy_png_to_clipboard(cx, &png) {
                    Ok(()) => {
                        let msg = tr_fmt(
                            "viewer.copy_img_only",
                            &[("error", &file_error.to_string())],
                        );
                        self.set_viewer_status(&msg, cx);
                        main.update(cx, |state, cx| state.set_status(&msg, cx));
                    }
                    Err(error) => {
                        let msg = tr_fmt("viewer.copy_fail", &[("error", &error.to_string())]);
                        self.set_viewer_status(&msg, cx);
                        main.update(cx, |state, cx| {
                            state.set_status(&msg, cx);
                        });
                    }
                }
            }
        }
        cx.refresh_windows();
    }

    /// 保存烧录图到默认保存目录并记入最近文件（结果就近显示在预览底部状态栏）。
    ///
    /// # 返回
    /// 保存成功返回 `true`；成功后清除未保存标记并收起关闭确认浮层。
    fn save_image(&mut self, cx: &mut Context<Self>) -> bool {
        let main = self.main.clone();
        // 保存永远另存为带时间戳的新文件，不覆盖原图（截图自动保存的原图同样受保护）。
        // 目录被用户删掉时尝试重建，重建失败则本次保存会失败并提示（不崩溃）。
        let dir = load_save_dir();
        let _ = std::fs::create_dir_all(&dir);
        let dst = dir.join(format!(
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
            // 状态栏就近显示保存位置，并提供打开文件 / 打开目录快捷入口。
            let name = dst
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(&tr("viewer.image_fallback"))
                .to_string();
            self.last_saved = Some(dst.clone());
            // 保存成功：脏标记清零，关闭确认浮层也不再需要。
            self.dirty = false;
            self.confirm_close = false;
            // 状态栏显示文件名与所在目录名（自定义目录也能一眼看出存哪了）。
            let loc = dst
                .parent()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str())
                .unwrap_or("?");
            let status_msg = tr_fmt("viewer.saved", &[("name", &name), ("loc", loc)]);
            self.set_viewer_status(&status_msg, cx);
            let msg = tr_fmt("viewer.saved_full", &[("path", &dst.display().to_string())]);
            main.update(cx, |state, cx| {
                state.push_recent(dst, msg, cx);
            });
        } else {
            self.set_viewer_status(&tr("viewer.save_fail"), cx);
            main.update(cx, |state, cx| {
                state.set_status(&tr("viewer.save_fail"), cx);
            });
        }
        cx.refresh_windows();
        saved
    }

    /// 步进调节线宽并同步滑杆位置（− / + 按钮与 [ ] 快捷键共用）。
    fn nudge_width(&mut self, delta: f32, window: &mut Window, cx: &mut Context<Self>) {
        let width = normalize_width(self.width + delta);
        self.width = width;
        // 同步滑杆 thumb，避免按钮与滑杆显示不一致。
        self.width_slider.update(cx, |state, cx| {
            state.set_value(width, window, cx);
        });
        cx.notify();
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

/// 找到最近一条文字标注的索引（底色开关未编辑时作用于它，保证每次点击可见生效）。
fn latest_text_index(shapes: &[Shape]) -> Option<usize> {
    shapes.iter().rposition(|s| matches!(s, Shape::Text { .. }))
}

/// 剪贴板缓存图片路径：保持文件存在，保证复制后资源管理器能异步读取。
fn clipboard_png_path() -> PathBuf {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    std::env::temp_dir()
        .join("rupix")
        .join("clipboard")
        .join(format!("rupix-{millis}.png"))
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

/// 按图片像素增量移动标注。
fn move_shape(shape: &mut Shape, delta: (f32, f32)) {
    let (dx, dy) = delta;
    match shape {
        Shape::Pen { pts, .. } | Shape::Highlighter { pts, .. } => {
            for p in pts {
                p.0 += dx;
                p.1 += dy;
            }
        }
        Shape::Rect { rect, .. } | Shape::Ellipse { rect, .. } | Shape::Mosaic { rect } => {
            rect.x += dx;
            rect.y += dy;
        }
        Shape::Arrow { from, to, .. } => {
            from.0 += dx;
            from.1 += dy;
            to.0 += dx;
            to.1 += dy;
        }
        Shape::Text { pos, .. } => {
            pos.0 += dx;
            pos.1 += dy;
        }
    }
}

/// 橡皮擦命中判定：返回最顶层被命中标注的索引（后画的在上层，先删它）。
///
/// - 画笔 / 荧光笔：点到笔迹折线的距离 ≤ `radius + 线宽/2` 即命中；
/// - 其余：落点在外包矩形内即命中。
fn erase_hit(shapes: &[Shape], p: (f32, f32), radius: f32) -> Option<usize> {
    shapes.iter().enumerate().rev().find_map(|(i, shape)| {
        let hit = match shape {
            Shape::Pen { pts, width, .. } | Shape::Highlighter { pts, width, .. } => {
                stroke_distance(pts, p) <= radius + width / 2.0
            }
            _ => shape.bbox().contains(p),
        };
        hit.then_some(i)
    })
}

/// 点到折线的最短距离（单点退化为点距，空笔迹返回无穷大）。
fn stroke_distance(pts: &[(f32, f32)], p: (f32, f32)) -> f32 {
    if pts.is_empty() {
        return f32::INFINITY;
    }
    if pts.len() == 1 {
        return (p.0 - pts[0].0).hypot(p.1 - pts[0].1);
    }
    pts.windows(2)
        .map(|pair| {
            let (ax, ay) = pair[0];
            let (bx, by) = pair[1];
            let dx = bx - ax;
            let dy = by - ay;
            let t = (((p.0 - ax) * dx + (p.1 - ay) * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
            (p.0 - (ax + t * dx)).hypot(p.1 - (ay + t * dy))
        })
        .fold(f32::INFINITY, f32::min)
}

/// 移动标注并尽量限制在图片范围内，避免一拖就把标注完全拖出画面。
fn move_shape_clamped(shape: &mut Shape, delta: (f32, f32), img_w: f32, img_h: f32) {
    let bbox = shape.bbox();
    let mut dx = delta.0;
    let mut dy = delta.1;
    // 先根据当前外包矩形修正本次位移，保持标注主体留在图片范围内。
    if bbox.x + dx < 0.0 {
        dx = -bbox.x;
    }
    if bbox.y + dy < 0.0 {
        dy = -bbox.y;
    }
    if bbox.x + bbox.w + dx > img_w {
        dx = img_w - bbox.x - bbox.w;
    }
    if bbox.y + bbox.h + dy > img_h {
        dy = img_h - bbox.y - bbox.h;
    }
    move_shape(shape, (dx, dy));
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

/// 绘制马赛克实时预览：按导出逻辑的 12px 图片块采样底图并平铺到 canvas。
fn paint_mosaic_preview(
    window: &mut Window,
    base: &RgbaImage,
    rect: PixelRect,
    zoom: f32,
    origin_x: f32,
    origin_y: f32,
) {
    const BLOCK: i32 = 12;
    let x0 = rect.x.floor().max(0.0) as i32;
    let y0 = rect.y.floor().max(0.0) as i32;
    let x1 = (rect.x + rect.w).ceil().min(base.width() as f32) as i32;
    let y1 = (rect.y + rect.h).ceil().min(base.height() as f32) as i32;
    if x0 >= x1 || y0 >= y1 {
        return;
    }

    let mut by = y0;
    while by < y1 {
        let mut bx = x0;
        while bx < x1 {
            // 直接采样图片块左上角，预览与导出保持同一马赛克颗粒度。
            let sample = base.get_pixel(bx as u32, by as u32);
            let color = rgpui::rgba(
                ((sample[0] as u32) << 24)
                    | ((sample[1] as u32) << 16)
                    | ((sample[2] as u32) << 8)
                    | (sample[3] as u32),
            );
            let block_x1 = (bx + BLOCK).min(x1);
            let block_y1 = (by + BLOCK).min(y1);
            let bounds = Bounds::new(
                point(
                    px(origin_x + bx as f32 * zoom),
                    px(origin_y + by as f32 * zoom),
                ),
                size(
                    px((block_x1 - bx) as f32 * zoom),
                    px((block_y1 - by) as f32 * zoom),
                ),
            );
            window.paint_quad(fill(bounds, color));
            bx += BLOCK;
        }
        by += BLOCK;
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
                    // Esc 优先级：关闭确认（二次 Esc 取消）> 拾色器面板 > 脏检查关窗。
                    "escape" => {
                        if this.confirm_close {
                            // 确认浮层已展开：二次 Esc 视为取消，留在窗口继续编辑。
                            this.confirm_close = false;
                            cx.notify();
                        } else if this.show_ocr_panel {
                            // 先收 OCR 面板，识别行保留，再点识别会被新结果替换。
                            this.show_ocr_panel = false;
                            cx.notify();
                        } else if this.show_color_picker {
                            // 优先收起拾色器面板，避免想关面板却退出整个窗口。
                            this.show_color_picker = false;
                            cx.notify();
                        } else if this.has_unsaved_changes() {
                            // 有未保存标注：先弹确认浮层，不直接关窗丢数据。
                            // 未提交的草稿本就不会进保存结果，这里直接丢弃。
                            this.draft = None;
                            this.anchor = None;
                            this.confirm_close = true;
                            cx.notify();
                        } else {
                            window.remove_window();
                        }
                    }
                    // 确认浮层中回车 = 保存并关闭（与按钮一致）。
                    "enter" if this.confirm_close => {
                        this.save_and_close(window, cx);
                    }
                    "+" | "=" => this.zoom_by(ZOOM_STEP, cx),
                    "-" | "_" => this.zoom_by(1.0 / ZOOM_STEP, cx),
                    "0" => this.handle(ViewerAction::Fit, window, cx),
                    // [ / ] 步进调节线宽（与滑杆双向同步）。
                    "[" => this.handle(ViewerAction::WidthNudge(-1.0), window, cx),
                    "]" => this.handle(ViewerAction::WidthNudge(1.0), window, cx),
                    // 空格按住平移（文字输入框里打空格时不拦截，留给输入法）。
                    "space" if this.text_editing.is_none() => {
                        if !this.space_held {
                            this.space_held = true;
                            cx.notify();
                        }
                    }
                    // V / 1-8 切工具（文字输入中不拦截，避免打字跳工具）。
                    "v" | "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8"
                        if this.text_editing.is_none() =>
                    {
                        if let Some(tool) = AnnoTool::from_shortcut(ev.keystroke.key.as_str()) {
                            this.handle(ViewerAction::Tool(tool), window, cx);
                        }
                    }
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
            // 空格松开：结束临时平移（拖一半松空格也停，避免无空格平移继续跟手）。
            .on_key_up(cx.listener(|this, ev: &KeyUpEvent, _window, cx| {
                if ev.keystroke.key.as_str() == "space" && this.space_held {
                    this.space_held = false;
                    this.pan_last = None;
                    cx.notify();
                }
            }))
            // 操作排：查看缩放 + 撤销重做 + 线宽底色 + 文件操作（关闭走系统标题栏 / Esc）。
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .h(px(ROW_H))
                    .px_3()
                    .bg(rgb(0x2D2D2D))
                    .child(self.bar_button(cx, "−", ViewerAction::ZoomOut))
                    .child(div().text_sm().w(px(48.)).child(percent))
                    .child(self.bar_button(cx, "+", ViewerAction::ZoomIn))
                    .child(self.bar_button(cx, tr("viewer.fit"), ViewerAction::Fit))
                    .child(self.mini_button(
                        cx,
                        "undo",
                        tr("viewer.undo"),
                        ViewerAction::Undo,
                        false,
                    ))
                    .child(self.mini_button(
                        cx,
                        "redo",
                        tr("viewer.redo"),
                        ViewerAction::Redo,
                        false,
                    ))
                    // 线宽：− / + 步进 + 滑杆自由调节 + 当前值显示。
                    .child(self.mini_button(
                        cx,
                        "w-down",
                        "−",
                        ViewerAction::WidthNudge(-1.0),
                        false,
                    ))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(0xBBBBBB))
                                    .child(tr("viewer.line_width")),
                            )
                            .child(div().w(px(110.)).child(Slider::new(&self.width_slider)))
                            .child(
                                div()
                                    .text_xs()
                                    .w(px(38.))
                                    .text_color(rgb(0xCCCCCC))
                                    .child(format!("{}px", self.width.round() as u32)),
                            ),
                    )
                    .child(self.mini_button(cx, "w-up", "+", ViewerAction::WidthNudge(1.0), false))
                    .child(self.mini_button(
                        cx,
                        "text-bg",
                        tr("viewer.text_bg"),
                        ViewerAction::TextBg,
                        self.text_bg,
                    ))
                    // 底色块：显示当前底色（半透明预览），点击用取色器设置底色。
                    .child(self.bg_swatch(cx))
                    .child(self.bar_button(cx, tr("viewer.copy"), ViewerAction::Copy))
                    .child(self.bar_button(cx, tr("viewer.save"), ViewerAction::Save))
                    .child(self.bar_button(cx, tr("viewer.ocr"), ViewerAction::Ocr))
                    .child(
                        div()
                            .flex_1()
                            .overflow_hidden()
                            .text_xs()
                            .text_right()
                            .text_color(rgb(0x999999))
                            .child(tr("viewer.op_hint")),
                    ),
            )
            // 标注排：标注工具 + 预设颜色（精简，自定义色走取色器）+ 提示。
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
                        tr("tool.select"),
                        ViewerAction::Tool(AnnoTool::Select),
                        self.tool == AnnoTool::Select,
                    ))
                    .child(self.mini_button(
                        cx,
                        "tool-rect",
                        tr("tool.rect"),
                        ViewerAction::Tool(AnnoTool::Rect),
                        self.tool == AnnoTool::Rect,
                    ))
                    .child(self.mini_button(
                        cx,
                        "tool-ellipse",
                        tr("tool.ellipse"),
                        ViewerAction::Tool(AnnoTool::Ellipse),
                        self.tool == AnnoTool::Ellipse,
                    ))
                    .child(self.mini_button(
                        cx,
                        "tool-arrow",
                        tr("tool.arrow"),
                        ViewerAction::Tool(AnnoTool::Arrow),
                        self.tool == AnnoTool::Arrow,
                    ))
                    .child(self.mini_button(
                        cx,
                        "tool-pen",
                        tr("tool.pen"),
                        ViewerAction::Tool(AnnoTool::Pen),
                        self.tool == AnnoTool::Pen,
                    ))
                    .child(self.mini_button(
                        cx,
                        "tool-highlighter",
                        tr("tool.highlighter"),
                        ViewerAction::Tool(AnnoTool::Highlighter),
                        self.tool == AnnoTool::Highlighter,
                    ))
                    .child(self.mini_button(
                        cx,
                        "tool-eraser",
                        tr("tool.eraser"),
                        ViewerAction::Tool(AnnoTool::Eraser),
                        self.tool == AnnoTool::Eraser,
                    ))
                    .child(self.mini_button(
                        cx,
                        "tool-mosaic",
                        tr("tool.mosaic"),
                        ViewerAction::Tool(AnnoTool::Mosaic),
                        self.tool == AnnoTool::Mosaic,
                    ))
                    .child(self.mini_button(
                        cx,
                        "tool-text",
                        tr("tool.text"),
                        ViewerAction::Tool(AnnoTool::Text),
                        self.tool == AnnoTool::Text,
                    ))
                    // 预设颜色精简为 6 色，其余自定义色走取色器。
                    .child(self.color_swatch(cx, AnnoColor::RED))
                    .child(self.color_swatch(cx, AnnoColor::YELLOW))
                    .child(self.color_swatch(cx, AnnoColor::GREEN))
                    .child(self.color_swatch(cx, AnnoColor::BLUE))
                    .child(self.color_swatch(cx, AnnoColor::WHITE))
                    .child(self.color_swatch(cx, AnnoColor::BLACK))
                    .child(self.mini_button(
                        cx,
                        "color-picker",
                        tr("tool.picker"),
                        ViewerAction::ToggleColorPicker,
                        self.show_color_picker,
                    ))
                    // 当前色指示：色块 + hex，自定义色一眼可见（纯文字看不出颜色）。
                    .child({
                        let current = self.color.to_rgpui();
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(
                                div()
                                    .w(px(14.))
                                    .h(px(14.))
                                    .rounded_sm()
                                    .border_1()
                                    .border_color(rgb(0x666666))
                                    .bg(current),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(0xCCCCCC))
                                    .child(self.color.to_hex()),
                            )
                    })
                    // 线宽与文字底色在操作排，避免标注排过宽。
                    .child(
                        if self.tool == AnnoTool::Text && self.text_editing.is_some() {
                            div()
                                .text_xs()
                                .text_color(rgb(0x999999))
                                .child(tr("viewer.text_confirm"))
                        } else if self.tool == AnnoTool::Text {
                            div()
                                .text_xs()
                                .text_color(rgb(0x999999))
                                .child(tr("viewer.text_new"))
                        } else if self.tool == AnnoTool::Select {
                            div()
                                .text_xs()
                                .text_color(rgb(0x999999))
                                .child(tr("viewer.select_hint"))
                        } else if self.tool == AnnoTool::Eraser {
                            div()
                                .text_xs()
                                .text_color(rgb(0x999999))
                                .child(tr("viewer.eraser_hint"))
                        } else if self.tool == AnnoTool::Highlighter {
                            div()
                                .text_xs()
                                .text_color(rgb(0x999999))
                                .child(tr("viewer.marker_hint"))
                        } else {
                            div()
                                .text_xs()
                                .text_color(rgb(0x999999))
                                .child(tr("viewer.place_hint"))
                        },
                    ),
            )
            // 图片区。
            .child({
                let left = (self.area_w - dw) / 2.0 + self.offset.0;
                let top = (self.area_h - dh) / 2.0 + self.offset.1;
                let zoom = self.zoom;
                let area = div().flex_1().relative().overflow_hidden();
                // 平移中一律抓手；空格按住时预告可平移；OCR 模式下箭头（框上有单独手形）；
                // 其余按工具显示。
                let area = if self.pan_last.is_some() {
                    area.cursor(CursorStyle::ClosedHand)
                } else if self.space_held {
                    area.cursor(CursorStyle::OpenHand)
                } else if self.show_ocr_panel {
                    area.cursor(CursorStyle::Arrow)
                } else {
                    match self.tool {
                        AnnoTool::Select => {
                            if self.drag_text.is_some() || self.drag_shape.is_some() {
                                area.cursor(CursorStyle::ClosedHand)
                            } else if self.hover_shape || self.can_pan() {
                                area.cursor(CursorStyle::OpenHand)
                            } else {
                                area.cursor(CursorStyle::Arrow)
                            }
                        }
                        AnnoTool::Text => area.cursor(CursorStyle::IBeam),
                        AnnoTool::Eraser => area.cursor(CursorStyle::PointingHand),
                        AnnoTool::Rect
                        | AnnoTool::Ellipse
                        | AnnoTool::Arrow
                        | AnnoTool::Pen
                        | AnnoTool::Highlighter
                        | AnnoTool::Mosaic => area.cursor(CursorStyle::Crosshair),
                    }
                };
                let area = area
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, ev: &MouseDownEvent, window, cx| {
                            this.on_anno_down(ev, window, cx);
                        }),
                    )
                    // 中键拖拽平移（与工具无关，默认矩形下放大看图用）。
                    .on_mouse_down(
                        MouseButton::Middle,
                        cx.listener(|this, ev: &MouseDownEvent, _window, cx| {
                            this.on_pan_down(ev, cx);
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
                    .on_mouse_up(
                        MouseButton::Middle,
                        cx.listener(|this, _ev: &MouseUpEvent, _window, cx| {
                            this.on_pan_up(cx);
                        }),
                    )
                    .on_mouse_up_out(
                        MouseButton::Middle,
                        cx.listener(|this, _ev: &MouseUpEvent, _window, cx| {
                            this.on_pan_up(cx);
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
                    }));
                // 图片容器组装完后，OCR 提取层按需盖在最上（照片式原位文字）。
                let container = div()
                    .absolute()
                    .left(px(left))
                    .top(px(top))
                    .w(px(dw))
                    .h(px(dh))
                    .relative()
                    // 底图。
                    .child(img(self.preview.path.clone()).w(px(dw)).h(px(dh)))
                    // 文字标注层（每条独立底色，开关只决定新建默认值）。
                    .children({
                        let mut marks: Vec<Div> = Vec::new();
                        for shape in &self.shapes {
                            if let Shape::Text {
                                pos,
                                content,
                                color,
                                size,
                                bg,
                                bg_color,
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
                                // 按条底色：用该条自己的底色，透明度统一。
                                if *bg {
                                    d = d.bg(bg_color.as_text_bg_rgpui());
                                }
                                marks.push(d);
                            }
                        }
                        // 正在编辑的 inline 输入框（所见即所得：底色跟随当前开关与底色）。
                        if let Some(editing) = &self.text_editing {
                            let input_text_size = DEFAULT_TEXT_SIZE * zoom;
                            let input_border = self.color.to_rgpui();
                            let edit_bg = self.text_bg;
                            let edit_color = self.color;
                            let edit_bg_color = self.text_bg_color;
                            let mut editor = div()
                                .absolute()
                                .left(px(editing.container_pos.0))
                                .top(px(editing.container_pos.1))
                                .min_w(px(180.0))
                                .max_w(px(400.0))
                                .px_2()
                                .py_1()
                                .rounded_sm()
                                .border_1()
                                .border_color(input_border)
                                .text_color(edit_color.to_rgpui())
                                .text_size(px(input_text_size));
                            if edit_bg {
                                editor = editor.bg(edit_bg_color.as_text_bg_rgpui());
                            }
                            marks.push(
                                editor
                                    // 阻止输入框点击冒泡到图片区，避免触发 commit + place。
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|_, _ev, _, cx| {
                                            cx.stop_propagation();
                                        }),
                                    )
                                    .child(
                                        Input::new(&editing.input)
                                            .appearance(false)
                                            .bordered(false)
                                            .text_color(edit_color.to_rgpui())
                                            .text_size(px(input_text_size)),
                                    ),
                            );
                        }
                        marks
                    })
                    // 矢量标注画布。
                    .child({
                        let paint_shapes = self.shapes.clone();
                        let paint_draft = self.draft.clone();
                        let base_image = self.base_image.clone();
                        canvas(
                            move |bounds, _, _| bounds,
                            move |_bounds, paint_bounds, window, _| {
                                let origin = paint_bounds.origin;
                                for shape in paint_shapes.iter().chain(paint_draft.iter()) {
                                    if let Shape::Mosaic { rect } = shape
                                        && let Some(base) = &base_image
                                    {
                                        paint_mosaic_preview(
                                            window,
                                            base,
                                            *rect,
                                            zoom,
                                            origin.x.as_f32(),
                                            origin.y.as_f32(),
                                        );
                                    }
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
                    });
                let container = if self.show_ocr_panel {
                    container.child(self.ocr_overlay(cx, zoom))
                } else {
                    container
                };
                let area = area.child(container);
                let area = if self.show_color_picker {
                    area.child(
                        div()
                            .absolute()
                            .left(px(12.0))
                            .top(px(12.0))
                            .occlude()
                            // 颜色面板浮在图片上，事件到这里为止，避免拖滑块时触发标注绘制。
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|_, _ev, _, cx| cx.stop_propagation()),
                            )
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener(|_, _ev, _, cx| cx.stop_propagation()),
                            )
                            .on_mouse_move(cx.listener(|_, _ev, _, cx| cx.stop_propagation()))
                            .on_scroll_wheel(cx.listener(|_, _ev, _, cx| cx.stop_propagation()))
                            // 标题标明当前目标：字色还是底色，避免调错。
                            .child(
                                div()
                                    .px_2()
                                    .py_1()
                                    .text_xs()
                                    .text_color(rgb(0xCCCCCC))
                                    .child(tr(if self.picker_for_bg {
                                        "viewer.picker_bg"
                                    } else {
                                        "viewer.picker_fg"
                                    })),
                            )
                            .child(self.color_picker.clone()),
                    )
                } else {
                    area
                };
                // OCR 右侧面板：默认列出全部文字，确认浮层之下（确认优先模态）。
                let area = if self.show_ocr_panel {
                    area.child(self.ocr_side_panel(cx))
                } else {
                    area
                };
                // 关闭确认浮层：有未保存修改时 Esc 弹出，模态盖住图片区防误触。
                if self.confirm_close {
                    area.child(self.close_confirm(cx))
                } else {
                    area
                }
            })
            // 底部状态栏：复制 / 保存结果就近显示 + 图片信息。
            .child(self.status_bar(cx))
    }
}

impl ImageViewer {
    /// OCR 提取层（编号矩形框）：只画框，不重排文字。
    ///
    /// 之前 overlay 重排文字的教训：检测框偏大 / 并行时，拟合字撑爆行间没法看。
    /// 只画框则对齐天然精确（框即检测结果），文字一律走右侧面板看；
    /// 点框 / 点行双向选中高亮，复制走状态栏与剪贴板。
    /// 框层吞掉左键按下 / 弹起 / 移动（OCR 模式下图片区不落笔，见 `on_anno_down` 守卫），
    /// 滚轮放行（Ctrl+滚轮照常缩放）。
    fn ocr_overlay(&self, cx: &mut Context<Self>, zoom: f32) -> impl IntoElement {
        let selected = self.selected_ocr;
        div()
            .absolute()
            .left(px(0.0))
            .top(px(0.0))
            .right(px(0.0))
            .bottom(px(0.0))
            .occlude()
            // 只截停左键按下 / 弹起（OCR 模式下图片区不落笔）；
            // 移动事件放行，中键 / 空格平移照常可用。
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _ev, _, cx| {
                    cx.stop_propagation();
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|_, _ev, _, cx| {
                    cx.stop_propagation();
                }),
            )
            .children(
                self.ocr_lines
                    .iter()
                    .enumerate()
                    .map(|(i, line)| {
                        // 每框一个点击闭包，实体按框克隆（`Entity` 是廉价句柄）。
                        let view = cx.entity();
                        let is_selected = selected == Some(i);
                        div()
                            .absolute()
                            .id(format!("ocr-box-{i}"))
                            .left(px(line.rect.x * zoom))
                            .top(px(line.rect.y * zoom))
                            .w(px(line.rect.w * zoom))
                            .h(px(line.rect.h * zoom))
                            .rounded_sm()
                            .cursor_pointer()
                            .border_2()
                            .border_color(if is_selected {
                                rgb(0xFFFFFF)
                            } else {
                                rgb(0x66B2FF)
                            })
                            .bg(if is_selected {
                                Rgba {
                                    r: 0.3,
                                    g: 0.6,
                                    b: 1.0,
                                    a: 0.18,
                                }
                            } else {
                                Rgba {
                                    r: 0.0,
                                    g: 0.0,
                                    b: 0.0,
                                    a: 0.0,
                                }
                            })
                            // 角标：框内左上角，点框时一眼对应面板行号。
                            .child(
                                div()
                                    .absolute()
                                    .left(px(0.0))
                                    .top(px(0.0))
                                    .px_1()
                                    .rounded_sm()
                                    .bg(rgb(0x66B2FF))
                                    .text_color(rgb(0xFFFFFF))
                                    .text_size(px(11.))
                                    .child(format!("{}", i + 1)),
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|_, _ev, _, cx| {
                                    cx.stop_propagation();
                                }),
                            )
                            .on_click(move |_, window, cx| {
                                view.update(cx, |this, cx| {
                                    this.handle(ViewerAction::OcrSelectLine(i), window, cx)
                                });
                            })
                    })
                    .collect::<Vec<_>>(),
            )
    }

    /// OCR 右侧面板：默认列出全部识别文字，点行与图上框双向选中高亮。
    ///
    /// 行可滚（多行截图防撑爆）；面板吞掉鼠标与滚轮，避免误触图片区。
    fn ocr_side_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        let selected = self.selected_ocr;
        let title = tr_fmt(
            "viewer.ocr_panel_title",
            &[("n", &self.ocr_lines.len().to_string())],
        );
        div()
            .absolute()
            .right(px(12.0))
            .top(px(12.0))
            .w(px(300.0))
            .max_h(px(420.0))
            .flex()
            .flex_col()
            .rounded_md()
            .border_1()
            .border_color(rgb(0x555555))
            .bg(rgb(0x252525))
            .occlude()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _ev, _, cx| {
                    cx.stop_propagation();
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|_, _ev, _, cx| {
                    cx.stop_propagation();
                }),
            )
            .on_mouse_move(cx.listener(|_, _ev, _, cx| cx.stop_propagation()))
            .on_scroll_wheel(cx.listener(|_, _ev, _, cx| cx.stop_propagation()))
            .child(
                div()
                    .px_3()
                    .py_2()
                    .text_sm()
                    .text_color(rgb(0xFFFFFF))
                    .child(title),
            )
            .child(
                div().id("ocr-rows").flex_1().overflow_y_scroll().children(
                    self.ocr_lines
                        .iter()
                        .enumerate()
                        .map(|(i, line)| {
                            // 行也要可点：id 让它可交互，与图上框共用选中动作。
                            let view = view.clone();
                            let is_selected = selected == Some(i);
                            div()
                                .id(format!("ocr-row-{i}"))
                                .px_3()
                                .py_1()
                                .rounded_sm()
                                .cursor_pointer()
                                .bg(if is_selected {
                                    rgb(0x1E90FF)
                                } else {
                                    rgb(0x252525)
                                })
                                .text_xs()
                                .text_color(rgb(0xDDDDDD))
                                .hover(|style| {
                                    style.bg(if is_selected {
                                        rgb(0x1E90FF)
                                    } else {
                                        rgb(0x3A3A3A)
                                    })
                                })
                                .child(format!("{:>2}. {}", i + 1, line.text))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|_, _ev, _, cx| {
                                        cx.stop_propagation();
                                    }),
                                )
                                .on_click(move |_, window, cx| {
                                    view.update(cx, |this, cx| {
                                        this.handle(ViewerAction::OcrSelectLine(i), window, cx)
                                    });
                                })
                        })
                        .collect::<Vec<_>>(),
                ),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .child(self.mini_button(
                        cx,
                        "ocr-copy-all",
                        tr("viewer.ocr_copy_all"),
                        ViewerAction::OcrCopyAll,
                        false,
                    ))
                    .child(self.mini_button(
                        cx,
                        "ocr-apply",
                        tr("viewer.ocr_apply"),
                        ViewerAction::OcrApply,
                        true,
                    ))
                    .child(self.mini_button(
                        cx,
                        "ocr-close",
                        "×",
                        ViewerAction::CloseOcrPanel,
                        false,
                    )),
            )
    }

    /// 关闭确认浮层：有未保存标注时 Esc 弹出，居中模态三选一。
    ///
    /// 浮层盖住图片区并吞掉鼠标 / 滚轮事件，避免误触继续标注；
    /// 点击遮罩不关闭（防误触丢数据），只走保存并关闭 / 直接关闭 / 取消。
    fn close_confirm(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .absolute()
            .left(px(0.0))
            .top(px(0.0))
            .right(px(0.0))
            .bottom(px(0.0))
            .flex()
            .items_center()
            .justify_center()
            .bg(Rgba {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 0.55,
            })
            .occlude()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _ev, _, cx| {
                    cx.stop_propagation();
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|_, _ev, _, cx| {
                    cx.stop_propagation();
                }),
            )
            .on_mouse_move(cx.listener(|_, _ev, _, cx| cx.stop_propagation()))
            .on_scroll_wheel(cx.listener(|_, _ev, _, cx| cx.stop_propagation()))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .p_4()
                    .rounded_lg()
                    .border_1()
                    .border_color(rgb(0x555555))
                    .bg(rgb(0x2D2D2D))
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(0xFFFFFF))
                            .child(tr("confirm.title")),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(self.mini_button(
                                cx,
                                "confirm-save-close",
                                tr("confirm.save"),
                                ViewerAction::ConfirmSaveClose,
                                true,
                            ))
                            .child(self.mini_button(
                                cx,
                                "confirm-discard-close",
                                tr("confirm.discard"),
                                ViewerAction::ConfirmDiscardClose,
                                false,
                            ))
                            .child(self.mini_button(
                                cx,
                                "confirm-cancel-close",
                                tr("confirm.cancel"),
                                ViewerAction::CancelCloseConfirm,
                                false,
                            )),
                    ),
            )
    }

    /// 底部状态栏（复制 / 保存反馈 + 图片信息 + 已保存快捷入口）。
    fn status_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        // 右侧图片信息：尺寸 / 缩放 / 标注数，一目了然；有未保存修改时追加提醒。
        let mut info = tr_fmt(
            "viewer.info",
            &[
                ("w", &self.preview.w.to_string()),
                ("h", &self.preview.h.to_string()),
                ("zoom", &((self.zoom * 100.0).round() as u32).to_string()),
                ("n", &self.shapes.len().to_string()),
            ],
        );
        if self.dirty {
            info.push_str(&tr("viewer.unsaved"));
        }
        let mut row = div()
            .flex()
            .items_center()
            .gap_2()
            .h(px(STATUS_H))
            .px_3()
            .bg(rgb(0x252525))
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .text_xs()
                    .text_color(rgb(0xBBBBBB))
                    .child(self.status.to_string()),
            );
        // 保存成功后才出现快捷入口：打开文件 / 打开目录。
        if self.last_saved.is_some() {
            row = row
                .child(self.mini_button(
                    cx,
                    "open-saved-file",
                    tr("viewer.open_file"),
                    ViewerAction::OpenSavedFile,
                    false,
                ))
                .child(self.mini_button(
                    cx,
                    "open-saved-dir",
                    tr("viewer.open_dir"),
                    ViewerAction::OpenSavedDir,
                    false,
                ));
        }
        row.child(
            div()
                .flex_shrink_0()
                .text_xs()
                .text_color(rgb(0x888888))
                .child(info),
        )
    }
}

#[cfg(test)]
mod tests {
    //! 预览模块单测：橡皮擦命中判定（纯函数，不依赖窗口）。

    use super::*;
    use crate::shapes::PixelRect;

    /// 橡皮擦：矩形按外包矩形命中，画笔按笔迹距离命中，未命中返回 None。
    #[test]
    fn erase_hit_cases() {
        let shapes = vec![
            Shape::Rect {
                rect: PixelRect::new(10.0, 10.0, 50.0, 40.0),
                color: AnnoColor::RED,
                width: 3.0,
            },
            Shape::Pen {
                pts: vec![(100.0, 100.0), (200.0, 100.0)],
                color: AnnoColor::BLUE,
                width: 4.0,
            },
        ];
        // 矩形内命中第一项。
        assert_eq!(erase_hit(&shapes, (20.0, 20.0), 8.0), Some(0));
        // 笔迹正上方 5px：半径 8 + 半宽 2 = 10 ≥ 5，命中第二项。
        assert_eq!(erase_hit(&shapes, (150.0, 105.0), 8.0), Some(1));
        // 笔迹上方 20px：超出范围，两项都不命中。
        assert_eq!(erase_hit(&shapes, (150.0, 120.0), 8.0), None);
        // 空白处不命中。
        assert_eq!(erase_hit(&shapes, (300.0, 300.0), 8.0), None);
    }

    /// 橡皮擦：重叠时删最顶层（后画的）。
    #[test]
    fn erase_hit_topmost() {
        let shapes = vec![
            Shape::Rect {
                rect: PixelRect::new(0.0, 0.0, 100.0, 100.0),
                color: AnnoColor::RED,
                width: 3.0,
            },
            Shape::Rect {
                rect: PixelRect::new(50.0, 50.0, 100.0, 100.0),
                color: AnnoColor::BLUE,
                width: 3.0,
            },
        ];
        assert_eq!(erase_hit(&shapes, (75.0, 75.0), 8.0), Some(1));
    }

    /// 固定窗口布局：小截图不放大（居中 100% 显示），窗口保持最小尺寸。
    #[test]
    fn viewer_layout_small_image_keeps_full_size() {
        let (area_w, area_h, fit) = viewer_layout(1920.0, 1080.0, 300, 200);
        // 小图不放大，窗口不随图片缩小。
        assert!((fit - 1.0).abs() < f32::EPSILON);
        assert!(area_w >= MIN_VIEWER_W);
        assert!(area_h >= 200.0);
    }

    /// 固定窗口布局：大截图按比例缩小，完整落入图片区。
    #[test]
    fn viewer_layout_large_image_shrinks_to_fit() {
        let (area_w, area_h, fit) = viewer_layout(1920.0, 1080.0, 3840, 2160);
        assert!(fit < 1.0);
        assert!(3840.0 * fit <= area_w + 1.0);
        assert!(2160.0 * fit <= area_h + 1.0);
    }

    /// 固定窗口布局：极小显示器也不 panic，缩放有下限。
    #[test]
    fn viewer_layout_tiny_display_no_panic() {
        let (area_w, area_h, fit) = viewer_layout(800.0, 600.0, 1920, 1080);
        assert!(area_w > 0.0 && area_h > 0.0);
        assert!(fit >= MIN_ZOOM);
    }

    /// 线宽规范化：钳制到滑杆范围并取整。
    #[test]
    fn normalize_width_cases() {
        assert_eq!(normalize_width(0.2), WIDTH_MIN);
        assert_eq!(normalize_width(4.6), 5.0);
        assert_eq!(normalize_width(99.0), WIDTH_MAX);
    }

    /// 快捷键映射：V 选择、1-8 按标注排顺序，其余返回 None。
    #[test]
    fn tool_shortcut_cases() {
        assert_eq!(AnnoTool::from_shortcut("v"), Some(AnnoTool::Select));
        assert_eq!(AnnoTool::from_shortcut("1"), Some(AnnoTool::Rect));
        assert_eq!(AnnoTool::from_shortcut("8"), Some(AnnoTool::Text));
        assert_eq!(AnnoTool::from_shortcut("9"), None);
        assert_eq!(AnnoTool::from_shortcut("z"), None);
    }

    /// 最近文字索引：取最后一条文字，无文字返回 None（底色开关无文字时只切默认）。
    #[test]
    fn latest_text_index_cases() {
        let shapes = vec![
            Shape::Rect {
                rect: PixelRect::new(0.0, 0.0, 10.0, 10.0),
                color: AnnoColor::RED,
                width: 2.0,
            },
            Shape::Text {
                pos: (0.0, 0.0),
                content: "旧".to_string(),
                color: AnnoColor::WHITE,
                size: 20.0,
                bg: true,
                bg_color: AnnoColor::BLACK,
            },
            Shape::Text {
                pos: (5.0, 5.0),
                content: "新".to_string(),
                color: AnnoColor::BLACK,
                size: 20.0,
                bg: false,
                bg_color: AnnoColor::WHITE,
            },
        ];
        // 取最后一条文字，而不是第一条。
        assert_eq!(latest_text_index(&shapes), Some(2));
        assert_eq!(latest_text_index(&shapes[..1]), None);
        assert_eq!(latest_text_index(&[]), None);
    }
}
