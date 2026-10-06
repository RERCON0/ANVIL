# Builds an unsigned portable Windows x64 package with native Codex inference.
# Usage: powershell -ExecutionPolicy Bypass -File scripts\package.ps1
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root
if ($env:OS -ne 'Windows_NT') { throw 'Packaging requires Windows x64' }
$seenRuntimeNames = @()
foreach ($line in Get-Content -LiteralPath 'vendor/conpty/checksums.sha256') {
    if ($line -notmatch '^([0-9a-f]{64})  (x64/(conpty\.dll|OpenConsole\.exe))$') {
        throw 'Invalid ConPTY checksum manifest'
    }
    $expectedHash = $Matches[1]
    $runtimeName = $Matches[2]
    if ($runtimeName -in $seenRuntimeNames) { throw 'Duplicate ConPTY runtime entry' }
    $seenRuntimeNames += $runtimeName
    $runtimePath = Join-Path $root ('vendor/conpty/' + $runtimeName)
    if ((Get-FileHash -LiteralPath $runtimePath -Algorithm SHA256).Hash.ToLowerInvariant() -ne $expectedHash) {
        throw 'ConPTY runtime checksum mismatch'
    }
}
if ($seenRuntimeNames.Count -ne 2) { throw 'Incomplete ConPTY checksum manifest' }

# Honour CARGO_TARGET_DIR / CARGO_BUILD_TARGET so the staged files always come
# from the build we just ran.
$targetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $root 'target' }
$targetTriple = 'x86_64-pc-windows-msvc'
cargo build --locked --release --bin anvil --bin anvil-claude-status --features codex --target $targetTriple --target-dir $targetDir
if ($LASTEXITCODE -ne 0) { throw 'cargo build --release failed' }
$release = if ($targetTriple) { Join-Path $targetDir $targetTriple\release } else { Join-Path $targetDir 'release' }

$version = (Select-String -Path 'Cargo.toml' -Pattern '^version\s*=\s*"([^"]+)"').Matches[0].Groups[1].Value
$dist = Join-Path $root 'dist'
$stage = [IO.Path]::GetFullPath((Join-Path $dist ('stage-' + [guid]::NewGuid().ToString('N'))))
$distPrefix = [IO.Path]::GetFullPath($dist).TrimEnd([IO.Path]::DirectorySeparatorChar) + [IO.Path]::DirectorySeparatorChar
if (-not $stage.StartsWith($distPrefix, [StringComparison]::OrdinalIgnoreCase)) { throw 'Unsafe staging path' }
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
if ($LASTEXITCODE -ne 0 -or $commit -notmatch '^[0-9a-f]{40}$') { throw 'Cannot determine source commit' }
$remote = (git config --get remote.origin.url)
# A remote like https://user:token@host/repo carries credentials: the archive
# is for distribution, so only scheme, host and path go into it.
if ($remote) { $remote = $remote -replace '^([a-zA-Z][a-zA-Z0-9+.-]*://)[^/@]*@', '$1' }
if (-not $remote) { $remote = '<the project repository>' }
@(
    "ANVIL $version (GPL-3.0-or-later)"
    "Commit: $commit"
    'Features: codex'
    "Corresponding source: $remote"
    ""
    "The complete source for this build is the repository above at this commit."
    "Bundled third-party assets and their licences are listed in LICENSES/."
    'This development package is unsigned.'
) | Set-Content -Path (Join-Path $stage 'SOURCE.txt') -Encoding UTF8

$files = @{}
Get-ChildItem -LiteralPath $stage -File -Recurse | ForEach-Object {
    $relative = $_.FullName.Substring($stage.Length + 1).Replace('\', '/')
    $files[$relative] = @{
        sha256 = (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
        size = $_.Length
    }
}
@{
    schema = 1
    version = $version
    source_commit = $commit
    target = $targetTriple
    features = @('codex')
    files = $files
} | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $stage 'BUILD.json') -Encoding UTF8

$zip = Join-Path $dist "anvil-$version-x64.zip"
$temporaryZip = Join-Path $dist ('package-' + [guid]::NewGuid().ToString('N') + '.zip')
Compress-Archive -Path (Join-Path $stage '*') -DestinationPath $temporaryZip
Move-Item -LiteralPath $temporaryZip -Destination $zip -Force
$zipHash = (Get-FileHash -LiteralPath $zip -Algorithm SHA256).Hash.ToLowerInvariant()
"$zipHash  $([IO.Path]::GetFileName($zip))" | Set-Content -LiteralPath (Join-Path $dist 'SHA256SUMS.txt') -Encoding ASCII
$resolvedStage = [IO.Path]::GetFullPath($stage)
if (-not $resolvedStage.StartsWith($distPrefix, [StringComparison]::OrdinalIgnoreCase)) { throw 'Unsafe cleanup path' }
Remove-Item -LiteralPath $resolvedStage -Recurse -Force

Write-Host "packed unsigned CI/development candidate: $zip"
Get-ChildItem $zip | Select-Object Name, Length
