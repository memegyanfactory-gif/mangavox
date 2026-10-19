#Requires -Version 5.0
<#
.SYNOPSIS
  Builds MangaVox release artifacts: payload zip, installer, SHA256SUMS.
.PARAMETER Version
  Defaults to [workspace.package] version in native/Cargo.toml.
.PARAMETER SkipBuild
  Skip `cargo build --release -p mangavox` (uses existing exe).
.PARAMETER DryRun
  Stage python/ + libmpv only (no exe, no zip, no installer).
#>
param(
    [string]$Version,
    [switch]$SkipBuild,
    [switch]$DryRun
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 2.0
[Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
$ProgressPreference = 'SilentlyContinue'

$Root   = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$Native = Join-Path $Root 'native'
$Dist   = Join-Path $Root 'dist'
$Cache  = Join-Path $Dist 'cache'
$Stage  = Join-Path $Dist 'stage\MangaVox'

function Invoke-Native([string]$Exe, [string[]]$ArgList, [string]$WorkDir) {
    Push-Location $WorkDir
    try {
        & $Exe @ArgList
        if ($LASTEXITCODE -ne 0) { throw "$Exe $($ArgList -join ' ') failed with exit code $LASTEXITCODE" }
    } finally { Pop-Location }
}

# ---- version ----
if (-not $Version) {
    $inWs = $false
    foreach ($line in Get-Content (Join-Path $Native 'Cargo.toml')) {
        if ($line -match '^\s*\[(.+)\]\s*$') { $inWs = ($Matches[1] -eq 'workspace.package'); continue }
        if ($inWs -and $line -match '^\s*version\s*=\s*"([^"]+)"') { $Version = $Matches[1]; break }
    }
    if (-not $Version) { throw 'Could not read version from native/Cargo.toml' }
}
Write-Host "Packaging MangaVox v$Version"

New-Item -ItemType Directory -Force $Dist, $Cache | Out-Null

# ---- build app ----
$exe = Join-Path $Native 'target\release\mangavox.exe'
if (-not $DryRun -and -not $SkipBuild) {
    Invoke-Native 'cargo' @('build', '--release', '-p', 'mangavox') $Native
}
if (-not $DryRun -and -not (Test-Path $exe)) { throw "Missing $exe" }

# ---- libmpv ----
function Find-7z {
    $c = Get-Command 7z -ErrorAction SilentlyContinue
    if ($c) { return $c.Source }
    foreach ($p in @('C:\Program Files\7-Zip\7z.exe', 'C:\Program Files (x86)\7-Zip\7z.exe')) {
        if (Test-Path $p) { return $p }
    }
    throw '7-Zip not found (need 7z on PATH or in C:\Program Files\7-Zip)'
}

function Resolve-Libmpv {
    if ($env:MANGAVOX_LIBMPV) {
        if (-not (Test-Path $env:MANGAVOX_LIBMPV)) { throw "MANGAVOX_LIBMPV not found: $env:MANGAVOX_LIBMPV" }
        return (Resolve-Path $env:MANGAVOX_LIBMPV).Path
    }
    if ($env:LOCALAPPDATA) {
        $s = Join-Path $env:LOCALAPPDATA 'Programs\Stremio\libmpv-2.dll'
        if (Test-Path $s) { return $s }
    }
    $cached = Join-Path $Cache 'libmpv-2.dll'
    if (Test-Path $cached) { return $cached }

    Write-Host 'Downloading libmpv (shinchiro/mpv-winbuild-cmake)...'
    $headers = @{ 'User-Agent' = 'mangavox-package'; 'Accept' = 'application/vnd.github+json' }
    if ($env:GITHUB_TOKEN) { $headers['Authorization'] = "Bearer $env:GITHUB_TOKEN" }
    $rel = Invoke-RestMethod -Uri 'https://api.github.com/repos/shinchiro/mpv-winbuild-cmake/releases/latest' -Headers $headers
    $asset = @($rel.assets | Where-Object { $_.name -like 'mpv-dev-x86_64-v3-*.7z' }) | Select-Object -First 1
    if (-not $asset) { $asset = @($rel.assets | Where-Object { $_.name -like 'mpv-dev-x86_64-*.7z' }) | Select-Object -First 1 }
    if (-not $asset) { throw 'No mpv-dev-x86_64 asset found in latest shinchiro release' }
    $archive = Join-Path $Cache $asset.name
    Invoke-WebRequest -Uri $asset.browser_download_url -OutFile $archive -UseBasicParsing
    $ex = Join-Path $Cache 'mpv-extract'
    if (Test-Path $ex) { Remove-Item $ex -Recurse -Force }
    New-Item -ItemType Directory $ex | Out-Null
    $z = Find-7z
    & $z x $archive "-o$ex" 'libmpv-2.dll' '-r' '-y' | Out-Null
    if ($LASTEXITCODE -ne 0) { throw '7z extraction failed' }
    $dll = Get-ChildItem $ex -Recurse -Filter 'libmpv-2.dll' | Select-Object -First 1
    if (-not $dll) { throw 'libmpv-2.dll not found in archive' }
    Copy-Item $dll.FullName $cached -Force
    Remove-Item $ex -Recurse -Force
    return $cached
}

$libmpv = Resolve-Libmpv
Write-Host "libmpv: $libmpv"

# ---- stage ----
if (Test-Path (Join-Path $Dist 'stage')) { Remove-Item (Join-Path $Dist 'stage') -Recurse -Force }
New-Item -ItemType Directory -Force $Stage | Out-Null

if (-not $DryRun) { Copy-Item $exe (Join-Path $Stage 'mangavox.exe') }

$pySrc = Join-Path $Root 'python'
$pyDst = Join-Path $Stage 'python'
New-Item -ItemType Directory -Force $pyDst | Out-Null
$pySrcFull = (Resolve-Path $pySrc).Path.TrimEnd('\')
Get-ChildItem $pySrc -Recurse -File | Where-Object {
    $rel = $_.FullName.Substring($pySrcFull.Length + 1)
    -not ($rel -split '\\' | Where-Object { $_ -eq '__pycache__' }) -and
    $_.Extension -ne '.pyc' -and
    $_.Extension -ne '.swift' -and
    $_.BaseName -ne 'vision_ocr'
} | ForEach-Object {
    $rel = $_.FullName.Substring($pySrcFull.Length + 1)
    $dest = Join-Path $pyDst $rel
    New-Item -ItemType Directory -Force (Split-Path $dest) | Out-Null
    Copy-Item $_.FullName $dest
}

Copy-Item $libmpv (Join-Path $Stage 'libmpv-2.dll')
[IO.File]::WriteAllText((Join-Path $Stage 'version.txt'), $Version)

$pyCount = @(Get-ChildItem $pyDst -Recurse -File).Count
Write-Host "Staged $Stage ($pyCount python files)"

if ($DryRun) {
    Write-Host 'DryRun: skipping zip and installer.'
    return
}

# ---- zip ----
Add-Type -AssemblyName System.IO.Compression.FileSystem
$zip = Join-Path $Dist "MangaVox-$Version-win64.zip"
if (Test-Path $zip) { Remove-Item $zip -Force }
[IO.Compression.ZipFile]::CreateFromDirectory($Stage, $zip, [IO.Compression.CompressionLevel]::Optimal, $false)
Write-Host "Zip: $zip"

# ---- installer ----
$setup = Join-Path $Dist "MangaVox-Setup-$Version.exe"
if (Test-Path $setup) { Remove-Item $setup -Force }
$env:MANGAVOX_PAYLOAD = $zip
$ErrorActionPreference = 'Continue'
$out = & cargo build --release -p mangavox-setup --manifest-path (Join-Path $Native 'Cargo.toml') 2>&1
$code = $LASTEXITCODE
$ErrorActionPreference = 'Stop'
$out | ForEach-Object { Write-Host $_ }
if ($code -ne 0) {
    if (($out | Out-String) -match 'did not match any packages|package ID specification') {
        Write-Warning 'Package mangavox-setup does not exist yet; skipping installer.'
    } else {
        throw "cargo build -p mangavox-setup failed ($code)"
    }
} else {
    $built = Join-Path $Native 'target\release\mangavox-setup.exe'
    if (-not (Test-Path $built)) { throw "Missing $built" }
    Copy-Item $built $setup -Force
}

# ---- checksums ----
$sums = Join-Path $Dist 'SHA256SUMS.txt'
$lines = @()
foreach ($f in @($zip, $setup)) {
    if (Test-Path $f) {
        $h = (Get-FileHash $f -Algorithm SHA256).Hash.ToLower()
        $lines += "$h  $(Split-Path $f -Leaf)"
    }
}
[IO.File]::WriteAllText($sums, (($lines -join "`n") + "`n"))

# ---- summary ----
Write-Host ''
Write-Host "== MangaVox v$Version artifacts =="
foreach ($f in @($zip, $setup, $sums)) {
    if (Test-Path $f) { Write-Host ('{0,10:N1} MB  {1}' -f ((Get-Item $f).Length / 1MB), $f) }
}
