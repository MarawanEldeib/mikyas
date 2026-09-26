# The app version must agree in package.json, Cargo.toml and src-tauri/tauri.conf.json.
# With -Tag, it must also equal the tag without its "v" (release workflow).
#   ./scripts/ci/check-versions.ps1 [-Tag v0.2.0]
param([string]$Tag)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)

$npm = (Get-Content (Join-Path $root 'package.json') -Raw | ConvertFrom-Json).version
$tauri = (Get-Content (Join-Path $root 'src-tauri/tauri.conf.json') -Raw | ConvertFrom-Json).version
$cargo = (Select-String -Path (Join-Path $root 'Cargo.toml') -Pattern '^version = "(.+)"' |
    Select-Object -First 1).Matches.Groups[1].Value

"package.json $npm · tauri.conf.json $tauri · Cargo.toml $cargo"
if (-not $npm -or $npm -ne $tauri -or $npm -ne $cargo) { throw "version mismatch" }
if ($Tag -and "v$npm" -ne $Tag) { throw "tag $Tag does not match version $npm" }
