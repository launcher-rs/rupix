//! 全局快捷键模块：负责截图热键的持久化、解析与系统级注册。
//!
//! 截图必须在窗口没有焦点时也能触发，因此使用 rgpui 的系统级全局热键
//! （Windows 上底层为 `RegisterHotKey`），而不是仅窗口聚焦时生效的键位绑定。

use rgpui::{App, Keystroke, components::HotkeyValue};

/// 全局截图热键在系统中的固定编号（`register_global_hotkey` 的 id 参数）。
pub const HOTKEY_ID: u32 = 1;

/// 出厂默认热键：Ctrl+Shift+A（参考 Snipaste / 微信截图的常用组合）。
pub const DEFAULT_HOTKEY: &str = "ctrl-shift-a";

/// 快捷键配置文件名，保存在程序所在目录，改键后下次启动仍然生效。
const HOTKEY_FILE_NAME: &str = "rupix-hotkey.txt";

/// 把录制到的快捷键值转换为 `Keystroke::parse` 可接受的按键串。
///
/// # 参数
/// - `value`: 快捷键录制框给出的按键值。
///
/// # 返回
/// 如 `ctrl-shift-a` 的小写按键串。
pub fn hotkey_value_to_string(value: &HotkeyValue) -> String {
    // 拼装顺序与 `Keystroke::parse` 的解析格式保持一致。
    let mut parts = Vec::new();
    if value.modifiers.control {
        parts.push("ctrl".to_string());
    }
    if value.modifiers.alt {
        parts.push("alt".to_string());
    }
    if value.modifiers.shift {
        parts.push("shift".to_string());
    }
    if value.modifiers.platform {
        parts.push("secondary".to_string());
    }
    parts.push(value.key.to_lowercase());
    parts.join("-")
}

/// 读取用户保存的截图热键，文件不存在或非法时回退到 [`DEFAULT_HOTKEY`]。
pub fn load_hotkey_string() -> String {
    // 配置与程序放在同一目录，重装也不会丢；取不到路径时直接用默认值。
    // 顺手校验一次，非法配置不污染本次运行。
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(HOTKEY_FILE_NAME)))
        .and_then(|path| std::fs::read_to_string(path).ok())
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty() && Keystroke::parse(text).is_ok())
        .unwrap_or_else(|| DEFAULT_HOTKEY.to_string())
}

/// 保存用户录制的截图热键字符串。
///
/// # 参数
/// - `hotkey`: 如 `ctrl-shift-a` 的按键串。
pub fn save_hotkey_string(hotkey: &str) {
    // 写入失败仅忽略（不阻断改键流程），调用方已通过返回值提示注册结果。
    if let Some(path) = hotkey_file_path() {
        let _ = std::fs::write(path, hotkey);
    }
}

/// 应用全局截图热键：先注销旧注册，再注册新按键串。
///
/// # 参数
/// - `cx`: rgpui 应用上下文。
/// - `hotkey`: 如 `ctrl-shift-a` 的按键串。
///
/// # 返回
/// 注册成功返回 `Ok(())`；按键串非法或被系统占用时返回中文错误说明。
pub fn apply_global_hotkey(cx: &mut App, hotkey: &str) -> Result<(), String> {
    // Windows 的 RegisterHotKey 只支持部分按键（如 F1-F12、单字符、方向键等），
    // 先解析再注册，错误直接展示给用户。
    let keystroke =
        Keystroke::parse(hotkey).map_err(|error| format!("快捷键解析失败（{hotkey}）：{error}"))?;
    // 同一编号重复注册前必须先注销，否则系统会返回占用错误。
    cx.unregister_global_hotkey(HOTKEY_ID);
    cx.register_global_hotkey(HOTKEY_ID, &keystroke)
        .map_err(|error| format!("全局快捷键注册失败（{hotkey}）：{error}"))
}

/// 快捷键配置文件的完整路径。
fn hotkey_file_path() -> Option<std::path::PathBuf> {
    // 优先放在程序所在目录， portable 风格，开箱即用；
    // 极端情况下（如路径不可取）回退到系统临时目录。
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        return Some(dir.join(HOTKEY_FILE_NAME));
    }
    Some(std::env::temp_dir().join(HOTKEY_FILE_NAME))
}
