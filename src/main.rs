//! 应用入口模块：初始化 rgpui，挂载主窗口、托盘与全局截图热键。

mod app;
mod capture;
// 标注阶段（形状绘制 / 导出烧录）暂未接入，先保留模块，恢复使用时去掉 allow。
#[allow(dead_code)]
mod export;
mod hotkey;
mod overlay;
// 标注阶段暂未接入，先保留模块，恢复使用时去掉 allow。
#[allow(dead_code)]
mod shapes;
mod tray;
mod viewer;

use std::{cell::RefCell, rc::Rc};

use rgpui::{App, Bounds, Entity, WindowHandle, prelude::*, px, size};
use rgpui_platform::application;

use crate::{app::RupixApp, hotkey::HotkeyKind};

// 检查器全局动作：F12 开关右侧 inspector 面板。
// 仅调试构建（或显式 --features inspector）编译，release 默认剥离。
#[cfg(any(feature = "inspector", debug_assertions))]
use rgpui::actions;
#[cfg(any(feature = "inspector", debug_assertions))]
actions!(inspector, [ToggleInspector]);

/// 程序入口：组装主窗口、托盘、全局热键后进入事件循环。
fn main() {
    application().run(|cx: &mut App| {
        // 初始化主题与内置组件（HotkeyInput / Input 等依赖它）。
        rgpui::init_all(cx);

        // 启用默认 inspector 面板，F12 开关。
        // 与 rgpui 库侧保持同条件编译：release 默认无检查器代码、零成本。
        #[cfg(any(feature = "inspector", debug_assertions))]
        {
            cx.enable_default_inspector();
            cx.on_global_action(ToggleInspector, Some("f12"), |window, cx| {
                window.toggle_inspector(cx);
            });
        }

        // 读取用户保存的截图热键（区域 / 全屏各一个），首次运行使用默认值。
        let region_hotkey = hotkey::load_hotkey_string(HotkeyKind::Region);
        let full_hotkey = hotkey::load_hotkey_string(HotkeyKind::Full);

        // 主窗口实体需要在构建闭包外使用，用容器带出来。
        let main_cell: Rc<RefCell<Option<Entity<RupixApp>>>> = Rc::new(RefCell::new(None));
        let main_cell_for_build = main_cell.clone();
        let region_for_build = region_hotkey.clone();
        let full_for_build = full_hotkey.clone();
        // 两张热键卡片比原来高，默认窗口加高。
        let bounds = Bounds::centered(None, size(px(560.), px(800.)), cx);
        let main_win: WindowHandle<RupixApp> = cx
            .open_window(
                rgpui::WindowOptions {
                    window_bounds: Some(rgpui::WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                move |window, cx| {
                    let view =
                        cx.new(|cx| RupixApp::new(window, cx, region_for_build, full_for_build));
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

        // 注册两个系统级全局热键（失焦可用），触发走托盘同款截图入口。
        // 两个都成功才报组合状态，任一失败直接展示该路错误（另一路不受影响）。
        let region_result = hotkey::apply_global_hotkey(cx, HotkeyKind::Region, &region_hotkey);
        let full_result = hotkey::apply_global_hotkey(cx, HotkeyKind::Full, &full_hotkey);
        let boot_msg = match (&region_result, &full_result) {
            (Ok(()), Ok(())) => {
                format!("全局截图：区域 {region_hotkey}，全屏 {full_hotkey}")
            }
            (Err(error), _) | (_, Err(error)) => error.clone(),
        };
        main.update(cx, |view, cx| view.set_status(&boot_msg, cx));
        cx.on_global_hotkey(move |id, cx| {
            // 按编号分发截图种类，其他全局热键（若有）忽略。
            if id == HotkeyKind::Region.id() {
                tray::trigger_capture(cx, main_win, main.clone(), false);
            } else if id == HotkeyKind::Full.id() {
                tray::trigger_capture(cx, main_win, main.clone(), true);
            }
        });

        cx.activate(true);
    });
}
