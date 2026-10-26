//! Virtual port drivers.
//!
//! * [`native::NativeDriver`]: SerialLoom's own UMDF driver
//!   (`drivers/serialloom-port`). Windows only.
//! * [`demo::DemoDriver`]: in-memory stand-in so the app works without the
//!   driver installed. Its ports cannot be opened by other programs.

pub mod demo;
#[cfg(windows)]
pub mod native;
#[cfg(windows)]
pub mod usb;

use std::io;
use std::time::Duration;

use crate::error::{Error, Result};
use crate::model::{DriverInfo, VirtualPortInfo};

pub use serialloom_proto::PortState as ControlState;

/// The SerialLoom side of one virtual port: bytes written by the program on
/// the COM side are read here, and bytes written here reach the program.
pub trait ControlChannel: Send + Sync {
    /// Waits up to `timeout` for data. Returns `Ok(0)` when nothing arrived.
    fn read(&self, buf: &mut [u8], timeout: Duration) -> io::Result<usize>;

    /// Returns how many bytes the driver accepted.
    fn write(&self, data: &[u8]) -> io::Result<usize>;

    /// What the program on the COM side has configured, when available.
    fn state(&self) -> Option<ControlState>;
}

pub trait PortDriver: Send + Sync {
    fn info(&self) -> DriverInfo;

    fn list_ports(&self) -> Result<Vec<VirtualPortInfo>>;

    /// Creates ports in one batch (one elevation prompt). `COM#` picks the
    /// next free number. Returns the ports in the order requested.
    fn create_ports(&self, names: &[String]) -> Result<Vec<VirtualPortInfo>>;

    fn remove_ports(&self, names: &[String]) -> Result<()>;

    /// Installs the driver package bundled with the app.
    fn install(&self) -> Result<()> {
        Err(Error::Invalid(
            "this driver cannot be installed from SerialLoom".into(),
        ))
    }

    /// Changes a virtual port's COM number (e.g. COM6 -> COM20).
    fn rename_port(&self, _port: &str, _new_name: &str) -> Result<VirtualPortInfo> {
        Err(Error::Invalid("this driver cannot rename ports".into()))
    }

    /// Plain-text report for troubleshooting (versions, port states, logs).
    fn diagnostics(&self) -> String {
        "No diagnostics are available for this driver.".into()
    }

    /// Opens the control side of a virtual port.
    fn open_control(&self, port: &str) -> Result<Box<dyn ControlChannel>>;
}

/// Accepts `COM1`..`COM255` and `COM#`. Names end up in a command line run
/// as administrator, so this check is strict on purpose.
pub fn validate_port_name(name: &str) -> Result<()> {
    if name == "COM#" || com_number(name).is_some() {
        Ok(())
    } else {
        Err(Error::Invalid(format!(
            "'{name}' is not a valid port name. Use COM1 to COM255, or COM# for the next free number."
        )))
    }
}

/// `COM20` -> 20.
pub fn com_number(name: &str) -> Option<u32> {
    let digits = name.strip_prefix("COM").or_else(|| name.strip_prefix("com"))?;
    if digits.is_empty()
        || digits.len() > 3
        || !digits.chars().all(|c| c.is_ascii_digit())
        || digits.starts_with('0')
    {
        return None;
    }
    digits.parse().ok().filter(|n| (1..=255).contains(n))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn port_names() {
        assert!(validate_port_name("COM20").is_ok());
        assert!(validate_port_name("COM#").is_ok());
        assert!(validate_port_name("COM255").is_ok());
        assert!(validate_port_name("COM256").is_err());
        assert!(validate_port_name("COM0").is_err());
        assert!(validate_port_name("COM05").is_err());
        assert!(validate_port_name("COM1 & calc").is_err());
        assert!(validate_port_name("").is_err());
        assert_eq!(com_number("COM7"), Some(7));
    }
}
