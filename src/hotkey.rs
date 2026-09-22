//! 全局快捷键模块：负责截图热键的持久化、解析与系统级注册。
//!
//! 截图必须在窗口没有焦点时也能触发，因此使用 rgpui 的系统级全局热键
//! （Windows 上底层为 `RegisterHotKey`），而不是仅窗口聚焦时生效的键位绑定。

use rgpui::{App, Keystroke, components::HotkeyValue};

use crate::i18n::{tr, tr_fmt};

/// 截图热键种类：区域、全屏、取色各一个系统级全局热键。
///（窗口吸附已合入区域截图，不再需要独立窗口热键；
/// 老版本残留的 `rupix-hotkey-window.txt` 会被忽略，不删除用户文件。）
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HotkeyKind {
    /// 区域截图（含窗口吸附）。
    Region,
    /// 全屏截图。
    Full,
    /// 屏幕取色。
    Picker,
}

impl HotkeyKind {
    /// 系统注册编号（三种热键互不干扰）。
    pub fn id(self) -> u32 {
        match self {
            HotkeyKind::Region => 1,
            HotkeyKind::Full => 2,
            HotkeyKind::Picker => 3,
        }
    }

    /// 出厂默认按键串（参考 Snipaste / 微信截图的常用组合）。
    pub fn default_text(self) -> &'static str {
        match self {
            HotkeyKind::Region => "ctrl-shift-a",
            HotkeyKind::Full => "ctrl-shift-f",
            HotkeyKind::Picker => "ctrl-shift-p",
        }
    }

    /// 配置文件名（与程序放同一目录；区域沿用旧单热键文件名，老配置直接生效）。
    pub fn file_name(self) -> &'static str {
        match self {
            HotkeyKind::Region => "rupix-hotkey.txt",
            HotkeyKind::Full => "rupix-hotkey-full.txt",
            HotkeyKind::Picker => "rupix-hotkey-picker.txt",
        }
    }

    /// 界面展示名（跟随当前语言）。
    pub fn label(self) -> String {
        match self {
            HotkeyKind::Region => tr("hotkey.region"),
            HotkeyKind::Full => tr("hotkey.full"),
            HotkeyKind::Picker => tr("hotkey.picker"),
        }
    }
}

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

/// 读取用户保存的截图热键，文件不存在或非法时回退到该种类的默认值。
pub fn load_hotkey_string(kind: HotkeyKind) -> String {
    // 配置与程序放在同一目录，重装也不会丢；取不到路径时直接用默认值。
    // 顺手校验一次，非法配置不污染本次运行。
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(kind.file_name())))
        .and_then(|path| std::fs::read_to_string(path).ok())
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty() && Keystroke::parse(text).is_ok())
        .unwrap_or_else(|| kind.default_text().to_string())
}

/// 保存用户录制的截图热键字符串。
///
/// # 参数
/// - `kind`: 热键种类（决定写哪个配置文件）。
/// - `hotkey`: 如 `ctrl-shift-a` 的按键串。
pub fn save_hotkey_string(kind: HotkeyKind, hotkey: &str) {
    // 写入失败仅忽略（不阻断改键流程），调用方已通过返回值提示注册结果。
    if let Some(path) = hotkey_file_path(kind) {
        let _ = std::fs::write(path, hotkey);
    }
}

/// 应用全局截图热键：先注销旧注册，再注册新按键串。
///
/// # 参数
/// - `cx`: rgpui 应用上下文。
/// - `kind`: 热键种类（决定系统注册编号）。
/// - `hotkey`: 如 `ctrl-shift-a` 的按键串。
///
/// # 返回
/// 注册成功返回 `Ok(())`；按键串非法或被系统占用时返回中文错误说明。
pub fn apply_global_hotkey(cx: &mut App, kind: HotkeyKind, hotkey: &str) -> Result<(), String> {
    // Windows 的 RegisterHotKey 只支持部分按键（如 F1-F12、单字符、方向键等），
    // 先解析再注册，错误直接展示给用户。
    let keystroke = Keystroke::parse(hotkey).map_err(|error| {
        tr_fmt(
            "hotkey.parse_fail",
            &[("hotkey", hotkey), ("error", &error.to_string())],
        )
    })?;
    // 同一编号重复注册前必须先注销，否则系统会返回占用错误。
    cx.unregister_global_hotkey(kind.id());
    cx.register_global_hotkey(kind.id(), &keystroke)
        .map_err(|error| {
            tr_fmt(
                "hotkey.register_fail",
                &[
                    ("label", &kind.label()),
                    ("hotkey", hotkey),
                    ("error", &error.to_string()),
                ],
            )
        })
}

/// 快捷键配置文件的完整路径。
fn hotkey_file_path(kind: HotkeyKind) -> Option<std::path::PathBuf> {
    // 优先放在程序所在目录， portable 风格，开箱即用；
    // 极端情况下（如路径不可取）回退到系统临时目录。
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        return Some(dir.join(kind.file_name()));
    }
    Some(std::env::temp_dir().join(kind.file_name()))
}
