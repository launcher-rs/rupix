//! 统一配置模块：exe 旁单个 `rupix-config.json` 集中保存全部用户设置。
//!
//! - 绿色便携：文件与程序同目录，重装不丢；
//! - 字段全部 `Option` 化：`null` 表示未设置，由各业务模块走原有回退链；
//! - 读改写走内存缓存 + 临时文件 rename，避免写到一半崩溃留下半个 JSON；
//! - JSON 损坏时备份为 `rupix-config.bak.json` 并回默认值，不 panic。

use std::{
    fs,
    path::{Path, PathBuf},
    sync::{OnceLock, RwLock},
};

use serde::{Deserialize, Serialize};

/// 当前配置结构版本（结构变更时递增，为后续迁移留锚点）。
const CONFIG_VERSION: u32 = 1;
/// 统一配置文件名（与程序放同一目录）。
const CONFIG_FILE: &str = "rupix-config.json";
/// 统一配置的临时文件名：先写这里再 rename，保证落盘原子性。
const CONFIG_TMP_FILE: &str = "rupix-config.json.tmp";
/// 配置损坏时的备份文件名（保留用户数据，便于手工找回）。
const CONFIG_BACKUP_FILE: &str = "rupix-config.bak.json";

/// 应用全部用户配置（字段缺失或为 `null` 时由业务模块回退默认值）。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    /// 配置结构版本（写入时固定为 [`CONFIG_VERSION`]）。
    pub version: u32,
    /// 截图默认保存目录；`None` 表示用系统图片库默认目录。
    pub save_dir: Option<PathBuf>,
    /// 三个系统级全局热键；`None` 表示用出厂默认组合。
    pub hotkeys: Hotkeys,
    /// 界面语言代码（`zh` / `en` / 外部 `lang-<code>.txt` 的 `code`）。
    pub lang: Option<String>,
    /// 手动指定的 OCR 模型三件套；任一项缺失即回退自动查找。
    pub ocr: OcrFiles,
    /// 截图完成后的行为开关。
    pub shot: ShotBehavior,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            version: CONFIG_VERSION,
            save_dir: None,
            hotkeys: Hotkeys::default(),
            lang: None,
            ocr: OcrFiles::default(),
            shot: ShotBehavior::default(),
        }
    }
}

/// 三个全局热键的按键串（如 `ctrl-shift-a`，与 `Keystroke::parse` 同格式）。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Hotkeys {
    /// 区域截图。
    pub region: Option<String>,
    /// 全屏截图。
    pub full: Option<String>,
    /// 屏幕取色。
    pub picker: Option<String>,
}

/// 手动指定的 OCR 模型三件套（检测 / 识别 / 字典）。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct OcrFiles {
    /// 检测模型。
    pub det: Option<PathBuf>,
    /// 识别模型。
    pub rec: Option<PathBuf>,
    /// 字符字典。
    pub keys: Option<PathBuf>,
}

/// 截图完成后的行为开关。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ShotBehavior {
    /// 截图后自动复制到剪贴板（默认开：截完直接 `Ctrl+V`）。
    pub auto_copy: bool,
    /// 截图后打开编辑窗（默认关：只在右下角闪保存提示）。
    pub open_editor: bool,
}

impl Default for ShotBehavior {
    fn default() -> Self {
        Self {
            auto_copy: true,
            open_editor: false,
        }
    }
}

/// 全局配置缓存（进程内单例，`update` 同步写盘）。
static STATE: OnceLock<RwLock<AppConfig>> = OnceLock::new();

/// 取全局配置缓存（首次访问时按磁盘内容初始化，保证任何调用路径可用）。
fn state() -> &'static RwLock<AppConfig> {
    STATE.get_or_init(|| RwLock::new(load_from_disk()))
}

/// 读取当前配置（返回副本，调用方随便改不会影响缓存）。
pub fn load() -> AppConfig {
    match state().read() {
        Ok(guard) => guard.clone(),
        Err(poisoned) => poisoned.into_inner().clone(),
    }
}

/// 读-改-写：在缓存副本上应用 `f` 后原子落盘。
///
/// 写盘失败只忽略（本次运行内存里仍生效）。
///
/// # 参数
/// - `f`: 修改配置的回调（如 `|cfg| cfg.shot.auto_copy = true`）。
pub fn update(f: impl FnOnce(&mut AppConfig)) {
    let mut guard = match state().write() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    f(&mut guard);
    guard.version = CONFIG_VERSION;
    let _ = write_config(&config_path(), &guard);
}

/// 配置目录：优先程序所在目录（绿色便携），取不到时回退系统临时目录。
fn config_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
        .unwrap_or_else(std::env::temp_dir)
}

/// 配置文件的完整路径（对外暴露给诊断 / 文档用）。
pub fn config_path() -> PathBuf {
    config_dir().join(CONFIG_FILE)
}

/// 读取磁盘配置：文件缺失或损坏时返回默认值。
fn load_from_disk() -> AppConfig {
    load_or_recover(&config_path())
}

/// 读取指定 JSON；解析失败时把损坏文件备份一份再回默认值。
fn load_or_recover(path: &Path) -> AppConfig {
    let Ok(text) = fs::read_to_string(path) else {
        return AppConfig::default();
    };
    match serde_json::from_str::<AppConfig>(&text) {
        Ok(cfg) => cfg,
        Err(_) => {
            // 备份而不是直接覆盖：用户手改出错时还能找回内容。
            let _ = fs::rename(path, path.with_file_name(CONFIG_BACKUP_FILE));
            AppConfig::default()
        }
    }
}

/// 原子写盘：先写临时文件再 rename，避免中途失败留下半个 JSON。
fn write_config(path: &Path, cfg: &AppConfig) -> std::io::Result<()> {
    let text = serde_json::to_string_pretty(cfg).map_err(std::io::Error::other)?;
    let tmp = path.with_file_name(CONFIG_TMP_FILE);
    fs::write(&tmp, format!("{text}\n"))?;
    // 同一目录内 rename；Windows 上会覆盖已存在的目标文件。
    fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    //! 配置单测：默认值、JSON 往返与损坏文件兜底（只碰临时目录）。

    use super::*;

    /// 为单测创建唯一临时目录（进程号 + 纳秒，避免并行冲突）。
    fn temp_dir(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "rupix-config-test-{tag}-{}-{nanos}",
            std::process::id()
        ));
        let _ = fs::create_dir_all(&dir);
        dir
    }

    /// 缺省值：自动复制开、打开编辑窗关、字段为空走业务回退。
    #[test]
    fn defaults_are_expected() {
        let cfg = AppConfig::default();
        assert_eq!(cfg.version, CONFIG_VERSION);
        assert!(cfg.save_dir.is_none());
        assert!(cfg.hotkeys.region.is_none());
        assert!(cfg.lang.is_none());
        assert!(cfg.shot.auto_copy);
        assert!(!cfg.shot.open_editor);
    }

    /// JSON 往返：写入后读回，字段与类型保持一致。
    #[test]
    fn writes_and_reads_roundtrip() {
        let dir = temp_dir("roundtrip");
        let path = dir.join(CONFIG_FILE);
        let cfg = AppConfig {
            save_dir: Some(PathBuf::from("C:/shots")),
            hotkeys: Hotkeys {
                full: Some("ctrl-shift-f".to_string()),
                ..Default::default()
            },
            ocr: OcrFiles {
                keys: Some(PathBuf::from("C:/models/keys.txt")),
                ..Default::default()
            },
            shot: ShotBehavior {
                open_editor: true,
                ..Default::default()
            },
            ..Default::default()
        };
        write_config(&path, &cfg).unwrap();

        let back = load_or_recover(&path);
        assert_eq!(back.save_dir, cfg.save_dir);
        assert_eq!(back.hotkeys.full.as_deref(), Some("ctrl-shift-f"));
        assert_eq!(back.ocr.keys, cfg.ocr.keys);
        assert!(back.shot.auto_copy && back.shot.open_editor);
        let _ = fs::remove_dir_all(&dir);
    }

    /// 损坏的 JSON：回默认值并留存备份，不 panic。
    #[test]
    fn corrupt_json_falls_back_and_backs_up() {
        let dir = temp_dir("corrupt");
        let path = dir.join(CONFIG_FILE);
        fs::write(&path, "{ broken").unwrap();
        let cfg = load_or_recover(&path);
        assert!(cfg.shot.auto_copy);
        assert!(!path.exists());
        assert!(dir.join(CONFIG_BACKUP_FILE).is_file());
        let _ = fs::remove_dir_all(&dir);
    }
}
