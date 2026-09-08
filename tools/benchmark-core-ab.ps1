param(
    [Parameter(Mandatory)][string]$BaselineExecutable,
    [Parameter(Mandatory)][string]$CandidateExecutable,
    [Parameter(Mandatory)][string]$OutputDirectory,
    [ValidateRange(1, 20)][int]$Rounds = 3,
    [ValidateSet('All', 'Runtime', 'SmallHint')][string]$Mode = 'All',
    [long]$Affinity = 4
)

$ErrorActionPreference = 'Stop'
$taskBins = @{
    baseline = (Resolve-Path -LiteralPath $BaselineExecutable).Path
    candidate = (Resolve-Path -LiteralPath $CandidateExecutable).Path
}
New-Item -ItemType Directory -Force $OutputDirectory | Out-Null
$taskOutput = (Resolve-Path -LiteralPath $OutputDirectory).Path
$taskProcess = [Diagnostics.Process]::GetCurrentProcess()
$taskProcess.ProcessorAffinity = [IntPtr]$Affinity
$taskProcess.PriorityClass = [Diagnostics.ProcessPriorityClass]::AboveNormal
@{
    utc = [DateTime]::UtcNow.ToString('o')
    processor = (Get-ItemProperty 'HKLM:/HARDWARE/DESCRIPTION/System/CentralProcessor/0' -Name ProcessorNameString).ProcessorNameString.Trim()
    logical_processors = [Environment]::ProcessorCount
    affinity = $Affinity
    runner_priority = $taskProcess.PriorityClass.ToString()
    # Windows inherits affinity here, but not AboveNormal priority.
    benchmark_priority = 'Normal'
    mode = $Mode
    binaries = @($taskBins.GetEnumerator() | ForEach-Object {
        @{ name = $_.Key; path = $_.Value; sha256 = (Get-FileHash -LiteralPath $_.Value -Algorithm SHA256).Hash }
    })
} | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $taskOutput 'environment.json')

for ($round = 1; $round -le $Rounds; $round++) {
    $order = if ($round % 2 -eq 1) { @('baseline', 'candidate') } else { @('candidate', 'baseline') }
    foreach ($name in $order) {
        Write-Output "round=$round version=$name"
        if ($Mode -ne 'Runtime') {
            if ($Mode -eq 'SmallHint') {
                & $taskBins[$name] --hint-small | Tee-Object -FilePath (Join-Path $taskOutput "$round-$name-core.txt")
            } else {
                & $taskBins[$name] | Tee-Object -FilePath (Join-Path $taskOutput "$round-$name-core.txt")
            }
            if ($LASTEXITCODE -ne 0) { throw "core benchmark failed: $name" }
        }
        if ($Mode -ne 'SmallHint') {
            & $taskBins[$name] --runtime | Tee-Object -FilePath (Join-Path $taskOutput "$round-$name-runtime.txt")
            if ($LASTEXITCODE -ne 0) { throw "runtime benchmark failed: $name" }
        }
    }
}
