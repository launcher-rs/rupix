//! 窗口枚举模块：列出顶层窗口并按坐标命中，供窗口截图高亮与裁剪。
//!
//! 坐标系为物理像素，与屏幕捕获首帧一致，可直接换算到浮层视图坐标。
//! 非 Windows 平台返回空列表，调用方降级提示“暂不支持”。

/// 可截图的顶层窗口。
#[derive(Clone, Debug)]
pub struct CaptureWindow {
    /// 窗口标题（高亮徽标展示用）。
    pub title: String,
    /// 窗口矩形（物理像素：左、上、右、下，含边框）。
    pub rect: (i32, i32, i32, i32),
}

impl CaptureWindow {
    /// 矩形宽（物理像素）。
    pub fn width(&self) -> i32 {
        (self.rect.2 - self.rect.0).max(0)
    }

    /// 矩形高（物理像素）。
    pub fn height(&self) -> i32 {
        (self.rect.3 - self.rect.1).max(0)
    }

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

/// Windows 实现：EnumWindows 按 Z 序枚举，过滤不可见 / 最小化 / 幽灵窗口与自身进程。
#[cfg(target_os = "windows")]
mod imp {
    use windows::Win32::{
        Foundation::{HWND, LPARAM, RECT, TRUE},
        Graphics::Dwm::{DWMWA_CLOAKED, DwmGetWindowAttribute},
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
            let mut rect = RECT::default();
            if unsafe { GetWindowRect(hwnd, &mut rect) }.is_err() {
                continue;
            }
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
        assert_eq!((w.width(), w.height()), (100, 100));
        assert!(w.contains(10, 20));
        assert!(w.contains(110, 120));
        assert!(!w.contains(9, 20));
        let empty = CaptureWindow {
            title: "e".to_string(),
            rect: (5, 5, 5, 5),
        };
        assert_eq!((empty.width(), empty.height()), (0, 0));
    }
}
