# Builds an unsigned portable Windows x64 package.
# Needs Python on PATH: scripts\collect_licenses.py writes the Rust crate licences.
# Usage: powershell -ExecutionPolicy Bypass -File scripts\package.ps1
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root
if ($env:OS -ne 'Windows_NT') { throw 'Packaging requires Windows x64' }
$seenRuntimeNames = @()
$runtimeHashes = @{}
foreach ($line in Get-Content -LiteralPath 'vendor/conpty/checksums.sha256') {
    if ($line -notmatch '^([0-9a-f]{64})  (x64/(conpty\.dll|OpenConsole\.exe))$') {
        throw 'Invalid ConPTY checksum manifest'
    }
    $expectedHash = $Matches[1]
    $runtimeName = $Matches[2]
    if ($runtimeName -in $seenRuntimeNames) { throw 'Duplicate ConPTY runtime entry' }
    $seenRuntimeNames += $runtimeName
    $runtimeHashes[$runtimeName] = $expectedHash
    $runtimePath = Join-Path $root ('vendor/conpty/' + $runtimeName)
    if ((Get-FileHash -LiteralPath $runtimePath -Algorithm SHA256).Hash.ToLowerInvariant() -ne $expectedHash) {
        throw 'ConPTY runtime checksum mismatch'
    }
}
if ($seenRuntimeNames.Count -ne 2) { throw 'Incomplete ConPTY checksum manifest' }
& (Join-Path $PSScriptRoot 'check_conpty_signatures.ps1')

# Honour CARGO_TARGET_DIR / CARGO_BUILD_TARGET so the staged files always come
# from the build we just ran.
$targetDir = [IO.Path]::GetFullPath($(if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $root 'target' }))
$targetTriple = 'x86_64-pc-windows-msvc'
# Panic locations and debug info embed absolute source paths. Remap the checkout,
# cargo home, toolchain and generated-code tree so the archive does not carry
# the builder's directories, including a fresh target tree outside the checkout.
# Ask the linker for a content-derived timestamp (/Brepro). For this build the variable
# replaces RUSTFLAGS and any rustflags from the cargo configuration.
$sysroot = "$(rustc --print sysroot)".Trim()
if ($LASTEXITCODE -ne 0 -or -not $sysroot) { throw 'Cannot determine the Rust sysroot' }
$cargoHome = if ($env:CARGO_HOME) { $env:CARGO_HOME } else { Join-Path $HOME '.cargo' }
$previousRustflags = $env:CARGO_ENCODED_RUSTFLAGS
$env:CARGO_ENCODED_RUSTFLAGS = @("--remap-path-prefix=$root=/anvil", "--remap-path-prefix=$cargoHome=/cargo", "--remap-path-prefix=$sysroot=/rust", "--remap-path-prefix=$targetDir=/build", '-Clink-arg=/Brepro', '-Ctarget-feature=+crt-static') -join [char]0x1f
try {
    cargo build --locked --release --bin anvil --bin anvil-claude-status --target $targetTriple --target-dir $targetDir
} finally {
    $env:CARGO_ENCODED_RUSTFLAGS = $previousRustflags
}
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
# The ConPTY runtime comes from the hash-checked vendored copy, not from the
# build directory, where build.rs keeps the old files while ANVIL runs from there.
foreach ($runtimeName in $runtimeHashes.Keys) {
    Copy-Item (Join-Path $root ('vendor/conpty/' + $runtimeName)) $stage
    $staged = Join-Path $stage ($runtimeName -replace '^x64/', '')
    if ((Get-FileHash -LiteralPath $staged -Algorithm SHA256).Hash.ToLowerInvariant() -ne $runtimeHashes[$runtimeName]) {
        throw 'Staged ConPTY runtime checksum mismatch'
    }
}
Copy-Item (Join-Path $root 'LICENSE') $stage

# Licences of everything embedded in the binaries.
Copy-Item (Join-Path $root 'vendor\conpty\LICENSE') (Join-Path $stage 'LICENSES\conpty-MIT.txt')
Copy-Item (Join-Path $root 'fonts\OFL-notice.txt') (Join-Path $stage 'LICENSES\CascadiaMono-OFL-notice.txt')
Copy-Item (Join-Path $root 'fonts\OFL-1.1.txt') (Join-Path $stage 'LICENSES\OFL-1.1.txt')
Copy-Item (Join-Path $root 'fonts\seti-LICENSE.txt') (Join-Path $stage 'LICENSES\SetiUI-MIT.txt')
Copy-Item (Join-Path $root 'LICENSES\egui-default-fonts\*') (Join-Path $stage 'LICENSES\egui-default-fonts')
python -B (Join-Path $PSScriptRoot 'collect_licenses.py') $targetTriple (Join-Path $stage 'LICENSES\THIRD-PARTY-RUST.txt')
if ($LASTEXITCODE -ne 0) { throw 'Cannot collect the Rust crate licences' }

# GPL-3.0 section 6: point at the Corresponding Source for this exact build.
$commit = (git rev-parse HEAD).Trim()
if ($LASTEXITCODE -ne 0 -or $commit -notmatch '^[0-9a-f]{40}$') { throw 'Cannot determine source commit' }
# Tracked changes and untracked files that are not ignored both count (a source
# file Git has not seen still shapes the build); target/ and dist/ are ignored.
$dirty = [bool](git status --porcelain --untracked-files=normal)
if ($LASTEXITCODE -ne 0) { throw 'Cannot read the Git status' }
$sourceNotice = if ($dirty) {
    'The working tree had uncommitted changes, so the repository above does not reproduce this build.'
} else {
    'The complete source for this build is the repository above at this commit.'
}
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
    $sourceNotice
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
    source_dirty = $dirty
    target = $targetTriple
    files = $files
} | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $stage 'BUILD.json') -Encoding UTF8

$zip = Join-Path $dist "anvil-$version-x64.zip"
$temporaryZip = Join-Path $dist ('package-' + [guid]::NewGuid().ToString('N') + '.zip')
# Not Compress-Archive: Windows PowerShell 5.1 writes backslashes into member
# names. Sorted members with the commit time keep the archive itself stable.
Add-Type -AssemblyName System.IO.Compression, System.IO.Compression.FileSystem
$memberTime = [DateTimeOffset]::Parse("$(git show -s --format=%cI HEAD)")
$members = @($files.Keys) + 'BUILD.json'
[Array]::Sort($members, [StringComparer]::Ordinal)
$archive = [IO.Compression.ZipFile]::Open($temporaryZip, [IO.Compression.ZipArchiveMode]::Create)
try {
    foreach ($member in $members) {
        $entry = $archive.CreateEntry($member, [IO.Compression.CompressionLevel]::Optimal)
        $entry.LastWriteTime = $memberTime
        $reader = [IO.File]::OpenRead((Join-Path $stage $member))
        $writer = $entry.Open()
        try { $reader.CopyTo($writer) } finally { $writer.Dispose(); $reader.Dispose() }
    }
} finally {
    $archive.Dispose()
}
Move-Item -LiteralPath $temporaryZip -Destination $zip -Force
$zipHash = (Get-FileHash -LiteralPath $zip -Algorithm SHA256).Hash.ToLowerInvariant()
"$zipHash  $([IO.Path]::GetFileName($zip))" | Set-Content -LiteralPath (Join-Path $dist 'SHA256SUMS.txt') -Encoding ASCII
$resolvedStage = [IO.Path]::GetFullPath($stage)
if (-not $resolvedStage.StartsWith($distPrefix, [StringComparison]::OrdinalIgnoreCase)) { throw 'Unsafe cleanup path' }
Remove-Item -LiteralPath $resolvedStage -Recurse -Force

Write-Host "packed unsigned CI/development candidate: $zip"
Get-ChildItem $zip | Select-Object Name, Length
