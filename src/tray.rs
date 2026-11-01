//! 系统托盘模块：常驻后台、全局截图入口与窗口显隐。
//!
//! Rupix 设置窗口平时隐藏，关闭主窗口时不退出，而是隐藏到托盘（参考 rgpui `tray` 示例）：
//! 截图、图片编辑、设置、退出全部走托盘菜单或全局热键。

use rgpui::{App, AppContext, Entity, TrayIconEvent, TrayMenuItem, WindowHandle};

use crate::{app::RupixApp, i18n::tr, viewer::open_image_for_edit_in_app};

/// 托盘菜单项编号（与点击回调中的分支对应）。
const MENU_REGION: &str = "capture_region";
/// 全屏截图菜单项编号。
const MENU_FULL: &str = "capture_full";
/// 屏幕取色菜单项编号。
const MENU_PICKER: &str = "pick_color";
/// 打开图片编辑菜单项编号。
const MENU_OPEN_IMAGE: &str = "open_image";
/// 编辑最近一张截图菜单项编号。
const MENU_EDIT_LAST: &str = "edit_last";
/// 显示设置窗口菜单项编号。
const MENU_SHOW: &str = "show_window";
/// 退出菜单项编号。
const MENU_QUIT: &str = "quit";

/// 初始化托盘：常驻 + 菜单 + 点击行为。
///
/// # 参数
/// - `cx`: 应用上下文。
/// - `main_win`: 主窗口句柄（显隐与截图触发用）。
/// - `main`: 主视图实体（截图状态机所在）。
pub fn setup_tray(cx: &mut App, main_win: WindowHandle<RupixApp>, main: Entity<RupixApp>) {
    // 没有窗口也要保持运行，否则隐藏主窗口即退出进程。
    cx.set_keep_alive_without_windows(true);
    cx.set_tray_tooltip(&tr("tray.tooltip"));
    // 托盘图标：品牌蓝取景框 + 山形（`assets/tray.png`，PNG 字节由后端解码）。
    cx.set_tray_icon(Some(include_bytes!("../assets/tray.png")));
    cx.set_tray_menu(tray_menu());

    // 左键单击托盘图标即显示设置窗口（右键弹出菜单由系统负责）。
    let show_win = main_win;
    cx.on_tray_icon_event(move |event, cx| {
        if matches!(event, TrayIconEvent::LeftClick) {
            show_main_window(cx, show_win);
        }
    });

    // 托盘菜单动作分发。
    cx.on_tray_menu_action(move |id, cx| match id.as_ref() {
        MENU_REGION => trigger_capture(cx, main_win, main.clone(), false),
        MENU_FULL => trigger_capture(cx, main_win, main.clone(), true),
        MENU_PICKER => trigger_picker(cx, main_win, main.clone()),
        MENU_OPEN_IMAGE => open_image_for_edit_in_app(main.clone(), cx),
        // 重新打开最近一张截图：不用再走文件对话框挑图。
        MENU_EDIT_LAST => main.update(cx, |view, cx| view.open_last_for_edit(cx)),
        MENU_SHOW => show_main_window(cx, main_win),
        MENU_QUIT => cx.quit(),
        _ => {}
    });
}

/// 托盘菜单项（按当前语言生成；初始化与语言切换后重建共用）。
fn tray_menu() -> Vec<TrayMenuItem> {
    vec![
        TrayMenuItem::Action {
            label: tr("tray.region").into(),
            id: MENU_REGION.into(),
        },
        TrayMenuItem::Action {
            label: tr("tray.full").into(),
            id: MENU_FULL.into(),
        },
        TrayMenuItem::Action {
            label: tr("tray.picker").into(),
            id: MENU_PICKER.into(),
        },
        TrayMenuItem::Separator,
        TrayMenuItem::Action {
            label: tr("tray.open_image").into(),
            id: MENU_OPEN_IMAGE.into(),
        },
        // 关掉「截图后打开编辑窗」后，托盘这里是找回上一张直接标注的快捷入口。
        TrayMenuItem::Action {
            label: tr("tray.edit_last").into(),
            id: MENU_EDIT_LAST.into(),
        },
        TrayMenuItem::Action {
            label: tr("tray.show").into(),
            id: MENU_SHOW.into(),
        },
        TrayMenuItem::Action {
            label: tr("tray.quit").into(),
            id: MENU_QUIT.into(),
        },
    ]
}

/// 按当前语言刷新托盘提示与菜单（设置页切换语言后调用，避免托盘文案停留在旧语言）。
///
/// # 参数
/// - `cx`: 应用上下文。
pub fn refresh_labels(cx: &mut App) {
    cx.set_tray_tooltip(&tr("tray.tooltip"));
    cx.set_tray_menu(tray_menu());
}

/// 从 App 上下文触发截图（托盘菜单 / 全局热键共用）。
///
/// 主窗口可能处于隐藏态：先借窗口发起捕获流程（`spawn_in` 需要窗口），
/// 捕获浮层是独立新窗口，不依赖主窗口可见。
pub fn trigger_capture(
    cx: &mut App,
    main_win: WindowHandle<RupixApp>,
    main: Entity<RupixApp>,
    fullscreen: bool,
) {
    // update_window 借出主窗口，再 update 主实体拿到 Context 发起异步捕获。
    let _ = cx.update_window(main_win.into(), |_, window, cx| {
        cx.update_entity(&main, |view, viewcx| {
            if fullscreen {
                view.begin_full(window, viewcx);
            } else {
                view.begin_region(window, viewcx);
            }
        });
    });
}

/// 从 App 上下文触发取色（托盘菜单 / 全局热键共用）。
///
/// 与截图同理：借主窗口发起，后续浮层是独立新窗口。
pub fn trigger_picker(cx: &mut App, main_win: WindowHandle<RupixApp>, main: Entity<RupixApp>) {
    // update_window 借出主窗口，再 update 主实体拿到 Context 发起异步取色。
    let _ = cx.update_window(main_win.into(), |_, window, cx| {
        cx.update_entity(&main, |view, viewcx| {
            view.begin_picker(window, viewcx);
        });
    });
}

/// 显示并激活设置窗口（托盘菜单 / 图标点击共用）。
fn show_main_window(cx: &mut App, main_win: WindowHandle<RupixApp>) {
    // 设置窗口只是被隐藏而非销毁，直接激活即可恢复显示。
    let _ = cx.update_window(main_win.into(), |_, window, _| {
        window.activate_window();
    });
}
