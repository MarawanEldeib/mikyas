# No network client crates in the (Windows) dependency tree: the only network use is the opt-in
# update check via System32 curl.exe. deny.toml bans the same crates; this check also runs where
# cargo-deny is not installed.
#   ./scripts/ci/no-network-crates.ps1
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)

Push-Location $root
try {
    $tree = cargo tree --workspace --target x86_64-pc-windows-msvc -e normal --prefix none
    if ($LASTEXITCODE -ne 0) { throw "cargo tree failed" }
} finally {
    Pop-Location
}
$hits = $tree | Select-String -Pattern '^(reqwest|hyper|ureq|isahc|surf|attohttpc|opentelemetry)[ -]'
if ($hits) { $hits; throw "network client crates in the dependency tree" }
"no network client crates"
