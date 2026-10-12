# Fetches libmpv for the desktop app into vendor/mpv/ (git-ignored).
#
# Pinned to one shinchiro/mpv-winbuild-cmake build and checked against its SHA-256, so every dev
# machine runs the same mpv. Those builds are daily and old ones get pruned: when the download
# 404s, pick a newer release, update $Tag/$Asset/$Sha256 together, and rerun.
#
# Usage: pwsh scripts/fetch-libmpv.ps1 [-Force]

param([switch]$Force)
$ErrorActionPreference = 'Stop'

$Tag    = '20261011'
$Asset  = 'mpv-dev-x86_64-20261011-git-accb7b2821.7z'
$Sha256 = 'cf63b4ce66e03e5e70acf2648cbc3177656c77c5b9583e16d83efc58801182b7'
$Url    = "https://github.com/shinchiro/mpv-winbuild-cmake/releases/download/$Tag/$Asset"

$root = Split-Path -Parent $PSScriptRoot
$dest = Join-Path $root 'vendor/mpv'
$dll  = Join-Path $dest 'libmpv-2.dll'

if ((Test-Path $dll) -and -not $Force) {
    Write-Host "libmpv already present: $dll (use -Force to fetch again)"
    exit 0
}

New-Item -ItemType Directory -Force $dest | Out-Null
$archive = Join-Path $dest $Asset
Write-Host "downloading $Url"
Invoke-WebRequest -Uri $Url -OutFile $archive -UseBasicParsing

$actual = (Get-FileHash -Algorithm SHA256 $archive).Hash.ToLowerInvariant()
if ($actual -ne $Sha256) {
    Remove-Item $archive
    throw "SHA-256 mismatch for $Asset`: expected $Sha256, got $actual"
}

# Windows' own tar (bsdtar/libarchive) reads 7z, so no 7-Zip install is needed.
$tar = Join-Path $env:SystemRoot 'System32/tar.exe'
& $tar -xf $archive -C $dest libmpv-2.dll
if ($LASTEXITCODE -ne 0) { throw "tar could not extract libmpv-2.dll from $Asset" }
Remove-Item $archive

Set-Content -Path (Join-Path $dest 'VERSION') -Value "$Asset $Sha256"
Write-Host "libmpv ready: $dll"
