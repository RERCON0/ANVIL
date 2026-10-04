# Builds the release binaries and packs the portable distribution zip.
# Usage: powershell -ExecutionPolicy Bypass -File scripts\package.ps1
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

# Honour CARGO_TARGET_DIR / CARGO_BUILD_TARGET so the staged files always come
# from the build we just ran.
$targetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $root 'target' }
$targetTriple = if ($env:CARGO_BUILD_TARGET) { $env:CARGO_BUILD_TARGET } else { $null }
cargo build --release --target-dir $targetDir
if ($LASTEXITCODE -ne 0) { throw 'cargo build --release failed' }
$release = if ($targetTriple) { Join-Path $targetDir $targetTriple\release } else { Join-Path $targetDir 'release' }

$version = (Select-String -Path 'Cargo.toml' -Pattern '^version\s*=\s*"([^"]+)"').Matches[0].Groups[1].Value
$dist = Join-Path $root 'dist'
$stage = Join-Path $dist 'stage'
if (Test-Path $stage) { Remove-Item -Recurse -Force $stage }
New-Item -ItemType Directory -Force -Path $stage | Out-Null
New-Item -ItemType Directory -Force -Path (Join-Path $stage 'LICENSES\egui-default-fonts') | Out-Null

Copy-Item (Join-Path $release 'anvil.exe') $stage
Copy-Item (Join-Path $release 'anvil-claude-status.exe') $stage
Copy-Item (Join-Path $release 'conpty.dll') $stage
Copy-Item (Join-Path $release 'OpenConsole.exe') $stage
Copy-Item (Join-Path $root 'LICENSE') $stage

# Licences of everything embedded in the binaries.
Copy-Item (Join-Path $root 'vendor\conpty\LICENSE') (Join-Path $stage 'LICENSES\conpty-MIT.txt')
Copy-Item (Join-Path $root 'fonts\OFL-notice.txt') (Join-Path $stage 'LICENSES\CascadiaMono-OFL-notice.txt')
Copy-Item (Join-Path $root 'fonts\OFL-1.1.txt') (Join-Path $stage 'LICENSES\OFL-1.1.txt')
Copy-Item (Join-Path $root 'fonts\seti-LICENSE.txt') (Join-Path $stage 'LICENSES\SetiUI-MIT.txt')
Copy-Item (Join-Path $root 'LICENSES\egui-default-fonts\*') (Join-Path $stage 'LICENSES\egui-default-fonts')

# GPL-3.0 section 6: point at the Corresponding Source for this exact build.
$commit = (git rev-parse HEAD).Trim()
$remote = (git config --get remote.origin.url)
# A remote like https://user:token@host/repo carries credentials: the archive
# is for distribution, so only scheme, host and path go into it.
if ($remote) { $remote = $remote -replace '^([a-zA-Z][a-zA-Z0-9+.-]*://)[^/@]*@', '$1' }
if (-not $remote) { $remote = '<the project repository>' }
@(
    "ANVIL $version (GPL-3.0-or-later)"
    "Commit: $commit"
    "Corresponding source: $remote"
    ""
    "The complete source for this build is the repository above at this commit."
    "Bundled third-party assets and their licences are listed in LICENSES/."
) | Set-Content -Path (Join-Path $stage 'SOURCE.txt') -Encoding UTF8

$zip = Join-Path $dist "anvil-$version-x64.zip"
if (Test-Path $zip) { Remove-Item -Force $zip }
Compress-Archive -Path (Join-Path $stage '*') -DestinationPath $zip
Remove-Item -Recurse -Force $stage

Write-Host "packed $zip"
Get-ChildItem $zip | Select-Object Name, Length
