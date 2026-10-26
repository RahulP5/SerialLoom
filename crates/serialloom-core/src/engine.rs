//! Owns the hubs, talks to the driver and persists the topology.
//!
//! Driver calls can block for a long time (a UAC prompt waits for the user),
//! so they always run with the hub lock released.

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};

use crate::config::{load_json, save_json};
use crate::driver::{PortDriver, validate_port_name};
use crate::error::{Error, Result};
use crate::hub::HubRuntime;
use crate::model::*;
use crate::ports::{natural_cmp, system_ports};

#[derive(Debug, Default, Serialize, Deserialize)]
struct Topology {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    hubs: Vec<HubSpec>,
}

struct HubEntry {
    spec: HubSpec,
    runtime: Option<HubRuntime>,
}

impl HubEntry {
    fn status(&self) -> HubStatus {
        let endpoints = match &self.runtime {
            Some(rt) => rt.status(),
            None => vec![EndpointStatus::default(); self.spec.endpoint_count()],
        };
        HubStatus {
            id: self.spec.id.clone(),
            running: self.runtime.is_some(),
            endpoints,
        }
    }

    fn view(&self) -> HubView {
        HubView {
            spec: self.spec.clone(),
            status: self.status(),
        }
    }
}

pub struct Engine {
    driver: RwLock<Arc<dyn PortDriver>>,
    hubs: Mutex<Vec<HubEntry>>,
    store: Option<PathBuf>,
}

impl Engine {
    /// `store` is the topology file; `None` keeps everything in memory.
    pub fn new(driver: Arc<dyn PortDriver>, store: Option<PathBuf>) -> Self {
        let topology: Topology = store.as_deref().map(load_json).unwrap_or_default();
        let hubs = topology
            .hubs
            .into_iter()
            .map(|spec| HubEntry { spec, runtime: None })
            .collect();
        Self {
            driver: RwLock::new(driver),
            hubs: Mutex::new(hubs),
            store,
        }
    }

    /// Starts every hub marked `autostart`.
    pub fn autostart(&self) {
        let ids: Vec<String> = self
            .hubs
            .lock()
            .iter()
            .filter(|h| h.spec.autostart)
            .map(|h| h.spec.id.clone())
            .collect();
        for id in ids {
            if let Err(e) = self.start_hub(&id) {
                tracing::warn!("autostart of hub {id} failed: {e}");
            }
        }
    }

    pub fn shutdown(&self) {
        for h in self.hubs.lock().iter_mut() {
            h.runtime = None;
        }
    }

    // ----- driver ---------------------------------------------------------

    pub fn driver(&self) -> Arc<dyn PortDriver> {
        Arc::clone(&self.driver.read())
    }

    /// Switches drivers. Running hubs are restarted on the new driver.
    pub fn set_driver(&self, driver: Arc<dyn PortDriver>) {
        *self.driver.write() = Arc::clone(&driver);
        for h in self.hubs.lock().iter_mut() {
            if h.runtime.take().is_some() {
                h.runtime = Some(HubRuntime::start(&h.spec, Arc::clone(&driver)));
            }
        }
    }

    pub fn driver_info(&self) -> DriverInfo {
        self.driver().info()
    }

    pub fn install_driver(&self) -> Result<DriverInfo> {
        let driver = self.driver();
        driver.install()?;
        Ok(driver.info())
    }

    pub fn diagnostics(&self) -> String {
        self.driver().diagnostics()
    }

    pub fn virtual_ports(&self) -> Result<Vec<VirtualPortInfo>> {
        self.driver().list_ports()
    }

    pub fn create_virtual_ports(&self, names: Vec<String>) -> Result<Vec<VirtualPortInfo>> {
        let names: Vec<String> = names.iter().map(|n| n.trim().to_ascii_uppercase()).collect();
        for n in &names {
            validate_port_name(n)?;
        }
        self.driver().create_ports(&names)
    }

    /// Gives a virtual port another COM number. Hubs that use the port follow
    /// the new name (running ones restart briefly).
    pub fn rename_virtual_port(&self, name: &str, new_name: &str) -> Result<VirtualPortInfo> {
        let info = self.driver().rename_port(name, new_name)?;
        let ids: Vec<String> = self
            .hubs
            .lock()
            .iter()
            .filter(|h| {
                h.spec.endpoints().any(
                    |e| matches!(&e.spec, EndpointSpec::Virtual { port } if port.eq_ignore_ascii_case(name)),
                )
            })
            .map(|h| h.spec.id.clone())
            .collect();
        for id in ids {
            let new = info.name.clone();
            self.mutate_and_restart(&id, |spec| {
                for e in std::iter::once(&mut spec.master).chain(spec.members.iter_mut()) {
                    if let EndpointSpec::Virtual { port } = &mut e.spec
                        && port.eq_ignore_ascii_case(name)
                    {
                        *port = new.clone();
                    }
                }
            })?;
        }
        self.save();
        Ok(info)
    }

    pub fn remove_virtual_port(&self, name: &str) -> Result<()> {
        if let Some(hub) = self.hub_using(name) {
            return Err(Error::Invalid(format!(
                "{name} is used by hub '{hub}'. Delete the hub or remove the port from it first."
            )));
        }
        self.driver().remove_ports(&[name.to_ascii_uppercase()])
    }

    fn hub_using(&self, port: &str) -> Option<String> {
        self.hubs
            .lock()
            .iter()
            .find(|h| {
                h.spec
                    .endpoints()
                    .any(|e| e.spec.port_name().is_some_and(|p| p.eq_ignore_ascii_case(port)))
            })
            .map(|h| h.spec.name.clone())
    }

    // ----- ports ----------------------------------------------------------

    pub fn ports(&self) -> Result<Vec<PortInfo>> {
        let mut ports = system_ports().unwrap_or_else(|e| {
            tracing::warn!("port enumeration failed: {e}");
            Vec::new()
        });
        let virtual_ports = self.virtual_ports().unwrap_or_else(|e| {
            tracing::warn!("listing virtual ports failed: {e}");
            Vec::new()
        });

        for v in &virtual_ports {
            match ports.iter_mut().find(|p| p.name.eq_ignore_ascii_case(&v.name)) {
                Some(p) => {
                    p.kind = PortKind::Virtual;
                    p.description = Some("SerialLoom virtual port".into());
                }
                None => ports.push(PortInfo {
                    name: v.name.clone(),
                    kind: PortKind::Virtual,
                    description: Some("SerialLoom virtual port".into()),
                    manufacturer: None,
                    vid: None,
                    pid: None,
                    serial_number: None,
                    in_use_by: None,
                    present: false,
                }),
            }
        }

        let mut in_use: HashMap<String, String> = HashMap::new();
        for h in self.hubs.lock().iter().filter(|h| h.runtime.is_some()) {
            for e in h.spec.endpoints() {
                if let Some(port) = e.spec.port_name() {
                    in_use.insert(port.to_ascii_uppercase(), h.spec.id.clone());
                }
            }
        }
        for p in &mut ports {
            p.in_use_by = in_use.get(&p.name.to_ascii_uppercase()).cloned();
        }
        ports.sort_by(|a, b| natural_cmp(&a.name, &b.name));
        Ok(ports)
    }

    // ----- hubs -----------------------------------------------------------

    pub fn hubs(&self) -> Vec<HubView> {
        self.hubs.lock().iter().map(HubEntry::view).collect()
    }

    pub fn statuses(&self) -> Vec<HubStatus> {
        self.hubs.lock().iter().map(HubEntry::status).collect()
    }

    pub fn monitor(&self, id: &str, after: u64) -> Result<Vec<MonitorRecord>> {
        let hubs = self.hubs.lock();
        let entry = find(&hubs, id)?;
        Ok(entry
            .runtime
            .as_ref()
            .map(|r| r.monitor_since(after))
            .unwrap_or_default())
    }

    pub fn create_hub(&self, req: CreateHubRequest) -> Result<HubView> {
        let name = req.name.trim().to_string();
        if name.is_empty() {
            return Err(Error::Invalid("give the hub a name".into()));
        }
        check_member_count(req.kind, req.members.len())?;
        let mut all = vec![req.master];
        all.extend(req.members);
        for r in &all {
            validate_request(r)?;
        }
        check_unique_ports(&all)?;

        let mut endpoints = self.resolve(all)?;
        let master = endpoints.remove(0);
        let spec = HubSpec {
            id: uuid::Uuid::new_v4().simple().to_string()[..12].to_string(),
            name,
            kind: req.kind,
            master,
            members: endpoints,
            autostart: req.autostart,
        };
        let id = spec.id.clone();
        self.hubs.lock().push(HubEntry { spec, runtime: None });
        self.save();
        if req.start {
            return self.start_hub(&id);
        }
        self.view(&id)
    }

    pub fn start_hub(&self, id: &str) -> Result<HubView> {
        let driver = self.driver();
        let mut hubs = self.hubs.lock();
        let idx = position(&hubs, id)?;
        if hubs[idx].runtime.is_none() {
            let wanted: HashSet<String> = ports_of(&hubs[idx].spec);
            for (i, other) in hubs.iter().enumerate() {
                if i == idx || other.runtime.is_none() {
                    continue;
                }
                if let Some(port) = ports_of(&other.spec).intersection(&wanted).next() {
                    return Err(Error::Invalid(format!(
                        "{port} is already open in hub '{}'. Stop that hub first.",
                        other.spec.name
                    )));
                }
            }
            let rt = HubRuntime::start(&hubs[idx].spec, driver);
            hubs[idx].runtime = Some(rt);
        }
        Ok(hubs[idx].view())
    }

    pub fn stop_hub(&self, id: &str) -> Result<HubView> {
        let mut hubs = self.hubs.lock();
        let idx = position(&hubs, id)?;
        hubs[idx].runtime = None;
        Ok(hubs[idx].view())
    }

    pub fn update_hub(&self, id: &str, name: Option<String>, autostart: Option<bool>) -> Result<HubView> {
        let view = {
            let mut hubs = self.hubs.lock();
            let idx = position(&hubs, id)?;
            if let Some(n) = name.map(|n| n.trim().to_string()).filter(|n| !n.is_empty()) {
                hubs[idx].spec.name = n;
            }
            if let Some(a) = autostart {
                hubs[idx].spec.autostart = a;
            }
            hubs[idx].view()
        };
        self.save();
        Ok(view)
    }

    pub fn delete_hub(&self, id: &str, remove_virtual_ports: bool) -> Result<()> {
        let entry = {
            let mut hubs = self.hubs.lock();
            let idx = position(&hubs, id)?;
            hubs.remove(idx)
        };
        let created = entry.spec.created_ports();
        drop(entry); // stops threads and closes ports before they are removed
        self.save();
        if remove_virtual_ports && !created.is_empty() {
            self.driver().remove_ports(&created)?;
        }
        Ok(())
    }

    pub fn add_member(&self, id: &str, req: EndpointRequest) -> Result<HubView> {
        validate_request(&req)?;
        {
            let hubs = self.hubs.lock();
            let entry = find(&hubs, id)?;
            check_member_count(entry.spec.kind, entry.spec.members.len() + 1)?;
            if let Some(port) = request_port(&req)
                && ports_of(&entry.spec).contains(&port)
            {
                return Err(Error::Invalid(format!("{port} is already part of this hub")));
            }
        }
        let endpoint = self.resolve(vec![req])?.remove(0);
        let view = self.mutate_and_restart(id, |spec| spec.members.push(endpoint))?;
        self.save();
        Ok(view)
    }

    pub fn remove_member(&self, id: &str, index: usize, remove_virtual_port: bool) -> Result<HubView> {
        {
            let hubs = self.hubs.lock();
            let entry = find(&hubs, id)?;
            if index >= entry.spec.members.len() {
                return Err(Error::NotFound(format!("member {index}")));
            }
            check_member_count(entry.spec.kind, entry.spec.members.len() - 1)?;
        }
        let mut removed = None;
        let view = self.mutate_and_restart(id, |spec| removed = Some(spec.members.remove(index)))?;
        self.save();
        if remove_virtual_port
            && let Some(m) = removed.filter(|m| m.created)
            && let Some(port) = m.spec.port_name()
        {
            self.driver().remove_ports(&[port.to_string()])?;
        }
        Ok(view)
    }

    // ----- helpers --------------------------------------------------------

    fn view(&self, id: &str) -> Result<HubView> {
        let hubs = self.hubs.lock();
        Ok(find(&hubs, id)?.view())
    }

    fn mutate_and_restart(&self, id: &str, f: impl FnOnce(&mut HubSpec)) -> Result<HubView> {
        let driver = self.driver();
        let mut hubs = self.hubs.lock();
        let idx = position(&hubs, id)?;
        let was_running = hubs[idx].runtime.take().is_some();
        f(&mut hubs[idx].spec);
        if was_running {
            let rt = HubRuntime::start(&hubs[idx].spec, driver);
            hubs[idx].runtime = Some(rt);
        }
        Ok(hubs[idx].view())
    }

    /// Turns requests into concrete endpoints, creating virtual ports in one batch.
    fn resolve(&self, requests: Vec<EndpointRequest>) -> Result<Vec<HubEndpoint>> {
        let wanted: Vec<String> = requests
            .iter()
            .filter_map(|r| match r {
                EndpointRequest::NewVirtual { name } => Some(name.trim().to_ascii_uppercase()),
                _ => None,
            })
            .collect();
        let created = if wanted.is_empty() {
            Vec::new()
        } else {
            self.driver().create_ports(&wanted)?
        };
        if created.len() != wanted.len() {
            return Err(Error::Driver(
                "the driver created a different number of ports than requested".into(),
            ));
        }
        let mut created = created.into_iter();

        Ok(requests
            .into_iter()
            .map(|r| {
                let (spec, created_here) = match r {
                    EndpointRequest::Serial { port, settings } => (
                        EndpointSpec::Serial {
                            port: port.trim().to_string(),
                            settings,
                        },
                        false,
                    ),
                    EndpointRequest::Virtual { port } => (
                        EndpointSpec::Virtual {
                            port: port.trim().to_ascii_uppercase(),
                        },
                        false,
                    ),
                    EndpointRequest::TcpServer { bind } => (
                        EndpointSpec::TcpServer {
                            bind: bind.trim().to_string(),
                        },
                        false,
                    ),
                    EndpointRequest::TcpClient { address } => (
                        EndpointSpec::TcpClient {
                            address: address.trim().to_string(),
                        },
                        false,
                    ),
                    EndpointRequest::NewVirtual { .. } => {
                        let port = created.next().expect("one port per request").name;
                        (EndpointSpec::Virtual { port }, true)
                    }
                };
                HubEndpoint {
                    spec,
                    created: created_here,
                }
            })
            .collect())
    }

    fn save(&self) {
        let Some(path) = &self.store else { return };
        let topology = Topology {
            version: 2,
            hubs: self.hubs.lock().iter().map(|h| h.spec.clone()).collect(),
        };
        if let Err(e) = save_json(path, &topology) {
            tracing::error!("cannot save topology: {e}");
        }
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn find<'a>(hubs: &'a [HubEntry], id: &str) -> Result<&'a HubEntry> {
    hubs.iter()
        .find(|h| h.spec.id == id)
        .ok_or_else(|| Error::NotFound(format!("hub {id}")))
}

fn position(hubs: &[HubEntry], id: &str) -> Result<usize> {
    hubs.iter()
        .position(|h| h.spec.id == id)
        .ok_or_else(|| Error::NotFound(format!("hub {id}")))
}

fn ports_of(spec: &HubSpec) -> HashSet<String> {
    spec.endpoints()
        .filter_map(|e| e.spec.port_name().map(str::to_ascii_uppercase))
        .collect()
}

fn request_port(r: &EndpointRequest) -> Option<String> {
    match r {
        EndpointRequest::Serial { port, .. } | EndpointRequest::Virtual { port } => {
            Some(port.trim().to_ascii_uppercase())
        }
        _ => None,
    }
}

fn check_member_count(kind: HubKind, members: usize) -> Result<()> {
    match kind {
        HubKind::Bridge if members != 1 => {
            Err(Error::Invalid("a bridge connects exactly two endpoints".into()))
        }
        _ if members == 0 => Err(Error::Invalid("a hub needs at least two endpoints".into())),
        _ if members > 32 => Err(Error::Invalid("a hub supports at most 33 endpoints".into())),
        _ => Ok(()),
    }
}

fn check_unique_ports(requests: &[EndpointRequest]) -> Result<()> {
    let mut seen = HashSet::new();
    for r in requests {
        let key = match r {
            EndpointRequest::Serial { port, .. } | EndpointRequest::Virtual { port } => {
                port.trim().to_ascii_uppercase()
            }
            EndpointRequest::NewVirtual { name } if name.trim() != "COM#" => name.trim().to_ascii_uppercase(),
            EndpointRequest::TcpServer { bind } => format!("tcp-listen:{}", bind.trim()),
            _ => continue,
        };
        if !seen.insert(key.clone()) {
            return Err(Error::Invalid(format!("{key} is used twice in this hub")));
        }
    }
    Ok(())
}

fn validate_request(r: &EndpointRequest) -> Result<()> {
    match r {
        EndpointRequest::Serial { port, settings } => {
            if port.trim().is_empty() {
                return Err(Error::Invalid("choose a serial port".into()));
            }
            validate_settings(settings)
        }
        EndpointRequest::Virtual { port } => {
            if port.trim().is_empty() {
                return Err(Error::Invalid("choose a virtual port".into()));
            }
            Ok(())
        }
        EndpointRequest::NewVirtual { name } => validate_port_name(&name.trim().to_ascii_uppercase()),
        EndpointRequest::TcpServer { bind } => bind
            .trim()
            .parse::<SocketAddr>()
            .map(|_| ())
            .map_err(|_| Error::Invalid(format!("'{bind}' is not a listen address such as 0.0.0.0:7000"))),
        EndpointRequest::TcpClient { address } => {
            let a = address.trim();
            match a.rsplit_once(':') {
                Some((host, port)) if !host.is_empty() && port.parse::<u16>().is_ok() => Ok(()),
                _ => Err(Error::Invalid(format!(
                    "'{address}' is not an address such as 192.168.1.20:4001"
                ))),
            }
        }
    }
}

fn validate_settings(s: &SerialSettings) -> Result<()> {
    if s.baud_rate == 0 || s.baud_rate > 12_000_000 {
        return Err(Error::Invalid(format!("unsupported baud rate {}", s.baud_rate)));
    }
    if !(5..=8).contains(&s.data_bits) {
        return Err(Error::Invalid(format!(
            "data bits must be 5-8, got {}",
            s.data_bits
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;
    use crate::driver::demo::DemoDriver;

    fn engine() -> (Engine, Arc<DemoDriver>) {
        let demo = Arc::new(DemoDriver::new());
        (Engine::new(demo.clone(), None), demo)
    }

    fn split_request() -> CreateHubRequest {
        CreateHubRequest {
            name: "GPS split".into(),
            kind: HubKind::Split,
            master: EndpointRequest::Serial {
                port: "COM3".into(),
                settings: SerialSettings::default(),
            },
            members: vec![
                EndpointRequest::NewVirtual { name: "COM20".into() },
                EndpointRequest::NewVirtual { name: "COM#".into() },
            ],
            autostart: false,
            start: false,
        }
    }

    fn wait_for(mut f: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !f() {
            assert!(Instant::now() < deadline, "condition not met in time");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn split_creates_virtual_ports() {
        let (e, _) = engine();
        let hub = e.create_hub(split_request()).unwrap();
        assert_eq!(hub.spec.members.len(), 2);
        assert_eq!(
            hub.spec.members[0].spec,
            EndpointSpec::Virtual { port: "COM20".into() }
        );
        assert!(hub.spec.members[0].created);
        assert_eq!(e.virtual_ports().unwrap().len(), 2);

        let ports = e.ports().unwrap();
        assert!(
            ports
                .iter()
                .any(|p| p.name == "COM20" && p.kind == PortKind::Virtual)
        );

        // Ports used by a hub cannot be removed directly.
        assert!(e.remove_virtual_port("COM20").is_err());
        e.delete_hub(&hub.spec.id, true).unwrap();
        assert!(e.virtual_ports().unwrap().is_empty());
    }

    /// Two virtual ports bridged together behave like a null-modem cable.
    #[test]
    fn bridged_virtual_ports_carry_data() {
        let (e, demo) = engine();
        let hub = e
            .create_hub(CreateHubRequest {
                name: "pair".into(),
                kind: HubKind::Bridge,
                master: EndpointRequest::NewVirtual { name: "COM30".into() },
                members: vec![EndpointRequest::NewVirtual { name: "COM31".into() }],
                autostart: false,
                start: true,
            })
            .unwrap();
        wait_for(|| e.statuses()[0].endpoints.iter().all(|s| s.connected));

        let a = demo.program_side("COM30").unwrap();
        let b = demo.program_side("COM31").unwrap();
        a.write(b"ping");
        let mut got = Vec::new();
        wait_for(|| {
            got.extend(b.read_all());
            got == b"ping"
        });
        assert!(!e.monitor(&hub.spec.id, 0).unwrap().is_empty());
    }

    #[test]
    fn renaming_a_port_updates_hubs() {
        let (e, _) = engine();
        let hub = e
            .create_hub(CreateHubRequest {
                name: "pair".into(),
                kind: HubKind::Bridge,
                master: EndpointRequest::NewVirtual { name: "COM30".into() },
                members: vec![EndpointRequest::NewVirtual { name: "COM31".into() }],
                autostart: false,
                start: false,
            })
            .unwrap();
        let info = e.rename_virtual_port("COM30", "com40").unwrap();
        assert_eq!(info.name, "COM40");
        let spec = &e.hubs()[0].spec;
        assert_eq!(spec.id, hub.spec.id);
        assert_eq!(spec.master.spec, EndpointSpec::Virtual { port: "COM40".into() });
        assert!(e.rename_virtual_port("COM40", "COM31").is_err(), "number in use");
    }

    #[test]
    fn bridge_needs_two_endpoints() {
        let (e, _) = engine();
        let mut req = split_request();
        req.kind = HubKind::Bridge;
        assert!(e.create_hub(req).is_err());
    }

    #[test]
    fn rejects_duplicates_and_bad_addresses() {
        let (e, _) = engine();
        let mut req = split_request();
        req.members[0] = EndpointRequest::Serial {
            port: "com3".into(),
            settings: SerialSettings::default(),
        };
        assert!(e.create_hub(req).is_err());

        let mut req = split_request();
        req.members[0] = EndpointRequest::TcpServer {
            bind: "nonsense".into(),
        };
        assert!(e.create_hub(req).is_err());

        let mut req = split_request();
        req.members[0] = EndpointRequest::NewVirtual {
            name: "COM1 & x".into(),
        };
        assert!(e.create_hub(req).is_err());
    }

    #[test]
    fn members_can_be_added_and_removed() {
        let (e, _) = engine();
        let hub = e.create_hub(split_request()).unwrap();
        let id = hub.spec.id;
        let v = e
            .add_member(
                &id,
                EndpointRequest::TcpServer {
                    bind: "127.0.0.1:0".into(),
                },
            )
            .unwrap();
        assert_eq!(v.spec.members.len(), 3);
        let v = e.remove_member(&id, 0, true).unwrap();
        assert_eq!(v.spec.members.len(), 2);
        assert_eq!(e.virtual_ports().unwrap().len(), 1);
    }

    #[test]
    fn topology_persists() {
        let dir = std::env::temp_dir().join(format!("serialloom-test-{}", std::process::id()));
        let path = dir.join("topology.json");
        {
            let e = Engine::new(Arc::new(DemoDriver::new()), Some(path.clone()));
            e.create_hub(split_request()).unwrap();
        }
        let e = Engine::new(Arc::new(DemoDriver::new()), Some(path));
        assert_eq!(e.hubs().len(), 1);
        let _ = std::fs::remove_dir_all(dir);
    }
}
