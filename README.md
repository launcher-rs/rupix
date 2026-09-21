# Rupix · 拾景

<img src="assets/logo.png" width="128" alt="Rupix logo" />

> **Rupix（拾景）是一款基于 Rust 与 rgpui 构建的轻量级跨平台截图工具，用于快速拾取并保存屏幕上的精彩瞬间。**
>
> **Rupix — A lightweight screenshot tool built with Rust and rgpui.**

## 名字含义

**拾景 = 拾取 + 景象**

- **拾**：拾取、收集、留存。从屏幕中拾取一个瞬间。
- **景**：景象、画面、所见之景。屏幕上的画面、窗口、区域、一瞬间的视觉内容。

> **以像素承载画面，以拾取留住所见。**
>
> **看到 → 截取 → 留存**

## 功能特性

- [x] 区域截图（全屏冻结帧上拖拽框选，尺寸徽标，右键 / Esc 取消）
- [x] 全屏截图（截取即进预览）
- [x] 预览与标注（缩放 / 平移 / 矩形 / 椭圆 / 箭头 / 画笔 / 马赛克 / 文字 / 撤销重做 / 颜色 / 线宽，底部状态栏显示结果）
- [x] 保存（桌面时间戳新文件，状态栏可直接打开文件 / 目录，从不覆盖原图）
- [x] 双格式复制（Word / 画图粘贴为图片，资源管理器粘贴为文件）
- [x] 本地图片编辑（托盘 / 设置页打开任意图片做标注，另存新文件）
- [x] 双全局热键（区域 / 全屏各一个，设置页录制，失焦可用，默认 `Ctrl+Shift+A` / `Ctrl+Shift+F`）
- [x] 托盘常驻（启动即隐藏到托盘：区域截图 / 全屏截图 / 打开图片编辑 / 打开设置 / 退出）
- [ ] 窗口截图（Window Capture）
- [ ] OCR 文字识别
- [ ] 屏幕取色器（Color Picker）

## 技术栈

```text
Rust + rgpui + rgpui-platform
```

| 依赖              | 用途                          |
| ----------------- | ----------------------------- |
| `rgpui`           | UI 渲染与交互框架             |
| `rgpui-platform`  | 跨平台窗口 / 事件支持         |
| `image`           | 图片编解码、裁剪与导出        |
| `ab_glyph`        | 文字标注的字形光栅化          |
| `windows`         | Windows 双格式剪贴板（图片+文件） |
| `embed-resource`  | Windows exe 图标嵌入（构建依赖） |

Rust 版本要求：见 `Cargo.toml` 中 `edition = "2024"`，建议使用最新 stable 工具链。

## 项目结构

```text
rupix/
├── assets/              应用图标与托盘图标（logo.png / app.ico / tray.png）
├── build.rs             Windows exe 图标嵌入
├── src/
│   ├── main.rs          程序入口（设置窗口默认隐藏到托盘）
│   ├── app.rs           设置窗口：热键录制、截图入口、保存历史
│   ├── capture.rs       截图核心（推流取首帧）
│   ├── overlay.rs       区域选择浮层（冻结帧 + 拖拽框选）
│   ├── viewer.rs        预览与标注窗口（含底部状态栏）
│   ├── export.rs        标注烧录、保存、双格式复制
│   ├── shapes.rs        矢量标注模型
│   ├── hotkey.rs        全局热键注册与持久化
│   └── tray.rs          托盘常驻与菜单
├── Cargo.toml           包配置与依赖（含 release 优化）
├── README.md            项目说明（本文件）
└── AGENTS.md            AI 协作与代码规范说明
```

## 快速开始

```bash
# rupix 目录下执行
cargo run              # 本地运行（启动即隐藏到托盘，用快捷键截图）

# 构建 release（已配 opt-level=3 / LTO / strip）
cargo build --release

# 代码检查（提交前必须通过）
cargo fmt --check
cargo clippy -- -D warnings
cargo test
```

## 贡献

欢迎提交 Issue 与 PR。提交代码前请阅读 `AGENTS.md`，特别注意：**所有新增与修改的 Rust 代码必须添加中文注释**。

## License

Apache-2.0，见 `LICENSE-APACHE`。
