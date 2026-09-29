# Builds the mikyas-capture shim (release) and places it where Tauri's externalBin expects it:
# src-tauri/binaries/mikyas-capture-<target-triple>.exe. Run before `npx tauri build`.
# Only the mikyas-capture binary is built (the integration tests' helper is an example, never built
# here), so nothing test-only is ever bundled.
#
# The target triple is TAURI_ENV_TARGET_TRIPLE when Tauri sets it, otherwise rustc's host triple.
# It is passed to cargo explicitly and cargo reports its own target directory, so the copied file
# is always the one just built (whatever CARGO_TARGET_DIR says, relative or not).
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot

Push-Location $root
try {
    $triple = $env:TAURI_ENV_TARGET_TRIPLE
    if (-not $triple) {
        $hostLine = rustc -vV | Select-String '^host: (.+)$'
        if ($LASTEXITCODE -ne 0 -or -not $hostLine) { throw "rustc -vV failed" }
        $triple = $hostLine.Matches[0].Groups[1].Value.Trim()
    }

    $metadata = cargo metadata --format-version 1 --no-deps
    if ($LASTEXITCODE -ne 0) { throw "cargo metadata failed ($LASTEXITCODE)" }
    $targetDir = ($metadata | ConvertFrom-Json).target_directory

    # Release flags (static CRT, no local paths embedded); restored afterwards for the caller.
    $savedFlags = $env:CARGO_ENCODED_RUSTFLAGS
    $env:CARGO_ENCODED_RUSTFLAGS = & (Join-Path $PSScriptRoot 'release-rustflags.ps1')
    try {
        cargo build -p mikyas-capture --release --bin mikyas-capture --target $triple --locked
        if ($LASTEXITCODE -ne 0) { throw "cargo build failed ($LASTEXITCODE)" }
    } finally {
        $env:CARGO_ENCODED_RUSTFLAGS = $savedFlags
    }
} finally {
    Pop-Location
}

$exe = if ($triple -like '*windows*') { '.exe' } else { '' }
$src = Join-Path $targetDir "$triple\release\mikyas-capture$exe"
$dstDir = Join-Path $root 'src-tauri\binaries'
New-Item -ItemType Directory -Force $dstDir | Out-Null
$dst = Join-Path $dstDir "mikyas-capture-$triple$exe"
Copy-Item -LiteralPath $src -Destination $dst -Force
Write-Host "sidecar: $dst ($((Get-Item $dst).Length) bytes)"
