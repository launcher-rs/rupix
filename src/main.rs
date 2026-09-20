//! 应用入口模块：初始化 rgpui，挂载主窗口、托盘与全局截图热键。

mod app;
mod capture;
mod export;
mod hotkey;
mod overlay;
mod shapes;
mod tray;

use std::{cell::RefCell, rc::Rc};

use rgpui::{App, Bounds, Entity, WindowHandle, prelude::*, px, size};
use rgpui_platform::application;

use crate::app::RupixApp;

/// 程序入口：组装主窗口、托盘、全局热键后进入事件循环。
fn main() {
    application().run(|cx: &mut App| {
        // 初始化主题与内置组件（HotkeyInput / Input 等依赖它）。
        rgpui::init_all(cx);

        // 读取用户保存的截图热键，首次运行使用默认值。
        let saved_hotkey = hotkey::load_hotkey_string();

        // 主窗口实体需要在构建闭包外使用，用容器带出来。
        let main_cell: Rc<RefCell<Option<Entity<RupixApp>>>> = Rc::new(RefCell::new(None));
        let main_cell_for_build = main_cell.clone();
        let hotkey_for_build = saved_hotkey.clone();
        let bounds = Bounds::centered(None, size(px(560.), px(660.)), cx);
        let main_win: WindowHandle<RupixApp> = cx
            .open_window(
                rgpui::WindowOptions {
                    window_bounds: Some(rgpui::WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                move |window, cx| {
                    let view = cx.new(|cx| RupixApp::new(window, cx, hotkey_for_build));
                    *main_cell_for_build.borrow_mut() = Some(view.clone());
                    // 关闭按钮只隐藏到托盘，不退出进程（退出走托盘菜单）。
                    window.on_window_should_close(cx, |window, _| {
                        window.hide_window();
                        false
                    });
                    view
                },
            )
            .expect("打开 Rupix 主窗口失败");
        let main = main_cell
            .borrow()
            .clone()
            .expect("主视图实体应在建窗时初始化");

        // 托盘常驻：截图入口与退出都走这里。
        tray::setup_tray(cx, main_win, main.clone());

        // 注册系统级全局热键（失焦可用），触发走托盘同款截图入口。
        match hotkey::apply_global_hotkey(cx, &saved_hotkey) {
            Ok(()) => {
                let msg = format!("全局截图快捷键：{saved_hotkey}");
                main.update(cx, |view, cx| view.set_status(&msg, cx));
            }
            Err(error) => {
                main.update(cx, |view, cx| view.set_status(&error, cx));
            }
        }
        cx.on_global_hotkey(move |id, cx| {
            // 只响应截图热键编号，其他全局热键（若有）忽略。
            if id == hotkey::HOTKEY_ID {
                tray::trigger_capture(cx, main_win, main.clone(), false);
            }
        });

        cx.activate(true);
    });
}
