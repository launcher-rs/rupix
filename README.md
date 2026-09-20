# Rupix · 拾景

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

## 功能定位

Rupix 的第一款产品定位非常简单：**轻量、快速的桌面截图工具**。核心理念不是复杂的图片编辑，而是快速拾取屏幕中的视觉信息。

- [x] 全屏截图（Fullscreen Capture）
- [x] 区域截图（Region Capture，八手柄选区 + 尺寸徽标）
- [ ] 窗口截图（Window Capture）
- [x] 标注（矩形 / 椭圆 / 箭头 / 画笔 / 荧光笔 / 文字 / 马赛克 / 橡皮擦 / 撤销重做）
- [ ] OCR 文字识别
- [ ] 取色器（Color Picker）
- [x] 剪贴板（复制 PNG 到剪贴板）
- [x] 快捷键（全局热键，可在主窗口录制自定义，默认 `Ctrl+Shift+A`）
- [x] 托盘常驻（关闭主窗口进托盘，托盘菜单截图 / 显示 / 退出）

## 技术栈

```text
Rust + rgpui + rgpui-platform
```

| 依赖             | 用途                 |
| ---------------- | -------------------- |
| `rgpui`          | UI 渲染与交互框架    |
| `rgpui-platform` | 跨平台窗口 / 事件支持 |

Rust 版本要求：见 `Cargo.toml` 中 `edition = "2024"`，建议使用最新 stable 工具链。

## 项目结构

```text
launcher-rs/rupix
├── src/
│   └── main.rs        入口
├── Cargo.toml         包配置与依赖
├── README.md          项目说明（本文件）
└── AGENTS.md          AI 协作与代码规范说明
```

后续规划中的模块划分：

```text
src/
├── main.rs            程序入口
├── app.rs             应用状态与根视图
├── capture/           截图核心（全屏 / 区域 / 窗口）
├── editor/            标注编辑（图形 / 文字 / 马赛克）
├── color_picker.rs    取色器
├── ocr.rs             文字识别
├── clipboard.rs       剪贴板交互
├── hotkey.rs          全局快捷键
├── tray.rs            托盘常驻
└── i18n.rs            中英文国际化
```

## 快速开始

```bash
# 克隆（rupix 为 launcher-rs 的子项目）
git clone <launcher-rs-repo-url>
cd launcher-rs/rupix

# 运行
cargo run

# 构建 release
cargo build --release

# 代码检查
cargo fmt --check
cargo clippy -- -D warnings
```

## 贡献

欢迎提交 Issue 与 PR。提交代码前请阅读 `AGENTS.md`，特别注意：**所有代码必须添加中文注释**。

## License

待定（建议 MIT / Apache-2.0 双许可，与 Rust 生态保持一致）。
