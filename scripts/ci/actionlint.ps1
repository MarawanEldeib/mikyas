# Lints .github/workflows/*.yml with a pinned, checksum-verified actionlint (downloaded to the temp
# dir, never installed). Run locally after editing a workflow:
#   ./scripts/ci/actionlint.ps1
$ErrorActionPreference = 'Stop'
$version = '1.7.12'
# SHA-256 of actionlint_1.7.12_windows_amd64.zip, from the release's actionlint_1.7.12_checksums.txt.
$sha256 = '6e7241b51e6817ea6a047693d8e6fed13b31819c9a0dd6c5a726e1592d22f6e9'

$root = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$tmp = if ($env:RUNNER_TEMP) { $env:RUNNER_TEMP } else { [IO.Path]::GetTempPath() }
$dir = Join-Path $tmp "actionlint-$version"
$exe = Join-Path $dir 'actionlint.exe'

if (-not (Test-Path $exe)) {
    $zip = Join-Path $tmp "actionlint-$version.zip"
    $url = "https://github.com/rhysd/actionlint/releases/download/v$version/actionlint_${version}_windows_amd64.zip"
    Invoke-WebRequest -Uri $url -OutFile $zip
    $hash = (Get-FileHash $zip -Algorithm SHA256).Hash
    if ($hash -ne $sha256) { throw "actionlint download checksum mismatch: $hash" }
    Expand-Archive $zip -DestinationPath $dir -Force
}

Push-Location $root
try {
    & $exe -color
    if ($LASTEXITCODE -ne 0) { throw "actionlint found problems" }
} finally {
    Pop-Location
}
