# Full-history secret scan with a pinned, checksum-verified gitleaks (CI security job; also runs
# locally). Needs a full clone (fetch-depth: 0) to see every commit.
#   ./scripts/ci/gitleaks.ps1
$ErrorActionPreference = 'Stop'
$version = '8.30.1'
# SHA-256 of gitleaks_8.30.1_windows_x64.zip, from the release's gitleaks_8.30.1_checksums.txt.
$sha256 = 'd29144deff3a68aa93ced33dddf84b7fdc26070add4aa0f4513094c8332afc4e'

$root = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$tmp = if ($env:RUNNER_TEMP) { $env:RUNNER_TEMP } else { [IO.Path]::GetTempPath() }
$dir = Join-Path $tmp "gitleaks-$version"
$exe = Join-Path $dir 'gitleaks.exe'

if (-not (Test-Path $exe)) {
    $zip = Join-Path $tmp "gitleaks-$version.zip"
    $url = "https://github.com/gitleaks/gitleaks/releases/download/v$version/gitleaks_${version}_windows_x64.zip"
    Invoke-WebRequest -Uri $url -OutFile $zip
    $hash = (Get-FileHash $zip -Algorithm SHA256).Hash
    if ($hash -ne $sha256) { throw "gitleaks download checksum mismatch: $hash" }
    Expand-Archive $zip -DestinationPath $dir -Force
}

Push-Location $root
try {
    & $exe git --redact --no-banner --config .gitleaks.toml
    if ($LASTEXITCODE -ne 0) { throw "gitleaks found a secret" }
} finally {
    Pop-Location
}
