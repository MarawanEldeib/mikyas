# Builds the cuw-capture shim (release) and places it where Tauri's externalBin expects it:
# src-tauri/binaries/cuw-capture-<target-triple>.exe. Run before `npx tauri build`.
# Only the cuw-capture binary is built, so the test helper cuw-test-child.exe is never bundled.
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$triple = 'x86_64-pc-windows-msvc'

Push-Location $root
try {
    cargo build -p cuw-capture --release --bin cuw-capture
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed ($LASTEXITCODE)" }
} finally {
    Pop-Location
}

$targetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $root 'target' }
$src = Join-Path $targetDir 'release\cuw-capture.exe'
$dstDir = Join-Path $root 'src-tauri\binaries'
New-Item -ItemType Directory -Force $dstDir | Out-Null
$dst = Join-Path $dstDir "cuw-capture-$triple.exe"
Copy-Item -LiteralPath $src -Destination $dst -Force
Write-Host "sidecar: $dst ($((Get-Item $dst).Length) bytes)"
