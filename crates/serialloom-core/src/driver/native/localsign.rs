//! Free, per-PC driver signing.
//!
//! Windows installs a driver package only when its catalog (`.cat`) is
//! signed by a publisher the PC trusts. Instead of a paid certificate,
//! SerialLoom signs the catalog on each PC during installation, the way
//! libwdi/Zadig do:
//!
//! 1. copy the bundled package into a fresh folder under %ProgramData%
//!    (only administrators can change files there);
//! 2. check that the catalog is exactly the one this build shipped with
//!    (SHA-256 recorded at compile time), so nothing else can get signed;
//! 3. create a new certificate on this PC, trust it, sign the catalog;
//! 4. delete the certificate's private key straight away.
//!
//! Because the key is gone, nobody (including SerialLoom's authors) can sign
//! anything else that this PC would trust. Each PC has its own certificate.
//!
//! A catalog that already carries a trusted signature (for example a
//! Microsoft-signed release build) is installed as is.

use std::path::{Path, PathBuf};
use std::process::Command;

use sha2::{Digest, Sha256};

use crate::error::{Error, Result};

/// SHA-256 of the unsigned catalog this build was made with (see build.rs).
const EXPECTED_CAT_SHA256: &str = env!("SERIALLOOM_DRIVER_CAT_SHA256");

/// Subject prefix of the per-PC certificates, used to find them again on uninstall.
const CERT_SUBJECT_PREFIX: &str = "SerialLoom Local Driver Signing";

const PACKAGE_FILES: [&str; 3] = [
    "serialloom_port.inf",
    "serialloom_port.dll",
    "serialloom_port.cat",
];

/// A signed copy of the driver package. The folder is deleted on drop.
pub struct StagedPackage {
    dir: PathBuf,
}

impl StagedPackage {
    pub fn inf(&self) -> PathBuf {
        self.dir.join(PACKAGE_FILES[0])
    }
}

impl Drop for StagedPackage {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn sha256_hex(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path)?;
    Ok(format!("{:x}", Sha256::digest(&bytes)))
}

fn staging_dir() -> Result<PathBuf> {
    let base = std::env::var_os("ProgramData")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("C:\\ProgramData"));
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let dir = base
        .join("SerialLoom")
        .join(format!("driver-{}-{stamp}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Copies, verifies and signs the bundled package. Requires administrator rights.
pub fn prepare(bundled_inf: &Path) -> Result<StagedPackage> {
    if EXPECTED_CAT_SHA256.is_empty() {
        return Err(Error::DriverUnavailable(
            "this build does not include a driver package (build it with drivers/serialloom-port/package.ps1)"
                .into(),
        ));
    }
    let source = bundled_inf
        .parent()
        .ok_or_else(|| Error::DriverUnavailable("driver package folder not found".into()))?;

    let staged = StagedPackage { dir: staging_dir()? };
    for name in PACKAGE_FILES {
        let from = source.join(name);
        if !from.is_file() {
            return Err(Error::DriverUnavailable(format!(
                "{name} is missing from the driver package"
            )));
        }
        std::fs::copy(&from, staged.dir.join(name))?;
    }

    let cat = staged.dir.join(PACKAGE_FILES[2]);
    if sha256_hex(&cat)? != EXPECTED_CAT_SHA256 {
        return Err(Error::Driver(
            "the driver package does not match this version of SerialLoom, so it was not installed. Reinstall SerialLoom."
                .into(),
        ));
    }

    sign_package(&staged.dir.join(PACKAGE_FILES[1]), &cat)?;
    Ok(staged)
}

/// Signs the driver DLL (embedded signature) and the catalog with a new
/// per-PC certificate, then deletes the private key. The DLL's Authenticode
/// hash excludes its signature, so signing it keeps the catalog valid.
///
/// The path reaches PowerShell through an environment variable, never by
/// pasting it into the script text.
fn sign_package(dll: &Path, cat: &Path) -> Result<()> {
    let script = format!(
        r#"
$ErrorActionPreference = 'Stop'
$cat = $env:SERIALLOOM_CAT
$existing = Get-AuthenticodeSignature -FilePath $cat
if ($existing.Status -eq 'Valid') {{ 'already-signed'; exit 0 }}

$subject = 'CN={CERT_SUBJECT_PREFIX} (' + $env:COMPUTERNAME + ')'
$cert = New-SelfSignedCertificate -Type CodeSigningCert -Subject $subject `
  -CertStoreLocation Cert:\LocalMachine\My -KeyUsage DigitalSignature `
  -KeyExportPolicy NonExportable -HashAlgorithm SHA256 -NotAfter (Get-Date).AddYears(30)
try {{
  $public = [System.Security.Cryptography.X509Certificates.X509Certificate2]::new($cert.RawData)
  foreach ($storeName in 'Root', 'TrustedPublisher') {{
    $store = [System.Security.Cryptography.X509Certificates.X509Store]::new($storeName, 'LocalMachine')
    $store.Open('ReadWrite'); $store.Add($public); $store.Close()
  }}
  foreach ($file in $env:SERIALLOOM_DLL, $cat) {{
    $sig = Set-AuthenticodeSignature -FilePath $file -Certificate $cert -HashAlgorithm SHA256
    if ($sig.Status -ne 'Valid') {{ throw "signing $file failed: $($sig.StatusMessage)" }}
  }}
  'signed'
}} finally {{
  # Delete the private key: this certificate can never sign anything else.
  Remove-Item -Path ("Cert:\LocalMachine\My\" + $cert.Thumbprint) -DeleteKey
}}
"#
    );
    run_powershell(&script, Some((dll, cat)))
}

/// Removes every per-PC SerialLoom certificate (used by the uninstaller).
pub fn remove_local_certificates() {
    let script = format!(
        r#"
foreach ($storeName in 'Root', 'TrustedPublisher', 'My') {{
  Get-ChildItem ("Cert:\LocalMachine\" + $storeName) |
    Where-Object {{ $_.Subject -like 'CN={CERT_SUBJECT_PREFIX}*' }} |
    ForEach-Object {{ Remove-Item -Path $_.PSPath -DeleteKey -ErrorAction SilentlyContinue }}
}}
"#
    );
    let _ = run_powershell(&script, None);
}

fn run_powershell(script: &str, files: Option<(&Path, &Path)>) -> Result<()> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    let system = std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("C:\\Windows"));
    let exe = system.join("System32\\WindowsPowerShell\\v1.0\\powershell.exe");
    let mut cmd = Command::new(exe);
    cmd.args(["-NoProfile", "-NonInteractive", "-Command", script])
        .creation_flags(CREATE_NO_WINDOW);
    if let Some((dll, cat)) = files {
        cmd.env("SERIALLOOM_DLL", dll).env("SERIALLOOM_CAT", cat);
    }
    let out = cmd.output()?;
    if out.status.success() {
        tracing::info!("driver signing: {}", String::from_utf8_lossy(&out.stdout).trim());
        Ok(())
    } else {
        Err(Error::Driver(format!(
            "could not sign the driver on this PC: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_files() {
        let dir = std::env::temp_dir().join(format!("sl-hash-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("x");
        std::fs::write(&f, b"abc").unwrap();
        assert_eq!(
            sha256_hex(&f).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
