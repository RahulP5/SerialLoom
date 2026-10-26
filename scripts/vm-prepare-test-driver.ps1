<#
.SYNOPSIS
  Prepares a TEST virtual machine to load the test-signed SerialLoom driver.

.DESCRIPTION
  Run this inside a disposable VM, never on your everyday PC. It:
    1. turns on Windows test-signing mode (needs Secure Boot OFF in the VM),
    2. trusts the SerialLoom test certificate (SerialLoomTestCert.cer),
  and then asks you to reboot.

  Undo later with:
    bcdedit /set testsigning off
    and remove "SerialLoom Test Signing" from certlm.msc > Trusted Root and Trusted Publishers.

.PARAMETER Certificate
  Path to SerialLoomTestCert.cer from drivers\serialloom-port\target\release\package.
#>
param(
  [Parameter(Mandatory = $true)] [string] $Certificate
)

$principal = [Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
  Write-Error "Run this from an elevated PowerShell (Run as administrator)."
  exit 1
}
if (-not (Test-Path $Certificate)) {
  Write-Error "Certificate not found: $Certificate"
  exit 1
}

try {
  if (Confirm-SecureBootUEFI) {
    Write-Warning "Secure Boot is ON. Test-signing is ignored while Secure Boot is on. Turn it off in the VM settings first."
  }
} catch {
  # Not a UEFI VM, or Secure Boot is unsupported: test signing can be used.
}

Write-Host "Trusting the SerialLoom test certificate..."
Import-Certificate -FilePath $Certificate -CertStoreLocation Cert:\LocalMachine\Root | Out-Null
Import-Certificate -FilePath $Certificate -CertStoreLocation Cert:\LocalMachine\TrustedPublisher | Out-Null

Write-Host "Enabling test-signing mode..."
bcdedit /set testsigning on
if ($LASTEXITCODE -ne 0) {
  Write-Error "bcdedit failed. Is Secure Boot off?"
  exit 1
}

Write-Host ""
Write-Host "Done. Reboot the VM, then install SerialLoom (or use Settings > Install driver)." -ForegroundColor Green
