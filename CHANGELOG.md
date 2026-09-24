# 更新日志

格式参考 Keep a Changelog：`Added / Changed / Fixed`，日期为 tag 日期。

## [Unreleased]

- 见 `docs/ROADMAP.md` 的 0.2 规划（JSON 配置统一、开机自启）。

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
