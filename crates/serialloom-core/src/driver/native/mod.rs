//! Backend for SerialLoom's own virtual port driver (`drivers/serialloom-port`).

mod channel;
pub mod elevate;
mod localsign;
pub(crate) mod setup;

use crate::driver::{ControlChannel, PortDriver, validate_port_name};
use crate::error::{Error, Result};
use crate::model::{DriverInfo, VirtualPortInfo};

pub use elevate::{ADMIN_FLAG, admin_main};

#[derive(Default)]
pub struct NativeDriver;

impl NativeDriver {
    pub fn new() -> Self {
        Self
    }
}

impl PortDriver for NativeDriver {
    fn info(&self) -> DriverInfo {
        let installed = setup::driver_installed();
        let bundled = elevate::bundled_inf();
        DriverInfo {
            id: "native".into(),
            name: "SerialLoom virtual port driver".into(),
            available: installed,
            installable: bundled.is_some(),
            location: bundled.map(|p| p.display().to_string()),
            message: (!installed).then(|| {
                if elevate::bundled_inf().is_some() {
                    "The SerialLoom driver is not installed yet. Install it to create virtual COM ports."
                        .to_string()
                } else {
                    "The SerialLoom driver is not installed, and no driver package was found next to the app."
                        .to_string()
                }
            }),
        }
    }

    fn list_ports(&self) -> Result<Vec<VirtualPortInfo>> {
        setup::list()
    }

    fn create_ports(&self, names: &[String]) -> Result<Vec<VirtualPortInfo>> {
        if names.is_empty() {
            return Ok(Vec::new());
        }
        for n in names {
            validate_port_name(n)?;
        }
        if !setup::driver_installed() {
            return Err(Error::DriverUnavailable(
                "install the SerialLoom driver first (Settings)".into(),
            ));
        }
        elevate::run(elevate::AdminOp::CreatePorts(names.to_vec()))
    }

    fn remove_ports(&self, names: &[String]) -> Result<()> {
        if names.is_empty() {
            return Ok(());
        }
        for n in names {
            validate_port_name(n)?;
        }
        elevate::run(elevate::AdminOp::RemovePorts(names.to_vec())).map(|_| ())
    }

    fn install(&self) -> Result<()> {
        elevate::run(elevate::AdminOp::InstallDriver).map(|_| ())
    }

    fn diagnostics(&self) -> String {
        setup::diagnostics(&self.info())
    }

    fn open_control(&self, port: &str) -> Result<Box<dyn ControlChannel>> {
        let path = setup::control_path(port)?;
        Ok(Box::new(channel::NativeChannel::open(&path)?))
    }
}
