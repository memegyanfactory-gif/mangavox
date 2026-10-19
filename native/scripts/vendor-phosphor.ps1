#Requires -Version 7
<#
.SYNOPSIS
  Vendors the Phosphor icons listed in crates/mv-ui/src/icons.rs.

.DESCRIPTION
  Reads the icon names between the "vendor-phosphor:begin" and "vendor-phosphor:end" markers in
  icons.rs, downloads each one in four weights (regular, bold, fill, duotone) from the pinned
  @phosphor-icons/core package on jsDelivr, and writes them to
  crates/mv-ui/assets/ph/<weight>/<name>.svg (the weight suffix is stripped, so one name addresses
  every weight). The package LICENSE (MIT) goes to assets/ph/LICENSE. Files that are no longer in
  the list are removed. Existing files are kept unless -Force is given.

  To add an icon: add `NAME = "phosphor-name"` to the list in icons.rs, run this script, rebuild.
  To update Phosphor: bump -Version (and the version noted in assets/brand/ATTRIBUTION.txt), run with -Force.

.EXAMPLE
  pwsh native/scripts/vendor-phosphor.ps1
  pwsh native/scripts/vendor-phosphor.ps1 -Version 2.1.1 -Force
#>
[CmdletBinding()]
param(
    [string]$Version = '2.1.1',
    [int]$Parallel = 12,
    [switch]$Force
)

$ErrorActionPreference = 'Stop'

$native = Split-Path -Parent $PSScriptRoot
$ui = Join-Path $native 'crates/mv-ui'
$iconsRs = Join-Path $ui 'src/icons.rs'
$out = Join-Path $ui 'assets/ph'
$cdn = "https://cdn.jsdelivr.net/npm/@phosphor-icons/core@$Version"
$weights = @('regular', 'bold', 'fill', 'duotone')

# 1. The icon list lives in icons.rs, so the typed consts and the vendored files cannot drift apart.
$block = [regex]::Match((Get-Content -Raw $iconsRs), '(?s)//\s*vendor-phosphor:begin(.*?)//\s*vendor-phosphor:end')
if (-not $block.Success) { throw "No vendor-phosphor:begin/end markers found in $iconsRs" }
$names = [regex]::Matches($block.Groups[1].Value, '[A-Z][A-Z0-9_]*\s*=\s*"([a-z0-9-]+)"') |
    ForEach-Object { $_.Groups[1].Value } | Sort-Object -Unique
if (-not $names) { throw 'The icon list is empty' }
Write-Host "Phosphor ${Version}: $($names.Count) icons x $($weights.Count) weights"

# 2. Download. Regular has no suffix upstream; the other weights are <name>-<weight>.svg.
$jobs = foreach ($w in $weights) {
    New-Item -ItemType Directory -Force (Join-Path $out $w) | Out-Null
    foreach ($n in $names) {
        $file = if ($w -eq 'regular') { "$n.svg" } else { "$n-$w.svg" }
        [pscustomobject]@{ Url = "$cdn/assets/$w/$file"; Dest = Join-Path $out "$w/$n.svg" }
    }
}

$results = $jobs | ForEach-Object -ThrottleLimit $Parallel -Parallel {
    $j = $_
    if ((Test-Path $j.Dest) -and (Get-Item $j.Dest).Length -gt 0 -and -not $using:Force) { return 'kept' }
    for ($try = 1; $try -le 3; $try++) {
        try {
            Invoke-WebRequest -Uri $j.Url -OutFile $j.Dest -TimeoutSec 60
            if ([IO.File]::ReadAllText($j.Dest).TrimStart().StartsWith('<svg')) { return 'ok' }
            throw 'not an SVG'
        } catch {
            if ($try -eq 3) { return "FAILED $($j.Url): $($_.Exception.Message)" }
            Start-Sleep -Seconds $try
        }
    }
}

$failed = @($results | Where-Object { $_ -like 'FAILED*' })
if ($failed.Count) { $failed | ForEach-Object { Write-Error $_ -ErrorAction Continue }; throw "$($failed.Count) downloads failed" }
Write-Host ("Downloaded {0}, kept {1}" -f @($results | Where-Object { $_ -eq 'ok' }).Count, @($results | Where-Object { $_ -eq 'kept' }).Count)

# 3. Drop files for icons that left the list.
foreach ($w in $weights) {
    Get-ChildItem (Join-Path $out $w) -Filter *.svg |
        Where-Object { $names -notcontains $_.BaseName } |
        ForEach-Object { Write-Host "Removing stale $w/$($_.Name)"; Remove-Item $_.FullName }
}

# 4. Licence text must ship with the files (MIT).
Invoke-WebRequest -Uri "$cdn/LICENSE" -OutFile (Join-Path $out 'LICENSE') -TimeoutSec 60
Write-Host "Done: $out"
