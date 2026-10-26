use std::collections::VecDeque;
use std::io;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::{Condvar, Mutex};

use super::{ControlChannel, ControlState, PortDriver, com_number, validate_port_name};
use crate::error::{Error, Result};
use crate::model::{DriverInfo, VirtualPortInfo, VirtualPortState};

/// Keeps ports in memory. Good for trying the UI and for tests.
///
/// Each demo port echoes nothing by itself, but tests can reach the "COM
/// side" through [`DemoDriver::program_side`] to act like a program.
#[derive(Default)]
pub struct DemoDriver {
    ports: Mutex<Vec<(VirtualPortInfo, Arc<DemoPipe>)>>,
}

/// Both directions of one demo port.
#[derive(Default)]
pub struct DemoPipe {
    to_program: Mutex<VecDeque<u8>>,
    to_control: Mutex<VecDeque<u8>>,
    signal: Condvar,
}

impl DemoDriver {
    pub fn new() -> Self {
        Self::default()
    }

    /// The program-facing side of a demo port, for tests.
    pub fn program_side(&self, port: &str) -> Option<DemoProgram> {
        self.ports
            .lock()
            .iter()
            .find(|(p, _)| p.name.eq_ignore_ascii_case(port))
            .map(|(_, pipe)| DemoProgram(Arc::clone(pipe)))
    }
}

/// Lets a test act as the program that opened a demo COM port.
pub struct DemoProgram(Arc<DemoPipe>);

impl DemoProgram {
    pub fn write(&self, data: &[u8]) {
        self.0.to_control.lock().extend(data);
        self.0.signal.notify_all();
    }

    pub fn read_all(&self) -> Vec<u8> {
        self.0.to_program.lock().drain(..).collect()
    }
}

struct DemoChannel(Arc<DemoPipe>);

impl ControlChannel for DemoChannel {
    fn read(&self, buf: &mut [u8], timeout: Duration) -> io::Result<usize> {
        let mut q = self.0.to_control.lock();
        if q.is_empty() {
            self.0.signal.wait_for(&mut q, timeout);
        }
        let n = buf.len().min(q.len());
        for (slot, b) in buf.iter_mut().zip(q.drain(..n)) {
            *slot = b;
        }
        Ok(n)
    }

    fn write(&self, data: &[u8]) -> io::Result<usize> {
        self.0.to_program.lock().extend(data);
        Ok(data.len())
    }

    fn state(&self) -> Option<ControlState> {
        None
    }
}

impl PortDriver for DemoDriver {
    fn info(&self) -> DriverInfo {
        DriverInfo {
            id: "demo".into(),
            name: "Demo driver (simulated)".into(),
            available: true,
            installable: false,
            location: None,
            message: Some(
                "Simulated ports exist only inside SerialLoom. Install the SerialLoom driver to create ports other programs can open."
                    .into(),
            ),
        }
    }

    fn list_ports(&self) -> Result<Vec<VirtualPortInfo>> {
        Ok(self.ports.lock().iter().map(|(p, _)| p.clone()).collect())
    }

    fn create_ports(&self, names: &[String]) -> Result<Vec<VirtualPortInfo>> {
        let mut ports = self.ports.lock();
        let mut created = Vec::with_capacity(names.len());
        for name in names {
            validate_port_name(name)?;
            let taken = |n: u32| ports.iter().any(|(p, _)| com_number(&p.name) == Some(n));
            let number = match com_number(name) {
                Some(n) if taken(n) => return Err(Error::Invalid(format!("{name} already exists"))),
                Some(n) => n,
                None => (50..=255)
                    .find(|&n| !taken(n))
                    .ok_or_else(|| Error::Invalid("no free COM numbers".into()))?,
            };
            let info = VirtualPortInfo {
                id: format!("DEMO\\PORTS\\{number:04}"),
                name: format!("COM{number}"),
                state: VirtualPortState::Ready,
                problem: None,
                problem_code: None,
                problem_status: None,
                driver_report: None,
            };
            ports.push((info.clone(), Arc::new(DemoPipe::default())));
            created.push(info);
        }
        Ok(created)
    }

    fn remove_ports(&self, names: &[String]) -> Result<()> {
        self.ports
            .lock()
            .retain(|(p, _)| !names.iter().any(|n| n.eq_ignore_ascii_case(&p.name)));
        Ok(())
    }

    fn rename_port(&self, port: &str, new_name: &str) -> Result<VirtualPortInfo> {
        let new_name = new_name.trim().to_ascii_uppercase();
        let number =
            com_number(&new_name).ok_or_else(|| Error::Invalid(format!("'{new_name}' is not a COM name")))?;
        let mut ports = self.ports.lock();
        if ports
            .iter()
            .any(|(p, _)| p.name.eq_ignore_ascii_case(&new_name) && !p.name.eq_ignore_ascii_case(port))
        {
            return Err(Error::Invalid(format!("{new_name} already exists")));
        }
        let (info, _) = ports
            .iter_mut()
            .find(|(p, _)| p.name.eq_ignore_ascii_case(port))
            .ok_or_else(|| Error::NotFound(format!("virtual port {port}")))?;
        info.name = format!("COM{number}");
        Ok(info.clone())
    }

    fn open_control(&self, port: &str) -> Result<Box<dyn ControlChannel>> {
        self.ports
            .lock()
            .iter()
            .find(|(p, _)| p.name.eq_ignore_ascii_case(port))
            .map(|(_, pipe)| Box::new(DemoChannel(Arc::clone(pipe))) as Box<dyn ControlChannel>)
            .ok_or_else(|| Error::NotFound(format!("virtual port {port}")))
    }
}
