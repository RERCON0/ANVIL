# Measures the acceptance numbers of stage 1. Results belong in docs/BENCH.md.
# Usage: powershell -ExecutionPolicy Bypass -File scripts\bench.ps1
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

$log = Join-Path $env:APPDATA 'anvil\anvil.log'
$exe = Join-Path $root 'target\release\anvil.exe'
if (-not (Test-Path $exe)) { throw "build the release binary first: cargo build --release" }

function Get-AnvilProcesses { @(Get-Process anvil -ErrorAction SilentlyContinue) }

$before = Get-AnvilProcesses
$start = Get-Date
Start-Process -FilePath $exe
$deadline = $start.AddSeconds(15)
$windowAt = $null
while ((Get-Date) -lt $deadline) {
    $proc = Get-AnvilProcesses | Where-Object { $_.MainWindowHandle -ne 0 -and $before.Id -notcontains $_.Id } | Select-Object -First 1
    if ($proc) { $windowAt = Get-Date; $pid_ = $proc.Id; break }
    Start-Sleep -Milliseconds 20
}
if (-not $windowAt) { throw 'the ANVIL window did not appear within 15 s' }
$startupMs = [int]($windowAt - $start).TotalMilliseconds

# The first frame mark from the log (main -> first rendered frame).
$firstFrame = Select-String -Path $log -Pattern 'first frame in (\d+) ms' | Select-Object -Last 1
$firstFrameMs = if ($firstFrame) { [int]$firstFrame.Matches[0].Groups[1].Value } else { $null }

$proc = Get-Process -Id $pid_
$workingSetMb = [math]::Round($proc.WorkingSet64 / 1MB, 1)
$cpuBefore = $proc.TotalProcessorTime
Start-Sleep -Seconds 30
$proc.Refresh()
$idleCpu = [math]::Round(($proc.TotalProcessorTime - $cpuBefore).TotalSeconds / 30 * 100, 2)

Write-Host "cold start to visible window : $startupMs ms"
if ($firstFrameMs) { Write-Host "first frame (log)            : $firstFrameMs ms" }
Write-Host "working set                  : $workingSetMb MB"
Write-Host "idle CPU over 30 s           : $idleCpu %"
Write-Host ''
Write-Host 'Manual steps for the rest (record them in docs/BENCH.md):'
Write-Host '  * open 3 tabs with 3 panes each (Ctrl+Shift+T, Ctrl+Shift+S, Ctrl+Shift+D)'
Write-Host '    and re-run: (Get-Process anvil).WorkingSet64 / 1MB'
Write-Host '  * in a pane run: time seq 1 2000000   (must finish < 5 s, window responsive)'
Write-Host "  * close ANVIL; process id was $pid_"
