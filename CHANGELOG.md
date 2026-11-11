# 更新日志

格式参考 Keep a Changelog：`Added / Changed / Fixed`，日期为 tag 日期。

## [Unreleased]

后续规划见 [docs/ROADMAP.md](docs/ROADMAP.md)。

## [0.3.0] - 2026-11-11

### Added

- 多显示器支持：区域截图与取色会在每块显示器上各开一层冻结首帧浮层，可在任意屏幕框选 /
  取色；一轮多屏浮层共享 `OverlaySession`，任一处完成、取消或被系统关闭时统一收尾。
  混合 DPI 下按「首帧物理尺寸 / 显示器逻辑尺寸」换算，窗口吸附改为桌面物理坐标命中
  （叠加各屏物理原点），副屏高亮与整窗截取不再偏移
- 全屏截图（设置页按钮 / `rupix -f` / CLI）改为逐屏取首帧后按物理坐标拼接并集，
  多显示器不再只截主屏

### Changed

- `rgpui` / `rgpui-platform` 升级到 1.7（滚动越界回弹与到边滚轮丢失修复，新增向导表单 /
  差异查看器等组件）

## [0.2.1] - 2026-11-01

### Added

- 设置页「截图」新增两个截图行为开关：「截图后自动复制到剪贴板」（默认开，截完直接 `Ctrl+V` 可用）、
  「截图后打开编辑窗」（默认关）；开关与热键、保存目录等设置一起存于 `rupix-config.json`
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

- 配置文件统一为 exe 旁单个 `rupix-config.json`（`src/config.rs`），取代原来的 8 个单值 txt
  （保存目录 / 三个热键 / 语言 / OCR 三件套 / 两个截图行为开关）；JSON 损坏时备份为
  `rupix-config.bak.json` 并回默认值。开机启动状态仍实时读系统登记，不写入配置
- 截图默认不再自动弹出编辑窗：截完直接保存并（默认）复制，右下角闪一下提示；
  需要标注时在设置 → 截图里打开「截图后打开编辑窗」，或从托盘「打开图片编辑」进来
- 发版工作流加串行并发控制（`concurrency: release`，不取消进行中的发版），
  避免 tag 发版与手动 `latest` 发版同时跑互相覆盖附件；原发版流程里的冒烟步骤整体迁到 `ci`
- `rgpui` / `rgpui-platform` 升级到 1.6（含 Linux 浮层 visual 选择、`show=false` 隐藏启动、
  帧同步握手与截图浮层 Esc 无法取消的修复）
- 文档：重写 README（核心特点、四张实机界面截图、下载 / 使用 / FAQ），
  发版与 CI 说明拆到 [docs/RELEASING.md](docs/RELEASING.md)；同步 ROADMAP、AGENTS 的模块与配置说明
- 清理未使用的颜色预设与导出常量，`cargo clippy --all-targets -- -D warnings` 全绿

### Fixed

- 跨平台默认保存目录：非 Windows 平台此前误读 `USERPROFILE` / `OneDrive`（Windows 专有环境变量），
  现在 macOS / Linux 依次尝试 `XDG_PICTURES_DIR`、`~/Pictures`、`~/Desktop`，都不可用时回退系统临时目录
- macOS / Linux 标注文字烧录缺失：字体候选此前全是 Windows 字体，现按平台选择
  （macOS：PingFang / STHeiti / Helvetica；Linux：Noto Sans CJK / 文泉驿 / DejaVu）
- Windows 剪贴板写入失败时 `HGLOBAL` 未释放：加锁或 `SetClipboardData` 失败路径现在会 `GlobalFree`
- 截取中重复触发热键会叠加浮层：区域 / 取色浮层打开期间保持 `capturing`，浮层关闭（含 Alt+F4）
  或打开失败时统一复位，按钮与状态栏不再卡在「截取中」
- 切换界面语言后托盘菜单仍是旧语言：语言变更时同步刷新托盘提示与菜单
- OCR 通用模型兜底在模式匹配守卫里被同名绑定遮蔽，导致文件名不在已知家族表里时找不到模型
  （`find_trio_cases` 覆盖）
- 去掉库逻辑里的 `unwrap` / `expect`（复制标注图、OCR 模型三件套、主窗口创建），
  失败改为状态栏提示 / 日志输出
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
