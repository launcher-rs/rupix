//! 应用入口模块：初始化 rgpui，挂载主窗口、托盘与全局截图热键。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod autostart;
mod capture;
mod cli;
// 公开函数均被 viewer 使用，allow 仅为冗余防护。
#[allow(dead_code)]
mod export;
mod hotkey;
mod i18n;
mod ocr;
mod overlay;
mod picker;
mod window;
// 标注阶段暂未接入，先保留模块，恢复使用时去掉 allow。
#[allow(dead_code)]
mod shapes;
mod toast;
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

/// 程序入口：带截图参数走命令行（截图后退出），无参数进图形界面。
fn main() {
    // CLI 早分发：不进 rgpui 事件循环，也不受单例守卫限制（允许与托盘进程并存）。
    if cli::is_cli_invocation() {
        std::process::exit(cli::run_cli());
    }
    // 单例守卫：已有实例在跑（托盘常驻）则直接退出，避免热键冲突与重复托盘图标。
    // 守卫必须存活整个进程期，故意泄漏持有到进程结束。
    let Ok(_guard) = rgpui::SingleInstance::acquire("rupix") else {
        return;
    };
    std::mem::forget(_guard);
    application().run(|cx: &mut App| {
        // 初始化主题与内置组件（HotkeyInput / Input 等依赖它）。
        rgpui::init_all(cx);
        // 语言初始化必须在建窗前：窗口标题与状态文案都走词表。
        i18n::init();

        // 启用默认 inspector 面板，F12 开关。
        // 与 rgpui 库侧保持同条件编译：release 默认无检查器代码、零成本。
        #[cfg(any(feature = "inspector", debug_assertions))]
        {
            cx.enable_default_inspector();
            cx.on_global_action(ToggleInspector, Some("f12"), |window, cx| {
                window.toggle_inspector(cx);
            });
        }

        // 读取用户保存的热键（区域 / 全屏 / 取色各一个），首次运行使用默认值。
        let region_hotkey = hotkey::load_hotkey_string(HotkeyKind::Region);
        let full_hotkey = hotkey::load_hotkey_string(HotkeyKind::Full);
        let picker_hotkey = hotkey::load_hotkey_string(HotkeyKind::Picker);

        // 主窗口实体需要在构建闭包外使用，用容器带出来。
        let main_cell: Rc<RefCell<Option<Entity<RupixApp>>>> = Rc::new(RefCell::new(None));
        let main_cell_for_build = main_cell.clone();
        let region_for_build = region_hotkey.clone();
        let full_for_build = full_hotkey.clone();
        let picker_for_build = picker_hotkey.clone();
        // 三张热键卡片，默认窗口加高。
        let bounds = Bounds::centered(None, size(px(560.), px(400.)), cx);
        let main_win: WindowHandle<RupixApp> = cx
            .open_window(
                rgpui::WindowOptions {
                    window_bounds: Some(rgpui::WindowBounds::Windowed(bounds)),
                    titlebar: Some(rgpui::TitlebarOptions {
                        title: Some(i18n::tr("app.window_title").into()),
                        ..Default::default()
                    }),
                    // 设置窗口平时不需要显示：启动即隐藏到托盘，
                    // 用户经托盘菜单 / 图标点击再打开，快捷键截图不受影响。
                    show: false,
                    focus: false,
                    ..Default::default()
                },
                move |window, cx| {
                    let view = cx.new(|cx| {
                        RupixApp::new(
                            window,
                            cx,
                            region_for_build,
                            full_for_build,
                            picker_for_build,
                        )
                    });
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

        // 注册三个系统级全局热键（失焦可用），触发走托盘同款入口。
        // 全部成功才报组合状态，任一失败展示全部错误（成功的不受影响）。
        let region_result = hotkey::apply_global_hotkey(cx, HotkeyKind::Region, &region_hotkey);
        let full_result = hotkey::apply_global_hotkey(cx, HotkeyKind::Full, &full_hotkey);
        let picker_result = hotkey::apply_global_hotkey(cx, HotkeyKind::Picker, &picker_hotkey);
        let mut boot_errors = Vec::new();
        for result in [&region_result, &full_result, &picker_result] {
            if let Err(error) = result {
                boot_errors.push(error.clone());
            }
        }
        let boot_msg = if boot_errors.is_empty() {
            i18n::tr_fmt(
                "app.boot_ready",
                &[
                    ("region", &region_hotkey),
                    ("full", &full_hotkey),
                    ("picker", &picker_hotkey),
                ],
            )
        } else {
            boot_errors.join("; ")
        };
        main.update(cx, |view, cx| view.set_status(&boot_msg, cx));
        cx.on_global_hotkey(move |id, cx| {
            // 按编号分发截图种类，其他全局热键（若有）忽略。
            if id == HotkeyKind::Region.id() {
                tray::trigger_capture(cx, main_win, main.clone(), false);
            } else if id == HotkeyKind::Full.id() {
                tray::trigger_capture(cx, main_win, main.clone(), true);
            } else if id == HotkeyKind::Picker.id() {
                tray::trigger_picker(cx, main_win, main.clone());
            }
        });

        cx.activate(true);
    });
}
