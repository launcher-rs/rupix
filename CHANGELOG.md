# 更新日志

格式参考 Keep a Changelog：`Added / Changed / Fixed`，日期为 tag 日期。

## [Unreleased]

- 见 `docs/ROADMAP.md` 的 0.2 规划（JSON 配置统一；开机自启已在本版本落地，见下）。

### Added

- 设置页「截图」新增两个截图行为开关：「截图后自动复制到剪贴板」（默认开，截完直接 `Ctrl+V` 可用）、
  「截图后打开编辑窗」（默认关）；开关存程序目录 `rupix-shot-autocopy.txt` / `rupix-shot-openeditor.txt`，
  与热键、保存目录等配置同一风格
- 新增 `src/toast.rs` 截图保存提示：不弹编辑窗时在屏幕右下角闪一枚胶囊（1.6 秒自动消失、
  不抢焦点、经 `WDA_EXCLUDEFROMCAPTURE` 排除出抓屏），交代「已保存（并复制）」与文件名
- 截图收尾统一到 `app::finish_capture`（区域框选与全屏两条路径同一处判断）：
  自动保存 → 按设置复制 → 打开编辑窗或闪提示；`autosave_screenshot` 负责保存后写剪贴板
- 新增「编辑最近一张」入口（设置页截图 Tab 与托盘菜单）：截图收尾时在进程内记下最近一张
  （`RupixApp::last_preview`，不写盘），关掉「截图后打开编辑窗」后可一键重开标注；
  没有截图或文件已删时入口置灰 / 只回写状态栏提示
- 设置页新增「关于」Tab：版本号（取自 `Cargo.toml`）、简介与仓库地址，
  支持一键打开仓库与复制链接
- 设置页「通用」新增开机启动开关（`src/autostart.rs`）：Windows 写 `HKCU\...\CurrentVersion\Run`、
  macOS 写 `~/Library/LaunchAgents/com.launcher-rs.rupix.plist`、Linux 写 `~/.config/autostart/rupix.desktop`；
  只写当前用户范围，无需管理员权限；状态实时读取系统登记，与任务管理器 / 系统设置的改动保持一致
- 新增 `ci` 工作流（push `main` / PR / 手动）：`cargo fmt --check` + `cargo clippy -- -D warnings`
  → 三平台 `cargo build --release` → `--version` / `--help` 冒烟 → `cargo test --release --bins`
  → glibc 基线与 `otool -L` 信息输出

### Changed

- 截图默认不再自动弹出编辑窗：截完直接保存并（默认）复制，右下角闪一下提示；
  需要标注时在设置 → 截图里打开「截图后打开编辑窗」，或从托盘「打开图片编辑」进来
- 发版工作流加串行并发控制（`concurrency: release`，不取消进行中的发版），
  避免 tag 发版与手动 `latest` 发版同时跑互相覆盖附件；原发版流程里的冒烟步骤整体迁到 `ci`

### Fixed

- Windows：开机启动开关打开后立刻又显示「已关闭」——`autostart::enabled()` 读回 REG_SZ 时
  数据带结尾 NUL（`RegGetValueW` 对 REG_SZ 连终止符一起返回），`from_utf16_lossy` 把它变成
  `'\0'` 字符后 `trim_matches('"')` 连引号都剥不掉，比对恒不相等。现在解码前先把尾部 NUL
  剥干净（`decode_reg_sz`），并补了对应单测
- Linux：区域截图看不到遮罩层与选区框。Rupix 侧把两个全屏浮层（`overlay.rs` 选区层、
  `picker.rs` 取色层）的 `window_background` 从 `Transparent` 改为 `Opaque` —— 它们铺的是
  一张冻结的整屏截图，本来就不需要窗口级透明；配合 rgpui 侧 X11 的 visual 选择修复
  （仅当「请求非 Opaque + 服务器有 32 位 visual + 合成器在场」才走 ARGB）后浮层正常上屏
- Linux：启动后设置窗口直接甩在桌面上（Windows 是隐藏到托盘）、以及设置窗口出现后先黑屏
  一段时间才出内容 —— 两条根因都在 rgpui 的 X11 后端（忽略 `WindowOptions::show == false`、
  建窗未设 `background_pixel`），Rupix 侧无改动，随 rgpui 更新生效

### Changed

- `rgpui` / `rgpui-platform` 依赖临时切到本地源码路径（`../rgpui/crates/*`）以带上上述 Linux
  修复；rgpui 发版后把 `path` 换回 `version`

## [0.1.0] - 2026-09-24

首个版本：看到 → 截取 → 留存。

### Added

- 区域截图（全屏冻结帧拖拽框选，尺寸徽标，右键 / Esc 取消）
- 全屏截图（截取即进预览）与窗口截图（悬停高亮，Windows）
- 屏幕取色（放大镜 + 十字线，点击复制 HEX）
- 预览与标注（缩放 / 平移 / 矩形 / 椭圆 / 箭头 / 画笔 / 荧光笔 / 橡皮擦 / 马赛克 / 文字 / 撤销重做）
- 保存（桌面时间戳新文件）与双格式复制（图片 + 文件）
- 本地图片编辑、四全局热键、托盘常驻、命令行截图（`-f` / `-r` / `-o` / `--copy`）
- 离线 OCR（PP-OCRv5 mobile，后台识别，编号框 + 文字面板 + 落为标注）
- OCR 三处来源：内嵌包（`--features bundled-ocr-models`）/ 默认目录 / 设置页手动指定三个文件
- 中英双语、设置页 Tab（截图 / 快捷键 / 保存 / OCR 模型 / 通用）

### Changed

- 发版构建走 `size` 档（体积优先）；静态 CRT（免 MSVC 运行库）

### 发布附件

- `rupix-0.1.0-windows-x64.exe`（常规版，模型外置）
- `rupix-0.1.0-windows-x64-bundled.exe`（内嵌版，开箱即用）
