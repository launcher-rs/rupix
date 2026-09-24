# Rupix OCR model downloader: fetches the PP-OCRv5 mobile trio from ocr-rs upstream.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File scripts/download-ocr-models.ps1
#   powershell -ExecutionPolicy Bypass -File scripts/download-ocr-models.ps1 -Target Dev
#   powershell -ExecutionPolicy Bypass -File scripts/download-ocr-models.ps1 -Target Bundled
#
# Source: https://github.com/zibo-chen/ocr-rs/tree/next/models (matches ocr-rs 2.4).
# The trio is ~21MB. After download, `cargo run` works (external mode);
# release bundles use `cargo run --features bundled-ocr-models`.
param(
    [ValidateSet("All", "Dev", "Bundled")]
    [string]$Target = "All"
)

$ErrorActionPreference = "Stop"

# Upstream model base URL (raw links, pinned to the branch ocr-rs 2.4 pairs with).
$BaseUrl = "https://raw.githubusercontent.com/zibo-chen/ocr-rs/next/models"
# Trio file names (must match DET_MODEL / REC_MODEL / KEYS_FILE in src/ocr.rs).
$Files = @(
    "PP-OCRv5_mobile_det.mnn",
    "PP-OCRv5_mobile_rec.mnn",
    "ppocr_keys_v5.txt"
)

# Repo root (the script lives in scripts/, its parent is the repo root).
$RepoRoot = Split-Path -Parent $PSScriptRoot
$DevDir = Join-Path $RepoRoot "temp/ocr-models"
$BundledDir = Join-Path $RepoRoot "assets/ocr-models"

if ($Target -eq "Dev") {
    $DestDirs = @($DevDir)
} elseif ($Target -eq "Bundled") {
    $DestDirs = @($BundledDir)
} else {
    $DestDirs = @($DevDir, $BundledDir)
}

foreach ($dir in $DestDirs) {
    New-Item -ItemType Directory -Force -Path $dir | Out-Null
    foreach ($file in $Files) {
        $dest = Join-Path $dir $file
        if (Test-Path -LiteralPath $dest) {
            Write-Host "Exists, skip: $dest"
            continue
        }
        $url = "$BaseUrl/$file"
        Write-Host "Downloading $url -> $dest"
        Invoke-WebRequest -Uri $url -OutFile $dest -UseBasicParsing
    }
}

Write-Host "Done. Verify the trio, then run cargo run (external) or cargo run --features bundled-ocr-models (bundled)."
