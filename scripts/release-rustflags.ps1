# Prints the rustc flags for release builds as a CARGO_ENCODED_RUSTFLAGS value:
#   $env:CARGO_ENCODED_RUSTFLAGS = & ./scripts/release-rustflags.ps1
# - +crt-static: the same as .cargo/config.toml. An env var REPLACES the config file's rustflags,
#   so it has to be repeated here.
# - --remap-path-prefix: panic locations and debug info would otherwise embed this machine's
#   paths (user profile, cargo registry, toolchain, checkout). When several prefixes match, rustc
#   applies the last one, so the broad user-profile prefix comes first.
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$profileDir = [Environment]::GetFolderPath('UserProfile')
$cargoHome = if ($env:CARGO_HOME) { $env:CARGO_HOME } else { Join-Path $profileDir '.cargo' }
$rustupHome = if ($env:RUSTUP_HOME) { $env:RUSTUP_HOME } else { Join-Path $profileDir '.rustup' }

$flags = @('-C', 'target-feature=+crt-static')
$remaps = [ordered]@{}
if ($profileDir) { $remaps[$profileDir] = '/home' }
$remaps[$cargoHome] = '/cargo'
$remaps[$rustupHome] = '/rustup'
$remaps[$root] = '/mikyas'
foreach ($from in $remaps.Keys) {
    $full = [IO.Path]::GetFullPath($from).TrimEnd('\', '/')
    $flags += "--remap-path-prefix=$full=$($remaps[$from])"
}
$flags -join [char]0x1f
