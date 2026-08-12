[CmdletBinding()]
param(
    [ValidateSet("x86_64-pc-windows-msvc", "aarch64-pc-windows-msvc")]
    [string] $Target = "x86_64-pc-windows-msvc",
    [string] $Executable,
    [string] $ConfigPath,
    [int] $ColdStarts = 30,
    [int] $ResidentSeconds = 30,
    [switch] $UsePerfProbe,
    [int] $ProbeTimeoutSeconds = 15,
    [switch] $KeepRunning
)

$ErrorActionPreference = "Stop"
$root = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$exe = if ($Executable) {
    (Resolve-Path -LiteralPath $Executable).Path
} else {
    Join-Path $root "dist\$Target\KeySteer\KeySteer.exe"
}
$config = if ($ConfigPath) {
    (Resolve-Path -LiteralPath $ConfigPath).Path
} elseif ($Executable) {
    Join-Path $root "keysteer.default.toml"
} else {
    Join-Path $root "dist\$Target\KeySteer\keysteer.default.toml"
}
if (-not (Test-Path -LiteralPath $exe -PathType Leaf)) { throw "missing $exe" }
if (-not (Test-Path -LiteralPath $config -PathType Leaf)) { throw "missing $config" }

$cold = [System.Collections.Generic.List[double]]::new()
$probeRoot = Join-Path $root "target\benchmarks\perf-probe"
if ($UsePerfProbe) { New-Item -ItemType Directory -Force -Path $probeRoot | Out-Null }
for ($index = 0; $index -lt $ColdStarts; $index++) {
    if ($UsePerfProbe) {
        $probePath = Join-Path $probeRoot "ready-$index.jsonl"
        $previousProbe = $env:KEYSTEER_PERF_PROBE
        $env:KEYSTEER_PERF_PROBE = $probePath
        $process = Start-Process -FilePath $exe -ArgumentList @("--config", $config) -PassThru -WindowStyle Hidden
        try {
            $deadline = [DateTime]::UtcNow.AddSeconds($ProbeTimeoutSeconds)
            $ready = $null
            while ([DateTime]::UtcNow -lt $deadline -and -not $process.HasExited) {
                if (Test-Path -LiteralPath $probePath) {
                    $ready = Get-Content -LiteralPath $probePath |
                        ForEach-Object { $_ | ConvertFrom-Json } |
                        Where-Object event -eq "backend_started" |
                        Select-Object -Last 1
                    if ($null -ne $ready) { break }
                }
                Start-Sleep -Milliseconds 5
                $process.Refresh()
            }
            if ($null -eq $ready) { throw "perf-probe did not report backend_started within $ProbeTimeoutSeconds seconds" }
            $cold.Add([double]$ready.elapsed_ns / 1e6)
        }
        finally {
            if (-not $process.HasExited) { Stop-Process -Id $process.Id }
            $env:KEYSTEER_PERF_PROBE = $previousProbe
        }
    }
    else {
        $watch = [Diagnostics.Stopwatch]::StartNew()
        $process = Start-Process -FilePath $exe -ArgumentList @("--config", $config, "--check") -PassThru -WindowStyle Hidden
        $process.WaitForExit()
        $watch.Stop()
        if ($process.ExitCode -ne 0) { throw "config probe failed with $($process.ExitCode)" }
        $cold.Add($watch.Elapsed.TotalMilliseconds)
    }
}

$resident = Start-Process -FilePath $exe -ArgumentList @("--config", $config) -PassThru -WindowStyle Hidden
try {
    $samples = [System.Collections.Generic.List[object]]::new()
    for ($second = 1; $second -le $ResidentSeconds; $second++) {
        Start-Sleep -Seconds 1
        $resident.Refresh()
        $samples.Add([ordered]@{
            second = $second
            working_set = $resident.WorkingSet64
            private_bytes = $resident.PrivateMemorySize64
            handles = $resident.HandleCount
            threads = $resident.Threads.Count
        })
    }
    $ordered = @($cold | Sort-Object)
    $result = [ordered]@{
        target = $Target
        startup_metric = $(if ($UsePerfProbe) { "backend_started_ms" } else { "config_check_process_ms" })
        startup_ms = [ordered]@{
            p50 = $ordered[[math]::Floor(($ordered.Count - 1) * 0.50)]
            p95 = $ordered[[math]::Floor(($ordered.Count - 1) * 0.95)]
            p99 = $ordered[[math]::Floor(($ordered.Count - 1) * 0.99)]
        }
        resident = $samples
    }
    $output = Join-Path $root "target\benchmarks\windows-$Target.json"
    New-Item -ItemType Directory -Force -Path (Split-Path $output) | Out-Null
    $result | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath $output -Encoding utf8
    Write-Output $output
}
finally {
    if (-not $KeepRunning -and -not $resident.HasExited) { Stop-Process -Id $resident.Id }
}
