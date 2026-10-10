# Rupix 路线图

## 0.2（下一版本）

### 1. 配置文件统一为 JSON

现状：exe 旁散落 6 个单值 txt（`rupix-savedir.txt` / `rupix-hotkey{,-full,-picker}.txt` /
`rupix-lang.txt` / `rupix-ocrfiles.txt`），难维护、难扩展。

目标：单个 `rupix-config.json`（仍放 exe 旁，保持绿色便携、重装不丢）：

```json
{
  "version": 1,
  "save_dir": "C:/Users/xxx/Pictures/rupix",
  "hotkeys": { "region": "Ctrl+Shift+A", "full": "Ctrl+Shift+F", "picker": "Ctrl+Shift+P" },
  "lang": "zh",
  "ocr": { "det": "C:/models/det.mnn", "rec": "C:/models/rec.mnn", "keys": "C:/models/keys.txt" },
  "autostart": false
}
```

- 新增 `serde` / `serde_json` 依赖（标准做法，体积可忽略）。
- 新增 `src/config.rs` 模块集中读写，对外 API 草拟：
  - `config::load() -> AppConfig`（文件缺失/损坏用默认值，不 panic）；
  - `config::update(f: impl FnOnce(&mut AppConfig))`（读-改-写原子落盘，写临时文件后 rename）；
  - 各字段 `Option` 化：`None` = 未设置，走现有回退链（系统默认/自动查找）。
- 首次启动自动迁移：读旧 txt → 合并进 JSON → 校验成功后删除旧 txt
  （任一步失败保留旧文件，下次启动再试，不丢配置）。
- 迁移映射：`rupix-savedir.txt`→`save_dir`，三个热键→`hotkeys.*`，
  `rupix-lang.txt`→`lang`，`rupix-ocrfiles.txt`三行→`ocr.{det,rec,keys}`。

### 2. 开机自启开关（已完成）

- 落地为 `src/autostart.rs`，设置 → 通用加"开机启动"开关；
  对外 API：`is_supported()` / `is_enabled()` / `set_enabled(on)`。
- Windows：`HKCU\Software\Microsoft\Windows\CurrentVersion\Run` 写 `Rupix = "<exe完整路径>"`，
  `windows` 依赖已加 `Win32_System_Registry` 特性；exe 路径加引号（防空格截断）；HKCU 无需管理员权限。
- macOS / Linux 一并实现（原计划"其他平台暂空实现"，实际三个平台都做了）：
  macOS 写 `~/Library/LaunchAgents/com.launcher-rs.rupix.plist` 并顺手 `launchctl bootstrap`，
  Linux 写 `~/.config/autostart/rupix.desktop`（XDG autostart）。
- 状态每次渲染实时读系统登记（外部改了也显示正确），**不另存期望值**：
  登记路径与当前 exe 不一致视为未开启，重开一次即用当前路径覆盖（搬目录后自愈）。
- 发版 release 说明里注明自启行为（默认关闭）。

### 3. 候选（待定优先级）

- 便携 zip 包：release 附 `exe + models/` 打包版本（常规版开箱即用）。
- OCR 小语种标签：家族表已支持 11 个小语种文件名，可继续补。
- 热键冲突提示：注册失败时指出被谁占用（尽力而为，系统 API 限制多）。
- 标注体验：按反馈继续打磨。

## 版本记录

- 0.1.0：首版（区域/全屏/窗口截图、取色、标注、托盘/热键/CLI、离线 OCR、双语）。
