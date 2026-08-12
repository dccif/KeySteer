[CmdletBinding()]
param()

$ErrorActionPreference = "Stop"
$root = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$target = if ($env:PROCESSOR_ARCHITECTURE -eq "ARM64") {
    "aarch64-pc-windows-msvc"
} else {
    "x86_64-pc-windows-msvc"
}
$env:CARGO_TARGET_DIR = Join-Path $root "target-native\$target"
$env:RUSTFLAGS = (($env:RUSTFLAGS, "-C target-cpu=native -C link-arg=/Brepro") -join " ").Trim()
$env:SOURCE_DATE_EPOCH = (& git -C $root log -1 --format=%ct).Trim()
& cargo build --manifest-path (Join-Path $root "Cargo.toml") --locked --release --target $target
if ($LASTEXITCODE -ne 0) {
    throw "native build failed with exit code $LASTEXITCODE"
}
Write-Output (Join-Path $env:CARGO_TARGET_DIR "$target\release\keysteer.exe")
