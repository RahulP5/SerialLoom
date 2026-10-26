//! Virtual COM ports as software USB serial devices.
//!
//! SerialLoom runs a USB/IP server on 127.0.0.1 and asks the usbip-win2
//! virtual host controller to plug each device in. Windows then loads its own
//! Microsoft-signed `usbser.sys` and creates the COM port. usbip-win2's
//! drivers are Microsoft-signed too, so this works without test mode or any
//! SerialLoom driver signing.
//!
//! The ports exist while SerialLoom runs. Each keeps a fixed USB serial
//! number, so Windows gives it the same COM number every time.

pub mod system;
pub mod vhci;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::config::{load_json, save_json};
use crate::driver::{ControlChannel, PortDriver, validate_port_name};
use crate::error::{Error, Result};
use crate::model::{DriverInfo, VirtualPortInfo, VirtualPortState};
use crate::usbip::{UsbipServer, VirtualDevice};

/// How long to wait for Windows to create the COM port after plugging in.
const ENUMERATION_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SavedPort {
    /// USB serial number; decides which COM number Windows assigns.
    serial: String,
    /// Last COM name seen, shown while the port is being re-attached.
    name: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct SavedPorts {
    #[serde(default)]
    ports: Vec<SavedPort>,
}

struct UsbPort {
    saved: SavedPort,
    device: Arc<VirtualDevice>,
    /// Last problem worth showing on the port's card (e.g. a failed rename).
    note: Option<String>,
}

pub struct UsbDriver {
    server: Option<UsbipServer>,
    server_error: Option<String>,
    ports: Mutex<Vec<UsbPort>>,
    /// Virtual hub port of each attached device, by bus ID.
    hub_ports: Mutex<HashMap<String, i32>>,
    store: PathBuf,
}

impl UsbDriver {
    /// `config_dir` keeps the list of ports so they come back on restart.
    pub fn new(config_dir: &Path) -> Arc<Self> {
        let store = config_dir.join("usb-ports.json");
        let (server, server_error) = match UsbipServer::start() {
            Ok(s) => (Some(s), None),
            Err(e) => (None, Some(e.to_string())),
        };
        let saved: SavedPorts = load_json(&store);
        let ports = match &server {
            Some(s) => saved
                .ports
                .into_iter()
                .map(|saved| UsbPort {
                    device: s.add_device(&saved.serial),
                    saved,
                    note: None,
                })
                .collect(),
            None => Vec::new(),
        };
        let driver = Arc::new(Self {
            server,
            server_error,
            ports: Mutex::new(ports),
            hub_ports: Mutex::new(HashMap::new()),
            store,
        });

        // Plug the saved ports back into Windows without delaying startup.
        let worker = Arc::clone(&driver);
        let _ = std::thread::Builder::new()
            .name("usb-reattach".into())
            .spawn(move || worker.reattach_all());
        driver
    }

    fn controller_ready() -> bool {
        matches!(system::vhci_state(), Some((VirtualPortState::Ready, _)))
    }

    fn reattach_all(&self) {
        if !Self::controller_ready() {
            return;
        }
        let devices: Vec<Arc<VirtualDevice>> =
            self.ports.lock().iter().map(|p| Arc::clone(&p.device)).collect();
        for d in devices {
            if let Err(e) = self.attach(&d) {
                tracing::warn!("re-attaching {} failed: {e}", d.busid());
            }
        }
    }

    fn server(&self) -> Result<&UsbipServer> {
        self.server.as_ref().ok_or_else(|| {
            Error::DriverUnavailable(format!(
                "the local USB/IP server could not start: {}",
                self.server_error.as_deref().unwrap_or("unknown error")
            ))
        })
    }

    /// Asks the virtual host controller to plug the device in.
    fn attach(&self, device: &VirtualDevice) -> Result<()> {
        let server = self.server()?;
        let controller =
            vhci::Vhci::open().map_err(|e| Error::Driver(format!("virtual USB controller: {e}")))?;
        let hub_port = controller
            .attach("127.0.0.1", server.port(), device.busid())
            .map_err(|e| Error::Driver(format!("Windows did not accept the virtual USB device: {e}")))?;
        self.hub_ports.lock().insert(device.busid().to_string(), hub_port);
        Ok(())
    }

    /// Unplugs the device and stops serving it.
    fn unplug(&self, device: &VirtualDevice) {
        if let Some(hub_port) = self.hub_ports.lock().remove(device.busid())
            && let Ok(controller) = vhci::Vhci::open()
        {
            let _ = controller.detach(hub_port);
        }
        if let Some(server) = &self.server {
            server.remove_device(device.busid());
        }
    }

    fn save(&self) {
        let ports = self.ports.lock().iter().map(|p| p.saved.clone()).collect();
        if let Err(e) = save_json(&self.store, &SavedPorts { ports }) {
            tracing::error!("cannot save virtual port list: {e}");
        }
    }

    /// Current COM name of a port (Windows may take a moment to assign it).
    fn current_name(saved: &SavedPort) -> Option<String> {
        system::com_port_for_serial(&saved.serial).or_else(|| saved.name.clone())
    }

    /// Waits until Windows names the device's COM port (optionally a
    /// specific name, after a rename) and returns it.
    fn wait_for_name(serial: &str, want: Option<&str>) -> Result<String> {
        let deadline = Instant::now() + ENUMERATION_TIMEOUT;
        loop {
            if let Some(n) = system::com_port_for_serial(serial)
                && want.is_none_or(|w| n.eq_ignore_ascii_case(w))
            {
                return Ok(n);
            }
            if Instant::now() >= deadline {
                return Err(Error::Driver(match want {
                    Some(w) => format!("Windows did not switch the port to {w} in time. Check Diagnostics."),
                    None => {
                        "Windows did not create a COM port for the virtual device in time. Check Diagnostics."
                            .into()
                    }
                }));
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }

    fn find_info(&self, serial: &str) -> Result<VirtualPortInfo> {
        self.ports
            .lock()
            .iter()
            .find(|p| p.saved.serial == serial)
            .map(Self::info_of)
            .ok_or_else(|| Error::NotFound(format!("virtual device {serial}")))
    }

    fn info_of(port: &UsbPort) -> VirtualPortInfo {
        let attached = port.device.is_attached();
        let live_name = system::com_port_for_serial(&port.saved.serial);
        let (state, problem) = match (attached, &live_name) {
            (true, Some(_)) => (VirtualPortState::Ready, None),
            (true, None) => (
                VirtualPortState::Stopped,
                Some("Windows is setting up the port…".to_string()),
            ),
            (false, _) => (
                VirtualPortState::Stopped,
                Some("Not connected to Windows. It reconnects when SerialLoom starts.".to_string()),
            ),
        };
        VirtualPortInfo {
            id: format!("USB\\{}", port.saved.serial),
            name: live_name
                .or_else(|| port.saved.name.clone())
                .unwrap_or_else(|| "(pending)".into()),
            state,
            problem: problem.or_else(|| port.note.clone()),
            problem_code: None,
            problem_status: None,
            driver_report: None,
        }
    }
}

impl PortDriver for UsbDriver {
    fn info(&self) -> DriverInfo {
        let controller = system::vhci_state();
        let available = matches!(controller, Some((VirtualPortState::Ready, _))) && self.server.is_some();
        let message = if available {
            None
        } else if controller.is_none() {
            Some(
                "The virtual USB controller is not installed yet. Install it to create virtual COM ports."
                    .into(),
            )
        } else if let Some((_, Some(problem))) = &controller {
            Some(format!("The virtual USB controller has a problem: {problem}"))
        } else {
            self.server_error.clone()
        };
        DriverInfo {
            id: "usb".into(),
            name: "Virtual USB serial (Windows usbser + usbip-win2 controller)".into(),
            available,
            installable: system::drivers_bundled(),
            location: system::bundled_dir().map(|p| p.display().to_string()),
            message,
        }
    }

    fn list_ports(&self) -> Result<Vec<VirtualPortInfo>> {
        let mut list: Vec<VirtualPortInfo> = self.ports.lock().iter().map(Self::info_of).collect();
        list.sort_by(|a, b| crate::ports::natural_cmp(&a.name, &b.name));
        Ok(list)
    }

    fn create_ports(&self, names: &[String]) -> Result<Vec<VirtualPortInfo>> {
        let names: Vec<String> = names.iter().map(|n| n.trim().to_ascii_uppercase()).collect();
        for (i, n) in names.iter().enumerate() {
            validate_port_name(n)?;
            if n != "COM#" {
                if names[..i].contains(n) {
                    return Err(Error::Invalid(format!("{n} is requested twice")));
                }
                if let Some(owner) = system::com_name_holder(n, None) {
                    return Err(Error::Invalid(format!("{n} is already used by \"{owner}\".")));
                }
            }
        }
        if !Self::controller_ready() {
            return Err(Error::DriverUnavailable(
                "install the virtual USB controller first (Settings)".into(),
            ));
        }
        let server = self.server()?;
        tracing::info!("creating virtual ports {names:?}");

        // 1. Plug in every device and wait for Windows to name its port.
        let mut plugged: Vec<(UsbPort, String)> = Vec::new();
        for requested in &names {
            let serial = format!(
                "SL{}",
                uuid::Uuid::new_v4().simple().to_string()[..10].to_ascii_uppercase()
            );
            let device = server.add_device(&serial);
            let assigned = self
                .attach(&device)
                .and_then(|()| Self::wait_for_name(&serial, None));
            tracing::info!("device {serial}: Windows named it {assigned:?} (asked {requested})");
            match assigned {
                Ok(name) => plugged.push((
                    UsbPort {
                        saved: SavedPort {
                            serial,
                            name: Some(name),
                        },
                        device,
                        note: None,
                    },
                    requested.clone(),
                )),
                Err(e) => {
                    self.unplug(&device);
                    for (p, _) in &plugged {
                        self.unplug(&p.device);
                    }
                    return Err(e);
                }
            }
        }

        // 2. Give ports the numbers that were asked for (one admin prompt).
        let renames: Vec<(String, String)> = plugged
            .iter()
            .filter(|(p, want)| want != "COM#" && p.saved.name.as_deref() != Some(want.as_str()))
            .map(|(p, want)| (p.saved.serial.clone(), want.clone()))
            .collect();
        tracing::info!("renames needed: {renames:?}");
        let rename_error = if renames.is_empty() {
            None
        } else {
            match super::native::elevate::run(super::native::elevate::AdminOp::RenameUsbPorts(renames)) {
                Ok(_) => None,
                Err(e) => {
                    tracing::error!("rename failed: {e}");
                    Some(e.to_string())
                }
            }
        };

        // 3. Record the final names.
        let mut created = Vec::new();
        for (mut port, want) in plugged {
            if want != "COM#"
                && rename_error.is_none()
                && let Ok(name) = Self::wait_for_name(&port.saved.serial, Some(&want))
            {
                port.saved.name = Some(name);
            }
            let mut info = Self::info_of(&port);
            if want != "COM#" && !info.name.eq_ignore_ascii_case(&want) {
                port.note = Some(format!(
                    "Asked for {want}, Windows kept {}{}.",
                    info.name,
                    rename_error
                        .as_deref()
                        .map(|e| format!(": {e}"))
                        .unwrap_or_default()
                ));
                info.problem = Some(format!(
                    "Windows assigned {} instead of {want}{}. Use \"Change number\" to try again.",
                    info.name,
                    rename_error
                        .as_deref()
                        .map(|e| format!(" ({e})"))
                        .unwrap_or_default()
                ));
            }
            created.push(info);
            self.ports.lock().push(port);
        }
        self.save();
        Ok(created)
    }

    fn rename_port(&self, port: &str, new_name: &str) -> Result<VirtualPortInfo> {
        let new_name = new_name.trim().to_ascii_uppercase();
        validate_port_name(&new_name)?;
        if new_name == "COM#" {
            return Err(Error::Invalid(
                "choose a specific COM number, such as COM20".into(),
            ));
        }
        let serial = {
            let ports = self.ports.lock();
            let found = ports
                .iter()
                .find(|p| Self::current_name(&p.saved).is_some_and(|n| n.eq_ignore_ascii_case(port)))
                .ok_or_else(|| Error::NotFound(format!("virtual port {port}")))?;
            found.saved.serial.clone()
        };
        if new_name.eq_ignore_ascii_case(port) {
            return self.find_info(&serial);
        }
        if let Some(owner) = system::com_name_holder(&new_name, Some(&serial)) {
            return Err(Error::Invalid(format!(
                "{new_name} is already used by \"{owner}\"."
            )));
        }
        super::native::elevate::run(super::native::elevate::AdminOp::RenameUsbPorts(vec![(
            serial.clone(),
            new_name.clone(),
        )]))?;
        let name = Self::wait_for_name(&serial, Some(&new_name))?;
        tracing::info!("device {serial} renamed to {name}");
        if let Some(p) = self.ports.lock().iter_mut().find(|p| p.saved.serial == serial) {
            p.saved.name = Some(name);
            p.note = None;
        }
        self.save();
        self.find_info(&serial)
    }

    fn remove_ports(&self, names: &[String]) -> Result<()> {
        let removed: Vec<UsbPort> = {
            let mut ports = self.ports.lock();
            let (gone, keep): (Vec<UsbPort>, Vec<UsbPort>) = ports.drain(..).partition(|p| {
                Self::current_name(&p.saved).is_some_and(|n| names.iter().any(|x| x.eq_ignore_ascii_case(&n)))
            });
            *ports = keep;
            gone
        };
        if removed.is_empty() {
            return Ok(());
        }
        for p in &removed {
            self.unplug(&p.device);
        }
        self.save();
        // Give Windows a moment to see the devices unplugged, then remove
        // their entries and COM numbers so nothing stays behind.
        std::thread::sleep(Duration::from_millis(800));
        let serials: Vec<String> = removed.iter().map(|p| p.saved.serial.clone()).collect();
        super::native::elevate::run(super::native::elevate::AdminOp::RemoveUsbDevices(serials))
            .map(|_| ())
            .map_err(|e| {
                Error::Driver(format!(
                    "The port was removed from SerialLoom, but Windows still lists it ({e}).                      It holds no data; uninstalling SerialLoom or Device Manager removes it."
                ))
            })
    }

    fn install(&self) -> Result<()> {
        super::native::elevate::run(super::native::elevate::AdminOp::InstallUsbip).map(|_| ())
    }

    fn diagnostics(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!("Windows: {}\n", super::native::setup::windows_version()));
        out.push_str(&format!("Virtual USB controller: {:?}\n", system::vhci_state()));
        out.push_str(&format!(
            "Controller interface: {}\n",
            match vhci::Vhci::open() {
                Ok(_) => "opened".to_string(),
                Err(e) => format!("cannot open ({e})"),
            }
        ));
        out.push_str(&format!(
            "Bundled drivers: {:?} (complete: {})\n",
            system::bundled_dir(),
            system::drivers_bundled()
        ));
        match &self.server {
            Some(s) => out.push_str(&format!("USB/IP server: 127.0.0.1:{}\n", s.port())),
            None => out.push_str(&format!("USB/IP server failed: {:?}\n", self.server_error)),
        }
        out.push_str("\n== Virtual ports ==\n");
        let hub_ports = self.hub_ports.lock().clone();
        for p in self.ports.lock().iter() {
            out.push_str(&format!(
                "{} busid={} hub_port={:?} serial={} attached={} program_open={} com={:?}\n",
                p.saved.name.as_deref().unwrap_or("?"),
                p.device.busid(),
                hub_ports.get(p.device.busid()),
                p.saved.serial,
                p.device.is_attached(),
                p.device.is_port_open(),
                system::com_port_for_serial(&p.saved.serial)
            ));
        }
        out.push_str("\n== Recent log (");
        out.push_str(&crate::logging::log_path().display().to_string());
        out.push_str(") ==\n");
        out.push_str(&crate::logging::tail(80));
        out.push('\n');
        out
    }

    fn open_control(&self, port: &str) -> Result<Box<dyn ControlChannel>> {
        let ports = self.ports.lock();
        let found = ports
            .iter()
            .find(|p| Self::current_name(&p.saved).is_some_and(|n| n.eq_ignore_ascii_case(port)))
            .ok_or_else(|| Error::NotFound(format!("virtual port {port}")))?;
        Ok(Box::new(found.device.open_control()?))
    }
}
