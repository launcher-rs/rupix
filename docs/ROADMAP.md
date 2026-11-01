# Rupix 路线图

## 下一版本（0.3）

### 候选（待定优先级）

- 多显示器区域截图：当前冻结帧只取主显示器，需按显示器拼接并处理混 DPI。
- OCR 小语种标签：家族表已支持 11 个小语种文件名，可继续补。
- 热键冲突提示：注册失败时指出被谁占用（尽力而为，系统 API 限制多）。
- 标注体验：按反馈继续打磨。

## 已完成

- **配置文件统一为 JSON**（0.2.1，原计划 0.3）：`src/config.rs`，exe 旁单个 `rupix-config.json` 取代原来散落的
  8 个单值 txt（保存目录 / 三个热键 / 语言 / OCR 三件套 / 两个截图行为开关）。

  ```json
  {
    "version": 1,
    "save_dir": "C:/Users/xxx/Pictures/rupix",
    "hotkeys": { "region": "ctrl-shift-a", "full": "ctrl-shift-f", "picker": "ctrl-shift-p" },
    "lang": "zh",
    "ocr": { "det": "C:/models/det.mnn", "rec": "C:/models/rec.mnn", "keys": "C:/models/keys.txt" },
    "shot": { "auto_copy": true, "open_editor": false }
  }
  ```

  - 对外 API：`load() -> AppConfig` / `update(|cfg| ...)`（读改写后先写临时文件再 rename）；
    字段 `Option` 化，`null` 表示未设置、走各业务模块原有回退链。
  - JSON 损坏时备份为 `rupix-config.bak.json` 并回默认值，不 panic。
  - 开机启动状态仍实时读系统登记（注册表 / LaunchAgents / XDG），**不写入 JSON**，
    避免与系统真实状态脱节。

- **开机自启开关**（0.2.1）：`src/autostart.rs`，设置 → 通用加"开机启动"开关；
  对外 API：`is_supported()` / `is_enabled()` / `set_enabled(on)`。
  - Windows：`HKCU\Software\Microsoft\Windows\CurrentVersion\Run` 写 `Rupix = "<exe完整路径>"`，
    exe 路径加引号（防空格截断），HKCU 无需管理员权限。
  - macOS / Linux 一并实现：macOS 写 `~/Library/LaunchAgents/com.launcher-rs.rupix.plist`
    并顺手 `launchctl bootstrap`，Linux 写 `~/.config/autostart/rupix.desktop`（XDG autostart）。
  - 状态每次渲染实时读系统登记（外部改了也显示正确），**不另存期望值**：
    登记路径与当前 exe 不一致视为未开启，重开一次即用当前路径覆盖（搬目录后自愈）。
  - 默认关闭，发版 release 说明里注明该行为。
- **便携 zip 包**（0.2.1）：发版工作流产出常规版（exe + 外置 `models/`）与内嵌版两个 zip。

## 版本记录

- 0.2.1：配置文件统一为 `rupix-config.json`；截图行为开关（自动复制 / 打开编辑窗）、
  保存提示胶囊、编辑最近一张、开机启动、关于页、CI 与三平台发版；rgpui 升级到 1.6。
- 0.1.0：首版（区域/全屏/窗口截图、取色、标注、托盘/热键/CLI、离线 OCR、双语）。
