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
- [x] 窗口截图（悬停高亮顶层窗口，点击截取，Windows）
- [x] 屏幕取色（冻结帧 + 放大镜 + 十字线，点击复制 HEX）
- [x] 预览与标注（缩放 / 平移 / 矩形 / 椭圆 / 箭头 / 画笔 / 荧光笔 / 橡皮擦 / 马赛克 / 文字 / 撤销重做 / 颜色 / 线宽，底部状态栏显示结果）
- [x] 保存（桌面时间戳新文件，状态栏可直接打开文件 / 目录，从不覆盖原图）
- [x] 双格式复制（Word / 画图粘贴为图片，资源管理器粘贴为文件）
- [x] 本地图片编辑（托盘 / 设置页打开任意图片做标注，另存新文件）
- [x] 双全局热键扩展为四热键（区域 / 全屏 / 取色 / 窗口各一个，设置页录制，失焦可用，默认 `Ctrl+Shift+A` / `Ctrl+Shift+F` / `Ctrl+Shift+P` / `Ctrl+Shift+W`）
- [x] 托盘常驻（启动即隐藏到托盘：区域截图 / 全屏截图 / 屏幕取色 / 窗口截图 / 打开图片编辑 / 打开设置 / 退出）
- [x] 命令行截图（`rupix -f` 全屏 / `rupix -r x,y,w,h` 区域，直存 PNG，可 `--copy` 进剪贴板）
- [x] OCR 文字识别（离线 PP-OCRv5 mobile，中英文；预览窗口点「识别」，后台识别不卡界面，结果进面板可复制文本 / 落为文字标注，需备好模型三件套）

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
| `ocr-rs`           | 离线 OCR（PP-OCRv5 mobile + MNN，中英文识别） |
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
│   ├── overlay.rs       区域选择浮层与窗口拾取浮层（冻结帧 + 拖拽框选 / 悬停高亮）
│   ├── picker.rs        屏幕取色浮层（放大镜 + 十字线，点击复制 HEX）
│   ├── window.rs        顶层窗口枚举与命中（Windows；其他平台返回空）
│   ├── viewer.rs        预览与标注窗口（含底部状态栏）
│   ├── export.rs        标注烧录、保存、双格式复制
│   ├── shapes.rs        矢量标注模型
│   ├── hotkey.rs        全局热键注册与持久化
│   ├── cli.rs           命令行截图（全屏 / 区域直存 PNG，不进图形界面）
│   ├── ocr.rs           离线 OCR（PP-OCRv5 mobile，识别底图并落成文字标注）
│   └── tray.rs          托盘常驻与菜单
├── Cargo.toml           包配置与依赖（含 release 优化）
├── README.md            项目说明（本文件）
└── AGENTS.md            AI 协作与代码规范说明
```

## 快速开始

> 注意：`cargo run` 是 debug 构建，未开优化，截图（冻结帧采集 / 渲染）会非常卡，仅用于快速验证逻辑。
> 日常使用 / 体验请务必用 release 构建（见下节「编译」）。

```bash
# rupix 目录下执行
cargo run              # 本地运行（启动即隐藏到托盘，用快捷键截图；debug 很卡，慎用）

# 命令行截图（不启动图形界面，可与托盘进程并存）
cargo run -- -f                          # 全屏截图，自动保存到默认目录
cargo run -- -f -o D:\shot.png           # 全屏截图，保存到指定文件
cargo run -- -r 100,200,800,600 -o a.png # 区域截图（虚拟屏幕物理像素）
cargo run -- --help                      # 查看全部参数

# 代码检查（提交前必须通过）
cargo fmt --check
cargo clippy -- -D warnings
cargo test
```

> OCR 构建前提：本机需安装 LLVM（bindgen 要用 libclang）。
> MSVC 头文件解析参数已由 `.cargo/config.toml` 自动带上，无需手动配环境变量；
> 若 LLVM 不在默认路径，再另行设置 `LIBCLANG_PATH` 指向其 `bin` 目录（改完重开终端）。
>
> OCR 模型（PP-OCRv5 mobile 三件套，约 21MB）已进仓库（`assets/ocr-models/`），构建脚本会自动同步到输出目录旁的 `models/`（开发/发版默认位置，开箱即用）。
> 查找顺序：`RUPIX_OCR_MODELS` 环境变量 > 设置页自定义目录 > exe 旁 `models/` > 工作目录 `models/`/`temp/ocr-models/` > 包根 `temp/`/`assets/`。
> 设置 → OCR 模型可逐个指定检测 / 识别 / 字典三个文件（文件名不做匹配，换模型直接选文件即可）；未指定时按默认目录自动查找（PP-OCRv5/v6 等家族按名识别，见 `src/ocr.rs`）。
> 模型下载位置二选一：ocr-rs 官方模型库（`https://github.com/zibo-chen/ocr-rs/tree/next/models`，全家在一个目录），或自有 HF 仓库（`https://huggingface.co/launcher-rs/rupix-models`，按家族分目录，推荐）。
> 更新模型版本才需跑：`powershell -ExecutionPolicy Bypass -File scripts/download-ocr-models.ps1`。
> 发版想零外部文件：`cargo run --features bundled-ocr-models`（二进制 +21MB，外置模型仍优先覆盖）。

## 编译

```bash
# 日常使用 / 发版：速度优先（opt-level=3 + thin LTO + strip），构建较快
cargo build --release
./target/release/rupix

# 打安装包：体积最小（opt-level=z + fat LTO + strip），构建更慢
cargo build --profile size
./target/size/rupix
```

说明：

- 不要用 `cargo run` 日常截图：debug 构建无优化，冻结帧采集与渲染会非常卡。
- `release` 是默认推荐；`size` 仅用于打安装包，日常开发继续用 `release`。
- 如需保留调试面板（F12 检查器），release 下显式开启：`cargo build --release --features inspector`。

> 为什么 debug 会非常卡？
> `cargo run` 默认是 debug 构建（`opt-level=0`，无内联 / 无自动向量化，保留 `debug-assertions` + `overflow-checks`，无 LTO）；
> 而 `release` 开了 `opt-level=3 + thin LTO + codegen-units=1`（见 `Cargo.toml`）。
> Rupix 的主链路恰好全是 debug 最怕的 CPU 密集像素运算：
> 一张 4K 冻结帧约 33MB，要走推流取帧 → PNG 落盘 → 解码（debug 下单次约半秒）→ RGBA→BGRA 逐像素转换 → 全屏浮层渲染 → 裁剪 / 马赛克 / 烧录导出。
> 这些循环在无优化下每个像素都有边界检查与函数调用开销，也无法向量化，比 release 慢一到两个数量级，所以拖拽选区、放大镜、取色都会掉帧。只用 `cargo run` 验证逻辑，体验务必用 release 包。

## 发版（GitHub Actions，手动触发）

不自动触发。去仓库 **Actions → release → Run workflow**，填 `tag`（如 `v0.2.0`，留空则取 `Cargo.toml` 版本加 `v` 前缀）与 `profile`（默认 `size`），CI 会编译并发布到 GitHub Releases：

- 矩阵：`windows-x86_64` / `linux-x86_64` / `macos-aarch64`；Windows 必须编过，Linux/macOS 只是尝试编译（本机无设备验证），挂了不阻塞发版。
- 每个平台两个包（对齐 0.1）：常规版 `rupix-<版本>-<平台>.zip`（exe + 外置 `models/`，解压即用）与内嵌版 `*-bundled.zip`（`--features bundled-ocr-models`，单文件开箱即用，体积 +21MB）。
- 常规包的 `models/` 由构建脚本自动从 `assets/ocr-models/` 分发；找不到模型时该平台只出内嵌包并告警。

## OCR 文字识别

预览窗口点「识别」，后台线程跑离线识别（前台不卡），完成后：

- 结果进右侧面板：点行可定位高亮并复制该行文本，也可一键复制全部；
- 确认无误再点「落为标注」，识别行转为文字标注（之后可像普通标注一样移动 / 删除）；
- 识别中重复点击会提示「识别中，请稍候」，不会重复开任务。

模型三件套（约 21MB，需自行准备）：`PP-OCRv5_mobile_det.mnn` / `PP-OCRv5_mobile_rec.mnn` / `ppocr_keys_v5.txt`，查找优先级：

1. 环境变量 `RUPIX_OCR_MODELS` 指向的目录；
2. 发版形态：exe 旁的 `models/` 目录；
3. 开发期：包根下 `temp/ocr-models/`（git 忽略）。

缺模型时点「识别」会提示应放入的目录，补齐后重试即可（引擎首次加载后常驻复用）。

## 命令行截图

不带参数启动图形界面（托盘常驻）；带截图参数时不启动界面，直接截图后退出，可与托盘进程并存，适合脚本调用。

```bash
rupix -f                              # 全屏截图，自动保存到默认目录
rupix -f -o D:\shot.png               # 全屏截图，保存到指定文件
rupix -r 100,200,800,600 -o a.png     # 区域截图（虚拟屏幕物理像素）
rupix -f --copy                       # 保存后再复制文件到剪贴板（Windows）
rupix --help                          # 查看全部参数
rupix --version                       # 显示版本
```

| 参数 | 说明 |
| ---- | ---- |
| `-f` / `--fullscreen` | 截取全屏（多显示器取并集） |
| `-r` / `--region x,y,w,h` | 截取指定区域，如 `100,200,800,600`；与全屏互斥 |
| `-o` / `--output 路径` | 输出文件或已存在目录（目录则自动补时间戳文件名）；无扩展名时补 `.png`；缺省为图形界面共用的默认目录（`图片库/rupix/rupix-<毫秒>.png`） |
| `--copy` | 保存后把文件再复制到剪贴板（Windows） |
| `-h` / `--help`、`-V` / `--version` | 帮助 / 版本 |

说明：

- 坐标为虚拟屏幕物理像素：多显示器取并集，主屏左上角不一定是 `(0,0)`；区域部分越界按交集裁剪，完全无交集报错。
- 退出码：`0` 成功，`1` 参数错误，`2` 截图 / 保存失败；成功时打印 `已保存：<路径>`。
- Windows 实现为 GDI 同步截屏（`BitBlt`），与其他截图软件的捕获会话不冲突；非 Windows 平台暂不支持。
- release 包无控制台窗口，从终端启动时会自动挂接父控制台输出结果。

## 贡献

欢迎提交 Issue 与 PR。提交代码前请阅读 `AGENTS.md`，特别注意：**所有新增与修改的 Rust 代码必须添加中文注释**。

## License

Apache-2.0，见 `LICENSE-APACHE`。
