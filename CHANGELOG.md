# 更新日志

格式参考 Keep a Changelog：`Added / Changed / Fixed`，日期为 tag 日期。

## [Unreleased]

- 见 `docs/ROADMAP.md` 的 0.2 规划（JSON 配置统一、开机自启）。

### Fixed

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
