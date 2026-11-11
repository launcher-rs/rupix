//! 窗口枚举模块：列出顶层窗口并按坐标命中，供窗口截图高亮与裁剪。
//!
//! 坐标系为物理像素，与屏幕捕获首帧一致，可直接换算到浮层视图坐标。
//! 非 Windows 平台返回空列表，调用方降级提示“暂不支持”。
//! 另外提供 [`exclude_from_capture`]：把自己的提示窗口排除出系统抓屏，
//! 免得连按两次快捷键时把上一次的保存提示也截进图里。

/// 可截图的顶层窗口。
#[derive(Clone, Debug)]
pub struct CaptureWindow {
    /// 窗口标题（高亮徽标展示用）。
    pub title: String,
    /// 窗口矩形（物理像素：左、上、右、下，可见边框，不含不可见阴影）。
    pub rect: (i32, i32, i32, i32),
}

impl CaptureWindow {
    /// 点是否落在窗口内（含边界）。
    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.rect.0 && x <= self.rect.2 && y >= self.rect.1 && y <= self.rect.3
    }
}

/// 列出当前可截图的顶层窗口（按 Z 序从上到下）。
pub fn list_windows() -> Vec<CaptureWindow> {
    imp::list_windows()
}

/// 命中指定物理像素点的最顶层窗口（从上往下第一个包含该点的）。
pub fn hit_test(x: i32, y: i32) -> Option<CaptureWindow> {
    list_windows().into_iter().find(|w| w.contains(x, y))
}

/// 把自身窗口排除出系统抓屏（提示胶囊用，避免出现在下一张截图里）。
///
/// 失败只影响「提示会不会被截进去」，不影响截图本身，因此静默忽略。
/// Windows 10 2004 之前没有 `WDA_EXCLUDEFROMCAPTURE`，系统会直接拒绝，效果等同未设置。
///
/// # 参数
/// - `window`: 要排除的顶层窗口（rgpui 的 `Window`）。
#[cfg(target_os = "windows")]
pub fn exclude_from_capture(window: &rgpui::Window) {
    use raw_window_handle::RawWindowHandle;
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        SetWindowDisplayAffinity, WDA_EXCLUDEFROMCAPTURE,
    };
    // rgpui 的 `Window` 自带的方法是取 `AnyWindowHandle`（类型擦除句柄），
    // 拿原生 HWND 只能走 raw-window-handle 的 `HasWindowHandle` trait。
    let Ok(handle) = raw_window_handle::HasWindowHandle::window_handle(window) else {
        return;
    };
    if let RawWindowHandle::Win32(win32) = handle.as_raw() {
        // raw-window-handle 的 hwnd 是 NonZeroIsize，直接按指针重建 HWND。
        let hwnd = HWND(win32.hwnd.get() as *mut core::ffi::c_void);
        unsafe {
            let _ = SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE);
        }
    }
}

/// 非 Windows 平台暂无对应能力，留空实现（提示照常显示，只是可能被截进去）。
#[cfg(not(target_os = "windows"))]
pub fn exclude_from_capture(_window: &rgpui::Window) {}

/// Windows 实现：EnumWindows 按 Z 序枚举，过滤不可见 / 最小化 / 幽灵窗口与自身进程。
#[cfg(target_os = "windows")]
mod imp {
    use windows::Win32::{
        Foundation::{HWND, LPARAM, RECT, TRUE},
        Graphics::Dwm::{DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute},
        System::Threading::GetCurrentProcessId,
        UI::WindowsAndMessaging::{
            EnumWindows, GetWindowRect, GetWindowTextW, GetWindowThreadProcessId, IsIconic,
            IsWindowVisible,
        },
    };
    use windows::core::BOOL;

    use super::CaptureWindow;

    /// EnumWindows 回调：收集全部顶层窗口句柄（枚举顺序即 Z 序从上到下）。
    unsafe extern "system" fn enum_callback(hwnd: HWND, lparam: LPARAM) -> BOOL {
        // 回调跑在调用线程，指针回填调用方栈上 Vec，安全。
        unsafe {
            let list = &mut *(lparam.0 as *mut Vec<HWND>);
            list.push(hwnd);
        }
        TRUE
    }

    /// 窗口是否被 DWM 遮蔽（UWP 挂起、虚拟桌面他页等拿不到有效画面，跳过）。
    fn is_cloaked(hwnd: HWND) -> bool {
        let mut cloaked = 0u32;
        unsafe {
            DwmGetWindowAttribute(
                hwnd,
                DWMWA_CLOAKED,
                &mut cloaked as *mut u32 as *mut std::ffi::c_void,
                std::mem::size_of::<u32>() as u32,
            )
        }
        .is_ok_and(|()| cloaked != 0)
    }

    /// 窗口标题（空标题的工具窗口 / 悬浮层直接跳过）。
    fn window_title(hwnd: HWND) -> String {
        let mut buf = [0u16; 256];
        let len = unsafe { GetWindowTextW(hwnd, &mut buf) };
        String::from_utf16_lossy(&buf[..len as usize])
            .trim()
            .to_string()
    }

    /// 窗口可见矩形：优先 DWM 扩展帧边界（去掉不可见缩放边框与阴影，
    /// `GetWindowRect` 会多出一圈透明边框，截出来比窗口大一圈）。
    /// DWM 取不到时（如无边框特殊窗口）回退到 `GetWindowRect`。
    fn visible_rect(hwnd: HWND) -> Option<RECT> {
        let mut frame = RECT::default();
        let dwm_ok = unsafe {
            DwmGetWindowAttribute(
                hwnd,
                DWMWA_EXTENDED_FRAME_BOUNDS,
                &mut frame as *mut RECT as *mut std::ffi::c_void,
                std::mem::size_of::<RECT>() as u32,
            )
        }
        .is_ok();
        if dwm_ok {
            return Some(frame);
        }
        let mut rect = RECT::default();
        unsafe { GetWindowRect(hwnd, &mut rect) }.ok()?;
        Some(rect)
    }

    pub fn list_windows() -> Vec<CaptureWindow> {
        // 先拿全量句柄，再逐个过滤（过滤条件多，EnumWindows 回调里只收集）。
        let mut hwnds: Vec<HWND> = Vec::new();
        unsafe {
            let _ = EnumWindows(
                Some(enum_callback),
                LPARAM(&mut hwnds as *mut Vec<HWND> as isize),
            );
        }
        let own_pid = unsafe { GetCurrentProcessId() };
        let mut out = Vec::new();
        for hwnd in hwnds {
            // 不可见与最小化窗口没有有效画面。
            if !unsafe { IsWindowVisible(hwnd) }.as_bool() {
                continue;
            }
            if unsafe { IsIconic(hwnd) }.as_bool() {
                continue;
            }
            if is_cloaked(hwnd) {
                continue;
            }
            // 排除自身进程：截图浮层 / 预览 / 设置窗口不参与拾取。
            let mut pid = 0u32;
            unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
            if pid == own_pid {
                continue;
            }
            // 可见矩形走 DWM 扩展帧边界，不含不可见阴影边框。
            let Some(rect) = visible_rect(hwnd) else {
                continue;
            };
            let (w, h) = (rect.right - rect.left, rect.bottom - rect.top);
            // 过小矩形多为阴影残留或后台代理窗口。
            if w < 60 || h < 60 {
                continue;
            }
            let title = window_title(hwnd);
            if title.is_empty() {
                continue;
            }
            out.push(CaptureWindow {
                title,
                rect: (rect.left, rect.top, rect.right, rect.bottom),
            });
        }
        out
    }
}

/// 非 Windows 平台：暂不支持窗口枚举，返回空列表。
#[cfg(not(target_os = "windows"))]
mod imp {
    use super::CaptureWindow;

    pub fn list_windows() -> Vec<CaptureWindow> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    //! 窗口模块单测：矩形几何纯逻辑（枚举依赖系统，不进单测）。

    use super::*;

    /// 包含判定与宽高钳制断言。
    #[test]
    fn rect_geometry() {
        let w = CaptureWindow {
            title: "t".to_string(),
            rect: (10, 20, 110, 120),
        };
        assert_eq!((w.rect.2 - w.rect.0, w.rect.3 - w.rect.1), (100, 100));
        assert!(w.contains(10, 20));
        assert!(w.contains(110, 120));
        assert!(!w.contains(9, 20));
        let empty = CaptureWindow {
            title: "e".to_string(),
            rect: (5, 5, 5, 5),
        };
        assert_eq!(
            (empty.rect.2 - empty.rect.0, empty.rect.3 - empty.rect.1),
            (0, 0)
        );
    }
}
