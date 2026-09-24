//! 构建脚本：Windows 下把应用图标嵌入 exe（任务栏 / 资源管理器显示 logo），
//! 全平台把 OCR 默认模型同步到输出目录旁（开发期 exe 在 target 下也能命中 exe 旁 `models/`）。
//!
//! 图标资源在 `assets/` 下（`app.ico` 多尺寸 + `rupix.rc` 资源脚本）。

/// OCR 默认三件套文件名（与 `src/ocr.rs` 的默认三件套同名，改名需两边同步）。
const OCR_MODELS: &[&str] = &[
    "PP-OCRv5_mobile_det.mnn",
    "PP-OCRv5_mobile_rec.mnn",
    "ppocr_keys_v5.txt",
];

/// 入口：Windows 嵌图标 + 全平台同步 OCR 模型。
fn main() {
    // 目标是 Windows 时才嵌入 ICO；其他平台无事可做，避免影响跨平台构建。
    #[cfg(target_os = "windows")]
    {
        // rc 内相对路径按 rc 所在目录解析，即 `assets/app.ico`。
        // 图标编译失败直接中断构建（构建脚本按惯例用 unwrap，图标缺失不应静默发布；
        // 无资源编译器的环境按 NotAttempted 放行，见 embed-resource 文档）。
        embed_resource::compile("assets/rupix.rc", embed_resource::NONE)
            .manifest_optional()
            .unwrap();
    }
    stage_ocr_models();
}

/// OCR 默认模型分发：把 `assets/ocr-models/` 三件套同步到输出目录旁的 `models/`。
///
/// 查找优先级 exe 旁最高（见 `src/ocr.rs`），开发期 exe 在 `target/<profile>/` 下，
/// 这里保证该位置恒有模型可用；发版时随安装包把 `models` 目录放 exe 旁即可。
/// 仅当目标缺失或大小不一致才拷贝（约 21MB，避免每次构建都复制）。
fn stage_ocr_models() {
    // OUT_DIR 固定布局：`$profile/build/<包>-<哈希>/out`，上溯三级即输出目录。
    let Ok(out_dir) = std::env::var("OUT_DIR") else {
        return;
    };
    let Some(profile_dir) = std::path::Path::new(&out_dir).ancestors().nth(3) else {
        return;
    };
    let Ok(manifest_dir) = std::env::var("CARGO_MANIFEST_DIR") else {
        return;
    };
    let src_dir = std::path::Path::new(&manifest_dir).join("assets/ocr-models");
    let dst_dir = profile_dir.join("models");
    for name in OCR_MODELS {
        // 模型文件变化触发重新分发。
        println!("cargo:rerun-if-changed=assets/ocr-models/{name}");
        let src = src_dir.join(name);
        let dst = dst_dir.join(name);
        // 大小一致视为已同步，跳过拷贝。
        let synced = std::fs::metadata(&src)
            .and_then(|src_meta| {
                std::fs::metadata(&dst).map(|dst_meta| dst_meta.len() == src_meta.len())
            })
            .unwrap_or(false);
        if synced {
            continue;
        }
        // 源缺失（如仓库不完整）不阻断构建：外置查找会报缺模型，内嵌包在编译模型处报错。
        let Ok(()) = std::fs::create_dir_all(&dst_dir) else {
            return;
        };
        if std::fs::copy(&src, &dst).is_err() {
            return;
        }
    }
}
