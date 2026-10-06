//! 命令行截图模块：无界面全屏 / 区域截图，直存 PNG 后退出。
//!
//! 不带参数启动图形界面（托盘常驻）；带截图参数时不进 rgpui 事件循环，
//! 直接经 Windows GDI 同步截取虚拟屏幕全屏（`--region` 再裁剪），保存后打印路径退出。
//! 参数解析只用标准库（不引入 clap 等新依赖），像素操作复用 `image` 依赖。

use std::path::{Path, PathBuf};

use image::RgbaImage;

/// 命令行帮助文案（`--help` 与参数错误时展示）。
const HELP: &str = "Rupix 拾景 · 命令行截图\n\
    \n\
    用法：\n\
    \x20 rupix                              启动图形界面（托盘常驻）\n\
    \x20 rupix -f [--output 路径] [--copy]             截取全屏\n\
    \x20 rupix -r x,y,w,h [--output 路径] [--copy]     截取指定区域\n\
    \x20 rupix -h | --help                  显示本帮助\n\
    \x20 rupix -V | --version               显示版本\n\
    \n\
    说明：\n\
    \x20 - 未指定 --output 时，保存到与图形界面共用的默认目录，文件名 rupix-<毫秒>.png。\n\
    \x20 - --output 可为文件或已存在的目录（目录则自动补文件名）；无扩展名时补 .png。\n\
    \x20 - 坐标为虚拟屏幕物理像素（多显示器取并集），如 100,200,800,600。\n\
    \x20 - --copy 把保存的文件再复制到剪贴板（Windows）。\n";

/// 命令行动具体作（参数解析结果）。
enum CliAction {
    /// 打印帮助。
    Help,
    /// 打印版本。
    Version,
    /// 执行截图并保存。
    Capture(CaptureRequest),
}

/// 一次命令行截图请求。
struct CaptureRequest {
    /// 截图区域（虚拟屏幕物理像素）；`None` 表示全屏。
    region: Option<Region>,
    /// 显式输出路径；`None` 表示用默认目录 + 时间戳文件名。
    output: Option<PathBuf>,
    /// 保存后是否再复制文件到剪贴板。
    copy: bool,
}

/// 截图区域（`--region x,y,w,h` 解析结果，虚拟屏幕物理像素）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Region {
    /// 左上角横坐标。
    x: i32,
    /// 左上角纵坐标。
    y: i32,
    /// 区域宽度（恒为正）。
    w: u32,
    /// 区域高度（恒为正）。
    h: u32,
}

/// 是否为命令行调用：带任意参数即走 CLI（截图后退出），无参数进图形界面。
pub fn is_cli_invocation() -> bool {
    std::env::args().len() > 1
}

/// 命令行入口：挂接父控制台 → 解析参数 → 执行截图，返回进程退出码。
///
/// 退出码：0 成功，1 参数错误，2 截图 / 保存失败。
pub fn run_cli() -> i32 {
    attach_console();
    let args: Vec<String> = std::env::args().skip(1).collect();
    match parse_args(&args) {
        Ok(CliAction::Help) => {
            print!("{HELP}");
            0
        }
        Ok(CliAction::Version) => {
            println!("rupix {}", env!("CARGO_PKG_VERSION"));
            0
        }
        Ok(CliAction::Capture(request)) => match execute_capture(&request) {
            Ok(path) => {
                println!("已保存：{}", path.display());
                0
            }
            Err(error) => {
                eprintln!("截图失败：{error}");
                2
            }
        },
        Err(error) => {
            eprintln!("参数错误：{error}\n\n{HELP}");
            1
        }
    }
}

/// 解析命令行参数（纯函数，无副作用，便于单测）。
fn parse_args(args: &[String]) -> Result<CliAction, String> {
    let mut fullscreen = false;
    let mut region: Option<Region> = None;
    let mut output: Option<PathBuf> = None;
    let mut copy = false;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "-h" | "--help" => return Ok(CliAction::Help),
            "-V" | "--version" => return Ok(CliAction::Version),
            "-f" | "--fullscreen" => {
                if region.is_some() {
                    return Err("--fullscreen 与 --region 不可同时使用".to_string());
                }
                fullscreen = true;
            }
            "-r" | "--region" => {
                if fullscreen {
                    return Err("--fullscreen 与 --region 不可同时使用".to_string());
                }
                index += 1;
                let spec = args
                    .get(index)
                    .ok_or("--region 缺少坐标（格式 x,y,w,h）".to_string())?;
                region = Some(parse_region(spec)?);
            }
            "-o" | "--output" => {
                index += 1;
                let path = args.get(index).ok_or("-o 缺少输出路径".to_string())?;
                if path.is_empty() {
                    return Err("-o 输出路径为空".to_string());
                }
                output = Some(PathBuf::from(path));
            }
            "--copy" => copy = true,
            unknown => return Err(format!("未知参数：{unknown}")),
        }
        index += 1;
    }
    if region.is_none() && !fullscreen {
        // 只有 -o / --copy 而没指定截图范围时，无法确定截什么。
        return Err("请指定 --fullscreen 或 --region x,y,w,h".to_string());
    }
    Ok(CliAction::Capture(CaptureRequest {
        region,
        output,
        copy,
    }))
}

/// 解析 `x,y,w,h`（如 `100,200,800,600`，物理像素，宽高必须为正）。
fn parse_region(spec: &str) -> Result<Region, String> {
    let parts: Vec<&str> = spec.split(',').collect();
    if parts.len() != 4 {
        return Err(format!(
            "区域格式错误：{spec}（应为 x,y,w,h，如 100,200,800,600）"
        ));
    }
    let number = |text: &str| {
        text.trim()
            .parse::<i32>()
            .map_err(|_| format!("区域数值非法：{spec}"))
    };
    let (x, y, w, h) = (
        number(parts[0])?,
        number(parts[1])?,
        number(parts[2])?,
        number(parts[3])?,
    );
    if w <= 0 || h <= 0 {
        return Err(format!("区域宽高必须为正：{spec}"));
    }
    Ok(Region {
        x,
        y,
        w: w as u32,
        h: h as u32,
    })
}

/// 执行截图：全屏为唯一像素来源，区域模式只是事后裁剪（与 GUI 首帧共享思路一致）。
fn execute_capture(request: &CaptureRequest) -> Result<PathBuf, String> {
    let full = capture_fullscreen()?;
    let img = match &request.region {
        Some(region) => crop_region(&full, region)?,
        None => full,
    };
    let path = save_image(request.output.as_deref(), &img)?;
    if request.copy {
        // 复制失败不影响已落盘文件，只告警。
        if let Err(error) = crate::export::copy_file_to_clipboard(&path) {
            eprintln!("警告：复制到剪贴板失败：{error}");
        }
    }
    Ok(path)
}

/// 按区域裁剪（与全图求交：部分越界按交集裁剪，完全无交集报错）。
fn crop_region(full: &RgbaImage, region: &Region) -> Result<RgbaImage, String> {
    let (full_w, full_h) = (full.width() as i32, full.height() as i32);
    let x0 = region.x.max(0);
    let y0 = region.y.max(0);
    let x1 = (region.x.saturating_add(region.w as i32)).min(full_w);
    let y1 = (region.y.saturating_add(region.h as i32)).min(full_h);
    if x1 <= x0 || y1 <= y0 {
        return Err(format!("区域 {} 与屏幕无交集", format_region(region)));
    }
    Ok(image::imageops::crop_imm(
        full,
        x0 as u32,
        y0 as u32,
        (x1 - x0) as u32,
        (y1 - y0) as u32,
    )
    .to_image())
}

/// 区域的人类可读形式（报错展示用）。
fn format_region(region: &Region) -> String {
    format!("{},{},{},{}", region.x, region.y, region.w, region.h)
}

/// 保存截图并返回最终路径（显式目录自动补文件名，无扩展名补 `.png`）。
fn save_image(explicit: Option<&Path>, img: &RgbaImage) -> Result<PathBuf, String> {
    let mut path = match explicit {
        Some(dir) if dir.is_dir() => dir.join(crate::app::timestamped_png_name()),
        Some(file) => file.to_path_buf(),
        None => crate::app::load_save_dir().join(crate::app::timestamped_png_name()),
    };
    if path.extension().is_none() {
        path.set_extension("png");
    }
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        // 输出到子目录时自动建目录，建失败直接报错（不静默落盘到别处）。
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("创建输出目录失败（{}）：{error}", parent.display()))?;
    }
    img.save(&path)
        .map_err(|error| format!("保存截图失败（{}）：{error}", path.display()))?;
    Ok(path)
}

/// 挂接父控制台（仅 Windows release）。
///
/// release 用 `windows_subsystem = "windows"` 编译，默认无控制台；
/// 从 cmd / PowerShell 启动时挂接父控制台，`println!` 才能被用户看到。
/// 挂接失败（如双击启动、CI 无头会话）则把标准输出 / 错误重定向到 NUL：
/// 此时根本没有可看的地方，保住退出码正确比直接崩溃更重要
///（`println!` 往无效句柄写会 panic，进程以 101 退出）。
#[cfg(target_os = "windows")]
fn attach_console() {
    use std::os::windows::io::IntoRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::Console::{
        ATTACH_PARENT_PROCESS, AttachConsole, STD_ERROR_HANDLE, STD_OUTPUT_HANDLE, SetStdHandle,
    };
    unsafe {
        if AttachConsole(ATTACH_PARENT_PROCESS).is_ok() {
            return;
        }
        // NUL 设备用标准库打开即可，不新增 windows 特性依赖；
        // into_raw_handle 会接管句柄（不析构），随进程结束自动回收。
        if let Ok(file) = std::fs::OpenOptions::new().write(true).open("NUL") {
            let handle = HANDLE(file.into_raw_handle() as isize);
            let _ = SetStdHandle(STD_OUTPUT_HANDLE, handle);
            let _ = SetStdHandle(STD_ERROR_HANDLE, handle);
        }
    }
}

/// 非 Windows 平台无需挂接控制台。
#[cfg(not(target_os = "windows"))]
fn attach_console() {}

/// Windows 实现：GDI 同步截取虚拟屏幕全屏（多显示器取并集）。
///
/// GUI 走 rgpui 推流取首帧（需事件循环）；CLI 无界面，改用 GDI `BitBlt` 一次拷贝，
/// 同步、无窗口、退出即释放，不与托盘进程的捕获会话冲突。
#[cfg(target_os = "windows")]
fn capture_fullscreen() -> Result<RgbaImage, String> {
    use windows::Win32::{
        Graphics::Gdi::{
            BITMAPINFO, BITMAPINFOHEADER, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC,
            DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, GetDIBits, ReleaseDC, SRCCOPY,
            SelectObject,
        },
        UI::WindowsAndMessaging::{
            GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
            SM_YVIRTUALSCREEN,
        },
    };

    // 虚拟屏幕（多显示器并集）：CLI 全屏即截全部显示器。
    let (left, top, width, height) = unsafe {
        (
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
        )
    };
    if width <= 0 || height <= 0 {
        return Err("获取屏幕尺寸失败".to_string());
    }

    unsafe {
        // 屏幕 DC（NULL 即全屏），后续 GDI 对象按逆序释放。
        let screen_dc = GetDC(None);
        if screen_dc.is_invalid() {
            return Err("获取屏幕设备上下文失败".to_string());
        }
        // 内存 DC + 兼容位图：BitBlt 拷贝目标。
        let mem_dc = CreateCompatibleDC(Some(screen_dc));
        if mem_dc.is_invalid() {
            ReleaseDC(None, screen_dc);
            return Err("创建内存设备上下文失败".to_string());
        }
        let bitmap = CreateCompatibleBitmap(screen_dc, width, height);
        if bitmap.is_invalid() {
            let _ = DeleteDC(mem_dc);
            ReleaseDC(None, screen_dc);
            return Err("创建兼容位图失败".to_string());
        }
        let old = SelectObject(mem_dc, bitmap.into());
        // 从虚拟屏幕原点拷贝整块到内存位图。
        let blit_ok = BitBlt(
            mem_dc,
            0,
            0,
            width,
            height,
            Some(screen_dc),
            left,
            top,
            SRCCOPY,
        )
        .is_ok();
        if !blit_ok {
            SelectObject(mem_dc, old);
            let _ = DeleteObject(bitmap.into());
            let _ = DeleteDC(mem_dc);
            ReleaseDC(None, screen_dc);
            return Err("复制屏幕像素失败".to_string());
        }
        // 32 位自顶向下 DIB（高度取负），GetDIBits 直接给出正向 BGRA。
        let mut info = BITMAPINFO::default();
        info.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        info.bmiHeader.biWidth = width;
        info.bmiHeader.biHeight = -height;
        info.bmiHeader.biPlanes = 1;
        info.bmiHeader.biBitCount = 32;
        info.bmiHeader.biCompression = 0; // BI_RGB：无压缩。
        let mut buf = vec![0u8; width as usize * 4 * height as usize];
        let lines = GetDIBits(
            mem_dc,
            bitmap,
            0,
            height as u32,
            Some(buf.as_mut_ptr() as *mut std::ffi::c_void),
            &mut info,
            DIB_RGB_COLORS,
        );
        // 用完即还：先恢复旧 GDI 对象，再删位图与 DC。
        SelectObject(mem_dc, old);
        let _ = DeleteObject(bitmap.into());
        let _ = DeleteDC(mem_dc);
        ReleaseDC(None, screen_dc);
        if lines == 0 {
            return Err("读取屏幕像素失败".to_string());
        }
        // BGRA 转 RGBA（与 capture.rs 首帧解码一致：image crate 通道顺序）。
        let (chunks, _) = buf.as_chunks_mut::<4>();
        for pixel in chunks {
            pixel.swap(0, 2);
        }
        RgbaImage::from_raw(width as u32, height as u32, buf)
            .ok_or_else(|| "组装图片失败".to_string())
    }
}

/// 非 Windows 平台：命令行截图暂不支持（与剪贴板文件项策略一致）。
#[cfg(not(target_os = "windows"))]
fn capture_fullscreen() -> Result<RgbaImage, String> {
    Err("当前平台暂不支持命令行截图".to_string())
}

#[cfg(test)]
mod tests {
    //! 命令行模块单测：参数解析与区域裁剪纯逻辑（GDI 截屏依赖系统，不进单测）。

    use super::*;

    /// 把 `&str` 切片转成参数向量（模拟 `std::env::args().skip(1)`）。
    fn args(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| item.to_string()).collect()
    }

    /// 全屏最小参数：`-f` 即合法，输出与复制均为空。
    #[test]
    fn fullscreen_minimal() {
        let action = parse_args(&args(&["-f"])).expect("应解析成功");
        let CaptureRequest {
            region,
            output,
            copy,
        } = assert_capture(action);
        assert!(region.is_none());
        assert!(output.is_none());
        assert!(!copy);
    }

    /// 区域 + 输出 + 复制组合解析。
    #[test]
    fn region_with_output_and_copy() {
        let action = parse_args(&args(&["-r", "100,200,800,600", "-o", "a.png", "--copy"]))
            .expect("应解析成功");
        let request = assert_capture(action);
        assert_eq!(
            request.region,
            Some(Region {
                x: 100,
                y: 200,
                w: 800,
                h: 600,
            })
        );
        assert_eq!(request.output, Some(PathBuf::from("a.png")));
        assert!(request.copy);
    }

    /// 断言解析结果为截图动作（非帮助 / 版本），并拆出请求。
    fn assert_capture(action: CliAction) -> CaptureRequest {
        match action {
            CliAction::Capture(request) => request,
            _ => panic!("应为截图动作"),
        }
    }

    /// 帮助与版本短长选项均可达。
    #[test]
    fn help_and_version() {
        assert!(matches!(
            parse_args(&args(&["--help"])),
            Ok(CliAction::Help)
        ));
        assert!(matches!(parse_args(&args(&["-h"])), Ok(CliAction::Help)));
        assert!(matches!(
            parse_args(&args(&["--version"])),
            Ok(CliAction::Version)
        ));
        assert!(matches!(parse_args(&args(&["-V"])), Ok(CliAction::Version)));
    }

    /// 区域格式用例：合法解析，非法（缺段 / 非数字 / 非正宽高）报错。
    #[test]
    fn region_format_cases() {
        assert!(parse_region("100,200,800,600").is_ok());
        assert!(parse_region(" -10 , -20 , 30 , 40 ").is_ok());
        assert!(parse_region("1,2,3").is_err());
        assert!(parse_region("a,b,c,d").is_err());
        assert!(parse_region("0,0,0,10").is_err());
        assert!(parse_region("0,0,10,-5").is_err());
    }

    /// 互斥与未知参数：全屏 + 区域同用、未知 flag、缺值均报错。
    #[test]
    fn conflicts_and_missing_values() {
        assert!(parse_args(&args(&["-f", "-r", "0,0,10,10"])).is_err());
        assert!(parse_args(&args(&["--wat"])).is_err());
        assert!(parse_args(&args(&["-r"])).is_err());
        assert!(parse_args(&args(&["-o"])).is_err());
        // 只有输出路径而没指定截什么，无法执行。
        assert!(parse_args(&args(&["-o", "a.png"])).is_err());
    }

    /// 区域裁剪：部分越界按交集裁剪，完全无交集报错。
    #[test]
    fn crop_clamps_to_screen() {
        // 合成底图 100x80，避免依赖真实屏幕。
        let full = RgbaImage::from_pixel(100, 80, image::Rgba([1, 2, 3, 255]));
        let partial = crop_region(
            &full,
            &Region {
                x: 90,
                y: 70,
                w: 20,
                h: 20,
            },
        )
        .expect("部分越界应按交集裁剪");
        assert_eq!(partial.dimensions(), (10, 10));
        assert!(
            crop_region(
                &full,
                &Region {
                    x: 200,
                    y: 200,
                    w: 10,
                    h: 10,
                },
            )
            .is_err()
        );
    }
}
