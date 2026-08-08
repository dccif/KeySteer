[CmdletBinding()]
param(
    [Parameter(Position = 0)]
    [ValidateSet("x86_64-pc-windows-msvc", "aarch64-pc-windows-msvc")]
    [string] $Target,

    # Cross-compiled executables cannot be launched on the x64 Actions runner.
    [switch] $SkipConfigCheck
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

if (-not $Target) {
    $Target = if ($env:PROCESSOR_ARCHITECTURE -eq "ARM64") {
        "aarch64-pc-windows-msvc"
    } else {
        "x86_64-pc-windows-msvc"
    }
}

$projectRoot = (Resolve-Path (Join-Path $PSScriptRoot "..\..")).Path
$manifest = Join-Path $projectRoot "Cargo.toml"
$version = $null
foreach ($line in Get-Content -LiteralPath $manifest) {
    if ($line -match '^version\s*=\s*"([^"]+)"') {
        $version = $Matches[1]
        break
    }
}
if ([string]::IsNullOrWhiteSpace($version)) {
    throw "cannot read package version from: $manifest"
}

Push-Location $projectRoot
try {
    & cargo build --locked --release --target $Target
    if ($LASTEXITCODE -ne 0) {
        throw "cargo build failed with exit code $LASTEXITCODE"
    }
}
finally {
    Pop-Location
}

$binary = Join-Path $projectRoot "target\$Target\release\keysteer.exe"
if (-not (Test-Path -LiteralPath $binary -PathType Leaf)) {
    throw "release executable was not produced: $binary"
}
$defaultConfig = Join-Path $projectRoot "keysteer.default.toml"
if (-not (Test-Path -LiteralPath $defaultConfig -PathType Leaf)) {
    throw "shipped default configuration is missing: $defaultConfig"
}

# Catch stale target-specific artifacts and config/schema drift before they can
# be copied into a portable archive. `--check` only parses and validates; it
# does not start the backend or request operating-system permissions.
if (-not $SkipConfigCheck) {
    & $binary --config $defaultConfig --check
    if ($LASTEXITCODE -ne 0) {
        throw "release executable rejected the shipped configuration (exit code $LASTEXITCODE)"
    }
}
else {
    Write-Verbose "Skipping target executable config check for cross compilation"
}

$dist = Join-Path $projectRoot "dist\$Target"
$payload = Join-Path $dist "KeySteer"
$archive = Join-Path $dist "KeySteer-v$version-$Target.zip"
$checksum = "$archive.sha256"
$legacyArchive = Join-Path $dist "KeySteer-$Target.zip"
$legacyChecksum = "$legacyArchive.sha256"

New-Item -ItemType Directory -Force -Path $dist | Out-Null
if (Test-Path -LiteralPath $payload) {
    Remove-Item -LiteralPath $payload -Recurse -Force
}
foreach ($path in @($archive, $checksum, $legacyArchive, $legacyChecksum)) {
    if (Test-Path -LiteralPath $path) {
        Remove-Item -LiteralPath $path -Force
    }
}

New-Item -ItemType Directory -Path $payload | Out-Null
Copy-Item -LiteralPath $binary -Destination (Join-Path $payload "KeySteer.exe")
Copy-Item -LiteralPath $defaultConfig -Destination (Join-Path $payload "keysteer.default.toml")
Compress-Archive -LiteralPath $payload `
    -DestinationPath $archive -CompressionLevel Optimal

$stream = [System.IO.File]::OpenRead($archive)
try {
    $sha256 = [System.Security.Cryptography.SHA256]::Create()
    try {
        $hashBytes = $sha256.ComputeHash($stream)
    }
    finally {
        $sha256.Dispose()
    }
}
finally {
    $stream.Dispose()
}
$hash = ([System.BitConverter]::ToString($hashBytes) -replace "-", "").ToLowerInvariant()
$line = "$hash  $(Split-Path -Leaf $archive)"
[System.IO.File]::WriteAllText($checksum, "$line`n", [System.Text.Encoding]::ASCII)

Write-Output $archive
Write-Output $checksum
