//! 系统托盘模块：常驻后台、全局截图入口与窗口显隐。
//!
//! Rupix 设置窗口平时隐藏，关闭主窗口时不退出，而是隐藏到托盘（参考 rgpui `tray` 示例）：
//! 截图、图片编辑、设置、退出全部走托盘菜单或全局热键。

use rgpui::{App, AppContext, Entity, TrayIconEvent, TrayMenuItem, WindowHandle};

use crate::{app::RupixApp, viewer::open_image_for_edit_in_app};

/// 托盘菜单项编号（与点击回调中的分支对应）。
const MENU_REGION: &str = "capture_region";
/// 全屏截图菜单项编号。
const MENU_FULL: &str = "capture_full";
/// 打开图片编辑菜单项编号。
const MENU_OPEN_IMAGE: &str = "open_image";
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
    cx.set_tray_tooltip("Rupix 拾景");
    // 托盘图标：品牌蓝取景框 + 山形（`assets/tray.png`，PNG 字节由后端解码）。
    cx.set_tray_icon(Some(include_bytes!("../assets/tray.png")));

    cx.set_tray_menu(vec![
        TrayMenuItem::Action {
            label: "区域截图".into(),
            id: MENU_REGION.into(),
        },
        TrayMenuItem::Action {
            label: "全屏截图".into(),
            id: MENU_FULL.into(),
        },
        TrayMenuItem::Separator,
        TrayMenuItem::Action {
            label: "打开图片编辑…".into(),
            id: MENU_OPEN_IMAGE.into(),
        },
        TrayMenuItem::Action {
            label: "打开设置".into(),
            id: MENU_SHOW.into(),
        },
        TrayMenuItem::Action {
            label: "退出".into(),
            id: MENU_QUIT.into(),
        },
    ]);

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
        MENU_OPEN_IMAGE => open_image_for_edit_in_app(main.clone(), cx),
        MENU_SHOW => show_main_window(cx, main_win),
        MENU_QUIT => cx.quit(),
        _ => {}
    });
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

/// 显示并激活设置窗口（托盘菜单 / 图标点击共用）。
fn show_main_window(cx: &mut App, main_win: WindowHandle<RupixApp>) {
    // 设置窗口只是被隐藏而非销毁，直接激活即可恢复显示。
    let _ = cx.update_window(main_win.into(), |_, window, _| {
        window.activate_window();
    });
}
