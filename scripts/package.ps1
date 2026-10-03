# Builds the release binaries and packs the portable distribution zip.
# Usage: powershell -ExecutionPolicy Bypass -File scripts\package.ps1
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

cargo build --release
if ($LASTEXITCODE -ne 0) { throw 'cargo build --release failed' }

$version = (Select-String -Path 'Cargo.toml' -Pattern '^version\s*=\s*"([^"]+)"').Matches[0].Groups[1].Value
$dist = Join-Path $root 'dist'
$stage = Join-Path $dist 'stage'
if (Test-Path $stage) { Remove-Item -Recurse -Force $stage }
New-Item -ItemType Directory -Force -Path $stage | Out-Null
New-Item -ItemType Directory -Force -Path (Join-Path $stage 'LICENSES') | Out-Null

$release = Join-Path $root 'target\release'
Copy-Item (Join-Path $release 'anvil.exe') $stage
Copy-Item (Join-Path $release 'anvil-claude-status.exe') $stage
Copy-Item (Join-Path $release 'conpty.dll') $stage
Copy-Item (Join-Path $release 'OpenConsole.exe') $stage
Copy-Item (Join-Path $root 'LICENSE') $stage
Copy-Item (Join-Path $root 'vendor\conpty\LICENSE') (Join-Path $stage 'LICENSES\conpty-MIT.txt')
Copy-Item (Join-Path $root 'fonts\OFL-notice.txt') (Join-Path $stage 'LICENSES\CascadiaMono-OFL.txt')

$zip = Join-Path $dist "anvil-$version-x64.zip"
if (Test-Path $zip) { Remove-Item -Force $zip }
Compress-Archive -Path (Join-Path $stage '*') -DestinationPath $zip
Remove-Item -Recurse -Force $stage

Write-Host "packed $zip"
Get-ChildItem $zip | Select-Object Name, Length
