<#
.SYNOPSIS
  Builds the SerialLoom driver and produces a test-signed driver package.

.DESCRIPTION
  Output goes to drivers\serialloom-port\target\<profile>\package:
    serialloom_port.dll, serialloom_port.inf, serialloom_port.cat,
    SerialLoomTestCert.cer (public part of the test certificate)

  Requires the Windows Driver Kit and LLVM (for libclang). See docs/DRIVER.md.

  Test-signed packages only load on machines in test-signing mode that trust
  SerialLoomTestCert.cer. Public releases must be signed through Microsoft
  attestation signing instead (see docs/DRIVER.md).

.PARAMETER Profile
  Cargo profile: release (default) or dev.

.PARAMETER NoSign
  Skip test signing (for example when a release pipeline signs later).
#>
param(
  [ValidateSet("release", "dev")] [string] $Profile = "release",
  [switch] $NoSign
)

# Native tools write progress to stderr; failures are detected through exit codes.
$ErrorActionPreference = "Continue"
$PSDefaultParameterValues["*:ErrorAction"] = "Stop"
$here = $PSScriptRoot

function Find-KitTool([string] $name) {
  $root = (Get-ItemProperty "HKLM:\SOFTWARE\Microsoft\Windows Kits\Installed Roots" -ErrorAction Stop).KitsRoot10
  $tool = Get-ChildItem -Path (Join-Path $root "bin"), (Join-Path $root "Tools") -Recurse -Filter $name -ErrorAction SilentlyContinue |
    Where-Object { $_.FullName -match "\\x64\\" -or $_.FullName -match "\\x86\\" } |
    Sort-Object { $_.FullName -match "\\x64\\" } -Descending |
    Sort-Object { [version]($_.FullName -replace '.*\\bin\\([\d\.]+)\\.*', '$1' -replace '^[^\d].*', '0.0') } -Descending |
    Select-Object -First 1
  if (-not $tool) { throw "$name not found. Install the Windows Driver Kit." }
  return $tool.FullName
}

if (-not $env:LIBCLANG_PATH -and (Test-Path "$env:ProgramFiles\LLVM\bin\libclang.dll")) {
  $env:LIBCLANG_PATH = "$env:ProgramFiles\LLVM\bin"
}

# Version comes from Cargo.toml so the INF and crate never disagree.
$version = (Select-String -Path "$here\Cargo.toml" -Pattern '^version\s*=\s*"([^"]+)"').Matches[0].Groups[1].Value
$infVersion = "$version.0"
# UMDF version the driver is built against (Cargo.toml metadata), e.g. 2.25.0
$umdfMinor = (Select-String -Path "$here\Cargo.toml" -Pattern '^target-umdf-version-minor\s*=\s*(\d+)').Matches[0].Groups[1].Value
$umdfVersion = "2.$umdfMinor.0"

Write-Host "Building serialloom_port $version ($Profile)..."
Push-Location $here
try {
  $cargoArgs = @("build")
  if ($Profile -eq "release") { $cargoArgs += "--release" }
  & cargo @cargoArgs 2>&1 | ForEach-Object { "$_" }
  if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }
} finally { Pop-Location }

$targetDir = if ($Profile -eq "release") { "release" } else { "debug" }
$out = Join-Path $here "target\$targetDir\package"
if (Test-Path $out) { Remove-Item $out -Recurse -Force }
New-Item -ItemType Directory -Force $out | Out-Null

Copy-Item "$here\target\$targetDir\serialloom_port.dll" $out
Copy-Item "$here\serialloom_port.inx" "$out\serialloom_port.inf"

Write-Host "Stamping INF..."
& (Find-KitTool "stampinf.exe") -f "$out\serialloom_port.inf" -d "*" -a amd64 -u $umdfVersion -v $infVersion | Out-Null
if ($LASTEXITCODE -ne 0) { throw "stampinf failed" }

Write-Host "Verifying INF..."
& (Find-KitTool "infverif.exe") /v /w "$out\serialloom_port.inf"
if ($LASTEXITCODE -ne 0) { throw "infverif reported problems" }

if (-not $NoSign) {
  $subject = "CN=SerialLoom Test Signing"
  $cert = Get-ChildItem Cert:\CurrentUser\My -CodeSigningCert | Where-Object Subject -eq $subject | Select-Object -First 1
  if (-not $cert) {
    Write-Host "Creating test certificate '$subject' in CurrentUser\My..."
    $cert = New-SelfSignedCertificate -Type CodeSigningCert -Subject $subject `
      -CertStoreLocation Cert:\CurrentUser\My -KeyUsage DigitalSignature -NotAfter (Get-Date).AddYears(5)
  }
  Export-Certificate -Cert $cert -FilePath "$out\SerialLoomTestCert.cer" | Out-Null
  $signtool = Find-KitTool "signtool.exe"

  Write-Host "Signing driver binary..."
  & $signtool sign /fd SHA256 /sha1 $cert.Thumbprint "$out\serialloom_port.dll"
  if ($LASTEXITCODE -ne 0) { throw "signtool (dll) failed" }
}

Write-Host "Creating catalog..."
& (Find-KitTool "inf2cat.exe") "/driver:$out" "/os:10_x64" /uselocaltime
if ($LASTEXITCODE -ne 0) { throw "inf2cat failed" }

if (-not $NoSign) {
  Write-Host "Signing catalog..."
  & $signtool sign /fd SHA256 /sha1 $cert.Thumbprint "$out\serialloom_port.cat"
  if ($LASTEXITCODE -ne 0) { throw "signtool (cat) failed" }
}

Write-Host ""
Write-Host "Driver package ready: $out"
Get-ChildItem $out | Format-Table Name, Length -AutoSize
