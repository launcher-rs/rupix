//! 开机启动（自启动）模块：把本程序登记进当前系统的登录自启动项。
//!
//! 三个平台各走各的机制，“是否开启”一律实时读取真实状态，不另存配置文件，
//! 这样被外部（任务管理器 / 系统设置）改动后，设置页显示的仍是最新的事实：
//!
//! - Windows：`HKCU\Software\Microsoft\Windows\CurrentVersion\Run` 写一个 `REG_SZ`
//!   （值为带引号的 exe 绝对路径）；
//! - macOS：`~/Library/LaunchAgents/com.launcher-rs.rupix.plist`
//!   （写完顺手 `launchctl bootstrap` 让本回会话立即生效，失败忽略，登录必生效）；
//! - Linux：`~/.config/autostart/rupix.desktop`（XDG autostart，桌面环境登录时读取）。
//!
//! 全部只写“当前用户”范围的项，不需要管理员 / root 权限；
//! 关闭时删除登记即可，不留残留。

use anyhow::{Context, Result};
use std::path::PathBuf;

/// 当前 exe 的绝对路径（自启动项要登记的就是它）。
fn exe_path() -> Result<PathBuf> {
    std::env::current_exe().context("读取当前程序路径失败")
}

/// 当前平台是否支持开机启动（Windows / macOS / Linux 支持，其余平台设置页隐藏开关）。
pub fn is_supported() -> bool {
    cfg!(any(
        target_os = "windows",
        target_os = "macos",
        target_os = "linux"
    ))
}

/// 开机启动当前是否已开启（实时读注册表 / 自启动文件，不缓存）。
pub fn is_enabled() -> bool {
    platform::enabled()
}

/// 开启或关闭开机启动。
///
/// # 参数
/// - `on`: `true` 登记自启动项，`false` 删除登记（已删除视为成功，保证可重复调用）。
///
/// # 返回
/// 成功返回 `Ok(())`；失败返回原因（目录权限、注册表拒绝写入等），
/// 由设置页状态栏原样展示，开关状态保持不变。
pub fn set_enabled(on: bool) -> Result<()> {
    platform::set(on)
}

// ————————————————————————— Windows —————————————————————————

#[cfg(target_os = "windows")]
mod platform {
    //! Windows 实现：往 `HKCU\...\CurrentVersion\Run` 写 / 删一个 `REG_SZ` 值。

    use super::exe_path;
    use anyhow::Result;
    use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
    use windows::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE, REG_SZ, RRF_RT_REG_SZ, RegCloseKey,
        RegDeleteValueW, RegGetValueW, RegOpenKeyExW, RegSetValueExW,
    };
    use windows::core::PCWSTR;

    /// Run 键路径（HKCU = 当前用户，无需管理员权限）。
    const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    /// 自启动项的值名。
    const VALUE_NAME: &str = "Rupix";

    /// Rust 字符串 → UTF-16 终止串（所有 `*W` 系 API 都要 `PCWSTR`）。
    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// 读 Run 键里的自启动项，且要求登记的路径就是当前 exe。
    ///
    /// 刻意按“路径一致”判断而不是“值存在”：程序换了安装位置后旧登记是死链，
    /// 这里显示未开启，用户再点一次开启即可用当前路径覆盖，天然自愈。
    pub(super) fn enabled() -> bool {
        // exe 路径读不到（极罕见）时无从比对，按未开启处理。
        let Ok(exe) = exe_path() else {
            return false;
        };
        let sub_key = wide(RUN_KEY);
        let name = wide(VALUE_NAME);
        // 2KB 缓冲足够装下任何合法路径；RegGetValue 会把数据按字节返回。
        let mut buf = [0u16; 1024];
        let mut bytes = (buf.len() * 2) as u32;
        let err = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                PCWSTR(sub_key.as_ptr()),
                PCWSTR(name.as_ptr()),
                RRF_RT_REG_SZ,
                None,
                Some(buf.as_mut_ptr().cast()),
                Some(&mut bytes),
            )
        };
        if err != ERROR_SUCCESS {
            // 文件不存在 = 没开过；其他错误一律按未开启处理，不阻断设置页。
            return false;
        }
        // 字节数 → UTF-16 切片，交给 decode_reg_sz 剥掉结尾 NUL 再转字符串。
        let units = &buf[..((bytes as usize / 2).min(buf.len()))];
        let stored = decode_reg_sz(units);
        // 我们写入时带引号，比对前把引号剥掉，大小写不敏感（Windows 路径特性）。
        stored
            .trim_matches('"')
            .eq_ignore_ascii_case(&exe.to_string_lossy())
    }

    /// 把 RegGetValueW 读回的 REG_SZ UTF-16 数据解码成字符串。
    ///
    /// `RegGetValueW` 对 REG_SZ 返回的数据是**连结尾 NUL 一起计入字节数**的，
    /// 必须先剔干净：`from_utf16_lossy` 会把 U+0000 原样变成 `'\0'` 字符，
    /// 之后 `trim_matches('"')` 从尾部遇到 `'\0'` 就停，连引号都剥不掉，
    /// 比对恒不相等——开关刚打开就显示「已关闭」。
    fn decode_reg_sz(units: &[u16]) -> String {
        // 切片没有 trim_end_matches，手动找最后一个非 NUL 单元的位置；
        // 全是 NUL（空值）时按空串处理。
        let end = units
            .iter()
            .rposition(|unit| *unit != 0)
            .map_or(0, |pos| pos + 1);
        String::from_utf16_lossy(&units[..end])
    }

    /// 写入或删除 Run 键中的自启动值。
    pub(super) fn set(on: bool) -> Result<()> {
        // 先算好要写的内容：拿到 exe 路径再开句柄，避免出错时句柄泄漏。
        let value = if on {
            // 路径可能含空格，必须整体加引号，否则开机启动会被拆成两段执行。
            Some(format!("\"{}\"", exe_path()?.display()))
        } else {
            None
        };
        let sub_key = wide(RUN_KEY);
        let mut key = HKEY::default();
        // 只申请 KEY_SET_VALUE：Run 键对当前用户可写，写不进去就如实报错。
        let opened = unsafe {
            RegOpenKeyExW(
                HKEY_CURRENT_USER,
                PCWSTR(sub_key.as_ptr()),
                None,
                KEY_SET_VALUE,
                &mut key,
            )
        };
        if opened != ERROR_SUCCESS {
            anyhow::bail!("打开注册表 Run 键失败（错误码 {}）", opened.0);
        }
        // 写 / 删结果统一在这里收口，句柄 HKEY 无 Drop，必须手动关。
        let result = if let Some(value) = &value {
            // REG_SZ 数据是带结尾 0 的 UTF-16 字节序列。
            let units: Vec<u16> = value.encode_utf16().chain(std::iter::once(0)).collect();
            let bytes: Vec<u8> = units.iter().flat_map(|unit| unit.to_le_bytes()).collect();
            let name = wide(VALUE_NAME);
            unsafe { RegSetValueExW(key, PCWSTR(name.as_ptr()), None, REG_SZ, Some(&bytes)) }
        } else {
            let name = wide(VALUE_NAME);
            unsafe { RegDeleteValueW(key, PCWSTR(name.as_ptr())) }
        };
        // 无论成败都关句柄（关闭本身的结果不影响业务结果，忽略即可）。
        let _ = unsafe { RegCloseKey(key) };
        match result {
            // 关闭时值本来就不在 = 已经是关闭状态，视为成功（幂等）。
            ERROR_SUCCESS => Ok(()),
            ERROR_FILE_NOT_FOUND if value.is_none() => Ok(()),
            code => anyhow::bail!(
                "{}注册表自启动项失败（错误码 {}）",
                if value.is_some() { "写入" } else { "删除" },
                code.0
            ),
        }
    }

    #[cfg(test)]
    mod tests {
        //! Windows 自启动单测：只测纯字符串解码逻辑（注册表读写不进单测）。

        use super::decode_reg_sz;

        /// 模拟 RegGetValueW 读回的数据（带引号 + 结尾 NUL），
        /// 解码后应能与裸路径正常比对。
        #[test]
        fn decode_reg_sz_strips_quotes_and_trailing_nul() {
            let raw: Vec<u16> = "\"C:\\Program Files\\rupix.exe\"\0"
                .encode_utf16()
                .collect();
            // 结尾 NUL 先被剥掉，trim_matches('"') 才能把两侧引号都去掉。
            assert!(
                decode_reg_sz(&raw)
                    .trim_matches('"')
                    .eq_ignore_ascii_case("c:\\program files\\rupix.exe")
            );
        }

        /// 无引号登记 + 连续多个 NUL 的数据也应被正确剥离。
        #[test]
        fn decode_reg_sz_handles_unquoted_and_multi_nul() {
            let raw: Vec<u16> = "C:\\a\\rupix.exe\0\0".encode_utf16().collect();
            assert_eq!(decode_reg_sz(&raw), "C:\\a\\rupix.exe");
        }
    }
}

// ————————————————————————— macOS —————————————————————————

#[cfg(target_os = "macos")]
mod platform {
    //! macOS 实现：写 `~/Library/LaunchAgents/<LABEL>.plist` 登记 LaunchAgent。

    use super::exe_path;
    use anyhow::{Context, Result};
    use std::path::{Path, PathBuf};

    /// LaunchAgent 的 Label，也用作 plist 文件名。
    const LABEL: &str = "com.launcher-rs.rupix";

    /// plist 文件路径：`~/Library/LaunchAgents/<LABEL>.plist`。
    fn file() -> Result<PathBuf> {
        let home = std::env::var("HOME").context("读取 HOME 环境变量失败")?;
        Ok(PathBuf::from(home)
            .join("Library")
            .join("LaunchAgents")
            .join(format!("{LABEL}.plist")))
    }

    /// 文件里登记的是否正是当前 exe（换过位置的旧登记按未开启处理，重开一次即修复）。
    fn records_current_exe(path: &Path) -> bool {
        let (Ok(text), Ok(exe)) = (std::fs::read_to_string(path), exe_path()) else {
            return false;
        };
        text.contains(&*exe.to_string_lossy())
    }

    /// 把路径塞进 `<string>` 前做最小 XML 转义（路径里出现 `& < >` 也不至于写出坏文件）。
    fn xml_escape(text: &str) -> String {
        text.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    }

    /// 生成 plist 内容：登录后无条件启动当前 exe，不弹 Dock 图标由程序自身（隐藏启动）决定。
    fn plist(exe: &str) -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{LABEL}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{exe}</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
</dict>
</plist>
"#,
            exe = xml_escape(exe)
        )
    }

    /// 执行一条 `launchctl` 子命令并忽略结果：
    /// 文件本身才是登录时的依据，立刻加载只是让“本回会话”也生效，
    /// 失败（无 launchctl / 已加载 / 沙箱限制）不影响下次登录生效。
    fn launchctl(args: &[&str]) {
        let _ = std::process::Command::new("launchctl")
            .args(args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }

    pub(super) fn enabled() -> bool {
        file()
            .map(|path| records_current_exe(&path))
            .unwrap_or(false)
    }

    pub(super) fn set(on: bool) -> Result<()> {
        let exe = exe_path()?;
        let path = file()?;
        if on {
            // LaunchAgents 目录首次使用时可能不存在，先建。
            let dir = path.parent().context("解析 LaunchAgents 目录失败")?;
            std::fs::create_dir_all(dir).context("创建 LaunchAgents 目录失败")?;
            std::fs::write(&path, plist(&exe.to_string_lossy()))
                .context("写入 LaunchAgent plist 失败")?;
            // 立刻加载，让当前会话也生效。
            let uid = std::process::Command::new("id")
                .arg("-u")
                .output()
                .ok()
                .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
                .filter(|uid| !uid.is_empty());
            if let Some(uid) = uid {
                launchctl(&[
                    "bootstrap",
                    &format!("gui/{uid}"),
                    &path.display().to_string(),
                ]);
            }
            Ok(())
        } else {
            // 先卸载再删文件；卸载失败（没加载过）无所谓。
            // 只有文件里登记的确实是当前 exe 时才 bootout，避免误停别的程序的同名项。
            if let Ok(text) = std::fs::read_to_string(&path)
                && text.contains(&*exe.to_string_lossy())
            {
                let uid = std::process::Command::new("id")
                    .arg("-u")
                    .output()
                    .ok()
                    .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
                    .filter(|uid| !uid.is_empty());
                if let Some(uid) = uid {
                    launchctl(&[
                        "bootout",
                        &format!("gui/{uid}"),
                        &path.display().to_string(),
                    ]);
                }
            }
            remove_if_exists(&path)
        }
    }

    /// 删除文件；不存在也算成功（重复关闭保持幂等）。
    fn remove_if_exists(path: &Path) -> Result<()> {
        match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error).context("删除 LaunchAgent plist 失败"),
        }
    }
}

// ————————————————————————— Linux —————————————————————————

#[cfg(target_os = "linux")]
mod platform {
    //! Linux 实现：写 XDG autostart 文件 `~/.config/autostart/rupix.desktop`。

    use super::exe_path;
    use anyhow::{Context, Result};
    use std::path::{Path, PathBuf};

    /// autostart 文件名。
    const FILE_NAME: &str = "rupix.desktop";

    /// autostart 文件路径：优先 `XDG_CONFIG_HOME`，未设置按规范回退 `~/.config`。
    fn file() -> Result<PathBuf> {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|dir| !dir.as_os_str().is_empty())
            .or_else(|| {
                std::env::var("HOME")
                    .ok()
                    .map(|home| PathBuf::from(home).join(".config"))
            })
            .context("读取用户配置目录失败（HOME / XDG_CONFIG_HOME 均未设置）")?;
        Ok(base.join("autostart").join(FILE_NAME))
    }

    /// 文件里登记的是否正是当前 exe（换过位置的旧登记按未开启处理，重开一次即修复）。
    fn records_current_exe(path: &Path) -> bool {
        let (Ok(text), Ok(exe)) = (std::fs::read_to_string(path), exe_path()) else {
            return false;
        };
        text.contains(&*exe.to_string_lossy())
    }

    /// 生成 `.desktop` 内容（Exec 值含空格要加引号，见桌面项规范）。
    fn desktop_entry(exe: &str) -> String {
        format!(
            "[Desktop Entry]\n\
             Type=Application\n\
             Name=Rupix 拾景\n\
             Comment=Rupix lightweight screenshot tool\n\
             Exec=\"{exe}\"\n\
             Terminal=false\n\
             X-GNOME-Autostart-enabled=true\n"
        )
    }

    pub(super) fn enabled() -> bool {
        file()
            .map(|path| records_current_exe(&path))
            .unwrap_or(false)
    }

    pub(super) fn set(on: bool) -> Result<()> {
        let exe = exe_path()?;
        let path = file()?;
        if on {
            // ~/.config/autostart 首次使用时可能不存在，先建。
            let dir = path.parent().context("解析 autostart 目录失败")?;
            std::fs::create_dir_all(dir).context("创建 autostart 目录失败")?;
            std::fs::write(&path, desktop_entry(&exe.to_string_lossy()))
                .context("写入 autostart 文件失败")?;
            Ok(())
        } else {
            match std::fs::remove_file(&path) {
                Ok(()) => Ok(()),
                // 文件本来就不存在 = 已经是关闭状态，视为成功（幂等）。
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error).context("删除 autostart 文件失败"),
            }
        }
    }
}

// ————————————————————————— 其他平台 —————————————————————————

#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
mod platform {
    //! 未适配平台：一律报告不支持，设置页不再展示开关（`is_supported` 先拦一道）。
    use anyhow::Result;

    pub(super) fn enabled() -> bool {
        false
    }

    pub(super) fn set(_on: bool) -> Result<()> {
        anyhow::bail!("当前系统暂不支持开机启动")
    }
}
