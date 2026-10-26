<#
.SYNOPSIS
  Collects the Microsoft-signed usbip-win2 files that SerialLoom bundles.

.DESCRIPTION
  SerialLoom creates virtual COM ports as software USB serial devices. Windows
  plugs them in through the usbip-win2 virtual USB host controller, whose
  drivers are signed by Microsoft (WHLK).

  The official usbip-win2 installer uses an Inno Setup format that current
  unpacking tools cannot read, so the files are taken from an installed copy:
  Windows keeps every installed driver package unmodified (inf, sys and the
  Microsoft-signed cat) in its driver store.

    * If usbip-win2 is installed, its files are copied.
    * With -InstallTemporarily, the official release is downloaded, checked
      against a pinned SHA-256, installed silently, copied, and uninstalled
      again (needs administrator rights; USB 3 hubs restart briefly).

  Every driver file must be signed by "Microsoft Windows Hardware
  Compatibility Publisher", or the script stops. Output: vendor\usbip-win2\.
  usbip-win2 is BSD-2-Clause licensed (https://github.com/vadimgrn/usbip-win2).

.PARAMETER InstallTemporarily
  Install the official release just long enough to copy its files.

.PARAMETER KeepInstalled
  With -InstallTemporarily: do not uninstall afterwards.
#>
param(
  [switch] $InstallTemporarily,
  [switch] $KeepInstalled,
  [string] $Version = "0.9.8.1",
  [string] $InstallerSha256 = "38cad6d4432b52d5bb9409d9ad03b72fdffc4ada4cd3a48fbeca1a2752a8518a"
)

$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"
$repo = Split-Path -Parent $PSScriptRoot
$dest = Join-Path $repo "vendor\usbip-win2"
$appDir = Join-Path $env:ProgramFiles "USBip"
$driverStore = Join-Path $env:WINDIR "System32\DriverStore\FileRepository"

function Test-Installed {
  (Test-Path (Join-Path $appDir "usbip.exe")) -and
  (Get-ChildItem $driverStore -Directory -Filter "usbip2_ude.inf_*" -ErrorAction SilentlyContinue)
}

function Invoke-Elevated([string] $file, [string[]] $arguments) {
  $p = Start-Process -FilePath $file -ArgumentList $arguments -Verb RunAs -Wait -PassThru
  if ($p.ExitCode -ne 0) { throw "$(Split-Path -Leaf $file) exited with code $($p.ExitCode)" }
}

$installedHere = $false
$work = $null
try {
  if (-not (Test-Installed)) {
    if (-not $InstallTemporarily) {
      throw "usbip-win2 is not installed. Run again with -InstallTemporarily to install it just long enough to copy its signed files."
    }
    $work = Join-Path ([IO.Path]::GetTempPath()) ("serialloom-usbip-" + [guid]::NewGuid().ToString("N"))
    New-Item -ItemType Directory -Force $work | Out-Null
    $installer = Join-Path $work "USBip-$Version-x64.exe"
    $url = "https://github.com/vadimgrn/usbip-win2/releases/download/v.$Version/USBip-$Version-x64.exe"
    Write-Host "Downloading $url"
    Invoke-WebRequest -Uri $url -OutFile $installer -UseBasicParsing
    $actual = (Get-FileHash $installer -Algorithm SHA256).Hash.ToLower()
    if ($actual -ne $InstallerSha256.ToLower()) { throw "SHA-256 mismatch: expected $InstallerSha256, got $actual" }

    Write-Host "Installing usbip-win2 temporarily (Windows asks for administrator permission)..."
    Invoke-Elevated $installer @("/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/SP-")
    $installedHere = $true
    if (-not (Test-Installed)) { throw "usbip-win2 did not install as expected." }
  }

  # Newest copy of each driver package in the driver store.
  $pkg = {
    param([string] $inf)
    $d = Get-ChildItem $driverStore -Directory -Filter "$inf`_*" | Sort-Object LastWriteTime -Descending | Select-Object -First 1
    if (-not $d) { throw "$inf is not in the driver store" }
    $d.FullName
  }
  $ude = & $pkg "usbip2_ude.inf"
  $filter = & $pkg "usbip2_filter.inf"

  Write-Host "Checking Microsoft signatures..."
  foreach ($f in @(Get-ChildItem $ude -File) + @(Get-ChildItem $filter -File) | Where-Object { $_.Extension -in ".sys", ".cat" }) {
    $sig = Get-AuthenticodeSignature $f.FullName
    $signer = $sig.SignerCertificate.Subject
    if ($sig.Status -ne "Valid" -or $signer -notmatch "Microsoft Windows Hardware Compatibility Publisher") {
      throw "$($f.Name) is not signed by Microsoft (status $($sig.Status), signer '$signer')"
    }
    Write-Host "  $($f.Name): $signer"
  }

  if (Test-Path $dest) { Remove-Item $dest -Recurse -Force }
  New-Item -ItemType Directory -Force (Join-Path $dest "driver") | Out-Null
  foreach ($d in $ude, $filter) {
    Get-ChildItem $d -File | Where-Object { $_.Extension -in ".inf", ".sys", ".cat" } |
      ForEach-Object { Copy-Item $_.FullName (Join-Path $dest "driver\$($_.Name)") }
  }
  foreach ($name in "usbip.exe", "libusbip.dll", "resources.dll") {
    $src = Join-Path $appDir $name
    if (-not (Test-Path $src)) { throw "$name not found in $appDir" }
    Copy-Item $src (Join-Path $dest $name)
  }
  foreach ($name in "usbip2_ude.inf", "usbip2_ude.sys", "usbip2_ude.cat", "usbip2_filter.inf", "usbip2_filter.sys", "usbip2_filter.cat") {
    if (-not (Test-Path (Join-Path $dest "driver\$name"))) { throw "driver\$name was not collected" }
  }

  $toolVersion = (Get-Item (Join-Path $dest "usbip.exe")).VersionInfo.ProductVersion
  Invoke-WebRequest -Uri "https://raw.githubusercontent.com/vadimgrn/usbip-win2/master/LICENSE.txt" `
    -OutFile (Join-Path $dest "LICENSE.txt") -UseBasicParsing

  $lines = @(
    "usbip-win2 (unmodified files), usbip.exe version $toolVersion",
    "Project: https://github.com/vadimgrn/usbip-win2",
    "Drivers copied from the Windows driver store; signed by Microsoft Windows Hardware Compatibility Publisher.",
    "",
    "Files (SHA-256):"
  )
  foreach ($f in Get-ChildItem $dest -Recurse -File | Where-Object Name -ne "SOURCE.txt") {
    $lines += "  " + $f.FullName.Substring($dest.Length + 1) + "  " + (Get-FileHash $f.FullName -Algorithm SHA256).Hash.ToLower()
  }
  $lines | Set-Content -Encoding utf8 (Join-Path $dest "SOURCE.txt")
  Write-Host ""
  Write-Host "usbip-win2 files ready in $dest (usbip.exe $toolVersion)" -ForegroundColor Green
} finally {
  if ($installedHere -and -not $KeepInstalled) {
    $uninstaller = Join-Path $appDir "unins000.exe"
    if (Test-Path $uninstaller) {
      Write-Host "Uninstalling the temporary usbip-win2 copy..."
      try { Invoke-Elevated $uninstaller @("/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART") } catch { Write-Warning $_ }
    }
  }
  if ($work) { Remove-Item $work -Recurse -Force -ErrorAction SilentlyContinue }
}
