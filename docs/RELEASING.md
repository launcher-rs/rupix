# 发版与持续集成

本文面向维护者，说明 Rupix 的本地检查、CI 与 GitHub Release 流程。
用户安装 / 使用说明见仓库根目录的 [README](../README.md)。

## 1. 本地检查（提交前）

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --bins
```

发版前另外确认 release / size 档能编译：

```bash
cargo build --release            # 日常与正式发版使用的档位
cargo build --profile size       # 打发布包使用的档位（体积最小）
```

## 2. 持续集成（`.github/workflows/ci.yml`）

触发条件：推 `main`、开 PR、手动触发；同一分支的新提交会取消进行中的旧跑。

两个 job：

- **lint**（`ubuntu-22.04`）：`cargo fmt --check` + `cargo clippy -- -D warnings`，
  先给出「能不能合」的结论；
- **build**（三平台矩阵，需 lint 通过）：
  - Windows 先跑一次 `cargo clippy --release`——剪贴板、注册表开机启动、控制台挂接等
    `cfg(windows)` 专属代码只有 Windows 才会被编译到，Linux 的 clippy 看不见；
  - `cargo build --release` → `--version` / `--help` 冒烟；
  - `cargo test --release --bins`（examples 是历史 spike，不参与）；
  - Linux 打印 glibc 基线、macOS 打印 `otool -L`，仅作信息，不作失败判定。

## 3. 发布（`.github/workflows/release.yml`）

同一条工作流，两种模式：

### 3.1 正式版本：打 tag 自动触发

```bash
git tag v0.2.1
git push origin v0.2.1
```

- Release 名与 tag 同名，**不碰 `latest`**，编译档固定 `size`；
- prepare 阶段校验 tag 与 `Cargo.toml` 的 `version` 一致，不一致直接失败——
  防止发出「标签写 0.3.0、`rupix --version` 却打 0.2.0」的误标包；
  改版本号后记得让 `Cargo.lock` 一起更新（构建时会自动同步）；
- 重复推同一 tag 只覆盖本平台附件，不会删已有 Release。

### 3.2 滚动 `latest`：手动触发

仓库 **Actions → release → Run workflow**，选择：

- `profile`：`size`（默认，体积最小）或 `release`（速度优先）；
- `platform`：`all`（默认）/ `windows` / `linux` / `macos`。

发的是固定 `latest` Release：

- 全平台触发时删旧建新；单选某平台时只替换该平台附件，可单独重编失败的平台；
- 各平台编完即传，不互相等待；
- 发版串行（`concurrency: release`，不取消进行中任务），避免两种触发方式互相覆盖附件。

### 3.3 产物与平台

矩阵：`windows-x86_64` / `linux-x86_64` / `macos-aarch64`。
Windows 必须编过；Linux / macOS 为尽力而为（本机无对应设备完整验证，失败不阻塞发版）。

每个平台两个包：

| 包 | 内容 |
| --- | --- |
| 常规版 `rupix-<平台>.zip` / `.tar.gz` | exe + 外置 `models/`（OCR 开箱即用），解压即用 |
| 内嵌版 `rupix-<平台>-bundled.zip` / `.tar.gz` | `--features bundled-ocr-models`，模型打进二进制，单文件开箱即用（体积约 +21MB） |

- 常规包的 `models/` 由 `build.rs` 从 `assets/ocr-models/` 自动分发；缺失时只出内嵌包并告警；
- Windows 使用静态 CRT，免装 MSVC 运行库；Linux 基线 glibc ≥ 2.35
  （Ubuntu 22.04+ / Debian 12+ / Fedora 35+）；macOS 为 Apple Silicon 构建。

## 4. 发布检查清单

1. 更新 `Cargo.toml` 的 `version`（`Cargo.lock` 同步）；
2. 在 `CHANGELOG.md` 补上本次版本的条目与日期；
3. UI 有变化时更新 `assets/screenshots/` 与 README 截图；
4. 推送 `main`，等 CI 全绿；
5. 打与版本号一致的 tag 并推送；
6. 下载各平台产物做一次冒烟：`rupix --version`、`--help`、截图 / 取色 / OCR 各一次；
7. 检查 Release 说明中的平台限制与开机启动行为（默认关闭）是否仍准确。
