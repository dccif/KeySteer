[CmdletBinding()]
param(
    [Parameter(Position = 0)]
    [ValidateSet("x86_64-pc-windows-msvc", "aarch64-pc-windows-msvc")]
    [string] $Target,

    # Cross-compiled executables cannot be launched on the x64 Actions runner.
    [switch] $SkipConfigCheck,

    # Authenticode is required for one-click automatic installation. Use a
    # PFX path/password or a CurrentUser\My certificate thumbprint.
    [string] $SigningPfx = $env:KEYSTEER_SIGNING_PFX,
    [string] $SigningPassword = $env:KEYSTEER_SIGNING_PASSWORD,
    [string] $SigningThumbprint = $env:KEYSTEER_SIGNING_THUMBPRINT,
    [string] $TimestampUrl = $env:KEYSTEER_TIMESTAMP_URL,
    [switch] $RequireSigning,
    [switch] $SkipTimestamp,
    [switch] $TrustSelfSignedForBuild,
    [string] $OutputRoot
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
if ([string]::IsNullOrWhiteSpace($OutputRoot)) {
    $resolvedOutputRoot = Join-Path $projectRoot "dist"
}
elseif ([IO.Path]::IsPathRooted($OutputRoot)) {
    $resolvedOutputRoot = [IO.Path]::GetFullPath($OutputRoot)
}
else {
    $resolvedOutputRoot = [IO.Path]::GetFullPath((Join-Path $projectRoot $OutputRoot))
}
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
    if (-not $env:SOURCE_DATE_EPOCH) {
        $env:SOURCE_DATE_EPOCH = (& git log -1 --format=%ct).Trim()
        if ($LASTEXITCODE -ne 0 -or -not $env:SOURCE_DATE_EPOCH) {
            throw "cannot derive SOURCE_DATE_EPOCH from the checked-out commit"
        }
    }
    $env:RUSTFLAGS = (($env:RUSTFLAGS, "-C link-arg=/Brepro") -join " ").Trim()

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

function Find-SignTool {
    $command = Get-Command "signtool.exe" -ErrorAction SilentlyContinue
    if ($command) {
        return $command.Source
    }
    $kits = Join-Path ${env:ProgramFiles(x86)} "Windows Kits\10\bin"
    if (Test-Path -LiteralPath $kits -PathType Container) {
        $candidate = Get-ChildItem -LiteralPath $kits -Directory |
            Sort-Object Name -Descending |
            ForEach-Object { Join-Path $_.FullName "x64\signtool.exe" } |
            Where-Object { Test-Path -LiteralPath $_ -PathType Leaf } |
            Select-Object -First 1
        if ($candidate) {
            return $candidate
        }
    }
    throw "signtool.exe was not found; install the Windows SDK or add it to PATH"
}

$signingRequested = -not [string]::IsNullOrWhiteSpace($SigningPfx) -or
    -not [string]::IsNullOrWhiteSpace($SigningThumbprint)
if ($RequireSigning -and -not $signingRequested) {
    throw "release signing is required, but no PFX or certificate thumbprint was supplied"
}
if ($signingRequested) {
    if (-not [string]::IsNullOrWhiteSpace($SigningPfx) -and
        -not [string]::IsNullOrWhiteSpace($SigningThumbprint)) {
        throw "specify either SigningPfx or SigningThumbprint, not both"
    }
    $signTool = Find-SignTool
    $signArgs = @("sign", "/fd", "SHA256", "/d", "KeySteer")
    if (-not $SkipTimestamp) {
        if ([string]::IsNullOrWhiteSpace($TimestampUrl)) {
            $TimestampUrl = "http://timestamp.digicert.com"
        }
        $signArgs += @("/tr", $TimestampUrl, "/td", "SHA256")
    }
    if (-not [string]::IsNullOrWhiteSpace($SigningPfx)) {
        $resolvedPfx = (Resolve-Path -LiteralPath $SigningPfx).Path
        $signArgs += @("/f", $resolvedPfx)
        if (-not [string]::IsNullOrWhiteSpace($SigningPassword)) {
            $signArgs += @("/p", $SigningPassword)
        }
    }
    else {
        $normalizedThumbprint = $SigningThumbprint -replace '\s', ''
        $signArgs += @("/sha1", $normalizedThumbprint)
    }
    & $signTool @signArgs $binary
    if ($LASTEXITCODE -ne 0) {
        throw "Authenticode signing failed with exit code $LASTEXITCODE"
    }
    $temporaryRootAdded = $false
    $temporaryCertificate = $null
    try {
        if ($TrustSelfSignedForBuild) {
            Import-Module Microsoft.PowerShell.Security
            Import-Module PKI
            if (-not [string]::IsNullOrWhiteSpace($SigningPfx)) {
                if ([string]::IsNullOrWhiteSpace($SigningPassword)) {
                    $pfxData = Get-PfxData -FilePath $resolvedPfx
                }
                else {
                    $securePassword = ConvertTo-SecureString `
                        -String $SigningPassword `
                        -AsPlainText `
                        -Force
                    $pfxData = Get-PfxData -FilePath $resolvedPfx -Password $securePassword
                }
                $endCertificates = @($pfxData.EndEntityCertificates)
                if ($endCertificates.Count -ne 1) {
                    throw "the signing PFX must contain exactly one end-entity certificate"
                }
                $signer = $endCertificates[0]
            }
            else {
                $signerPath = "Cert:\CurrentUser\My\$normalizedThumbprint"
                $signer = Get-Item -LiteralPath $signerPath -ErrorAction Stop
            }
            if ($signer.Subject -eq $signer.Issuer) {
                $rootPath = "Cert:\CurrentUser\Root\$($signer.Thumbprint)"
                if (-not (Test-Path -LiteralPath $rootPath)) {
                    $temporaryCertificate = [IO.Path]::GetTempFileName()
                    [IO.File]::WriteAllBytes(
                        $temporaryCertificate,
                        $signer.Export([Security.Cryptography.X509Certificates.X509ContentType]::Cert)
                    )
                    Import-Certificate `
                        -FilePath $temporaryCertificate `
                        -CertStoreLocation "Cert:\CurrentUser\Root" | Out-Null
                    $temporaryRootAdded = $true
                }
            }
        }
        & $signTool verify /pa /all $binary
        if ($LASTEXITCODE -ne 0) {
            throw "Authenticode verification failed with exit code $LASTEXITCODE"
        }
    }
    finally {
        if ($temporaryRootAdded) {
            $rootPath = "Cert:\CurrentUser\Root\$($signer.Thumbprint)"
            if (Test-Path -LiteralPath $rootPath) {
                Remove-Item -LiteralPath $rootPath -Force
            }
        }
        if ($temporaryCertificate -and (Test-Path -LiteralPath $temporaryCertificate)) {
            Remove-Item -LiteralPath $temporaryCertificate -Force
        }
    }
}
else {
    Write-Warning "Building unsigned artifacts; one-click automatic installation will be disabled"
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

$dist = Join-Path $resolvedOutputRoot $Target
$payload = Join-Path $dist "KeySteer"
$archive = Join-Path $dist "KeySteer-v$version-$Target.zip"
$updateExecutable = Join-Path $dist "KeySteer-v$version-$Target.exe"
$legacyArchive = Join-Path $dist "KeySteer-$Target.zip"
$staleChecksums = @("$archive.sha256", "$legacyArchive.sha256")

New-Item -ItemType Directory -Force -Path $dist | Out-Null
if (Test-Path -LiteralPath $payload) {
    Remove-Item -LiteralPath $payload -Recurse -Force
}
foreach ($path in @($archive, $updateExecutable, $legacyArchive) + $staleChecksums) {
    if (Test-Path -LiteralPath $path) {
        Remove-Item -LiteralPath $path -Force
    }
}

New-Item -ItemType Directory -Path $payload | Out-Null
Copy-Item -LiteralPath $binary -Destination (Join-Path $payload "KeySteer.exe")
Copy-Item -LiteralPath $binary -Destination $updateExecutable
Copy-Item -LiteralPath $defaultConfig -Destination (Join-Path $payload "keysteer.default.toml")
Compress-Archive -LiteralPath $payload `
    -DestinationPath $archive -CompressionLevel Optimal

Write-Output $archive
Write-Output $updateExecutable
