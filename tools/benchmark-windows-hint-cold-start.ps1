[CmdletBinding()]
param(
    [Parameter(Mandatory)] [string] $Executable,
    [Parameter(Mandatory)] [string] $ConfigPath,
    [int[]] $OffsetsMs = @(0, 10, 50, 100, 500),
    [int] $Samples = 20,
    [int] $TimeoutSeconds = 15,
    [string] $OutputPath
)

$ErrorActionPreference = "Stop"
$root = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$exe = (Resolve-Path -LiteralPath $Executable).Path
$config = (Resolve-Path -LiteralPath $ConfigPath).Path
$probeRoot = Join-Path $root "target\benchmarks\hint-cold-start"
New-Item -ItemType Directory -Force -Path $probeRoot | Out-Null
if (-not $OutputPath) { $OutputPath = Join-Path $probeRoot "results.json" }

Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
public static class KeySteerColdStartInput {
    [DllImport("user32.dll")]
    private static extern void keybd_event(byte key, byte scan, uint flags, UIntPtr extra);
    public static void ControlChord(byte key) {
        keybd_event(0x11, 0, 0, UIntPtr.Zero);
        keybd_event(key, 0, 0, UIntPtr.Zero);
        keybd_event(key, 0, 2, UIntPtr.Zero);
        keybd_event(0x11, 0, 2, UIntPtr.Zero);
    }
}
"@

function Read-Records([string] $Path) {
    if (-not (Test-Path -LiteralPath $Path)) { return @() }
    @(Get-Content -LiteralPath $Path | ForEach-Object {
        try { $_ | ConvertFrom-Json } catch { $null }
    } | Where-Object { $null -ne $_ })
}

function Wait-Offset([Diagnostics.Stopwatch] $Watch, [int] $Offset) {
    $coarse = $Offset - [int]$Watch.Elapsed.TotalMilliseconds - 2
    if ($coarse -gt 0) { [Threading.Thread]::Sleep($coarse) }
    while ($Watch.Elapsed.TotalMilliseconds -lt $Offset) { [Threading.Thread]::SpinWait(128) }
}

$results = [Collections.Generic.List[object]]::new()
foreach ($offset in $OffsetsMs) {
    for ($sample = 0; $sample -lt $Samples; $sample++) {
        $probe = Join-Path $probeRoot "$offset-$sample.jsonl"
        if (Test-Path -LiteralPath $probe) { Remove-Item -LiteralPath $probe -Force }
        $previousProbe = $env:KEYSTEER_PERF_PROBE
        $env:KEYSTEER_PERF_PROBE = $probe
        $watch = [Diagnostics.Stopwatch]::StartNew()
        $process = Start-Process -FilePath $exe -ArgumentList @("--config", $config) -PassThru -WindowStyle Hidden
        try {
            Wait-Offset $watch $offset
            $before = Read-Records $probe
            $readiness = [ordered]@{}
            foreach ($event in @("hook_ready", "uia_ready", "ocr_ready", "renderer_ready")) {
                $readiness[$event] = $null -ne ($before | Where-Object event -eq $event | Select-Object -First 1)
            }
            # The shipped bindings enter normal with Primary+E and Hint with Primary+F.
            [KeySteerColdStartInput]::ControlChord(0x45)
            [KeySteerColdStartInput]::ControlChord(0x46)
            $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
            $records = @()
            $scan = $null
            $present = $null
            while ([DateTime]::UtcNow -lt $deadline -and -not $process.HasExited) {
                $records = Read-Records $probe
                $scan = $records | Where-Object event -eq "scan_requested" | Select-Object -First 1
                if ($null -ne $scan) {
                    $present = $records | Where-Object { $_.event -eq "native_presented" -and $_.sequence -gt $scan.sequence } | Select-Object -First 1
                }
                if ($null -ne $present) { break }
                Start-Sleep -Milliseconds 5
                $process.Refresh()
            }
            $hook = if ($null -eq $scan) { $null } else {
                $records | Where-Object { $_.event -eq "hook_received" -and $_.value -eq 0x46 -and $_.sequence -lt $scan.sequence } | Select-Object -Last 1
            }
            $dropped = $records | Where-Object { $_.event -eq "probe_dropped" -and $_.value -gt 0 } | Select-Object -Last 1
            $valid = $null -ne $hook -and $null -ne $present -and $null -eq $dropped
            $results.Add([ordered]@{
                offset_ms = $offset
                sample = $sample
                valid = $valid
                readiness = $readiness
                correlation_id = if ($null -eq $hook) { $null } else { $hook.correlation_id }
                input_to_native_present_ms = if (-not $valid) { $null } else { ([double]$present.elapsed_ns - [double]$hook.elapsed_ns) / 1e6 }
                status = if ($null -ne $dropped) { "probe_dropped" } elseif ($null -eq $scan) { "scan_timeout" } elseif ($null -eq $hook) { "missing_hook_marker" } elseif ($null -eq $present) { "present_timeout" } else { "ok" }
            })
        }
        finally {
            if (-not $process.HasExited) { Stop-Process -Id $process.Id }
            $env:KEYSTEER_PERF_PROBE = $previousProbe
        }
    }
}

$summary = @($results | Group-Object offset_ms | ForEach-Object {
    $values = @($_.Group | Where-Object valid | ForEach-Object input_to_native_present_ms | Sort-Object)
    [ordered]@{
        offset_ms = [int]$_.Name
        valid = $values.Count
        invalid = $_.Count - $values.Count
        p50_ms = if ($values.Count) { $values[[math]::Floor(($values.Count - 1) * 0.50)] } else { $null }
        p95_ms = if ($values.Count) { $values[[math]::Floor(($values.Count - 1) * 0.95)] } else { $null }
        p99_ms = if ($values.Count) { $values[[math]::Floor(($values.Count - 1) * 0.99)] } else { $null }
    }
})
[ordered]@{ metric = "physical_hook_to_first_native_present"; samples = $results; summary = $summary } |
    ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $OutputPath -Encoding utf8
Write-Output $OutputPath
