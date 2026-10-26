//! Data types shared between the engine, the drivers and the UI.
//! Everything here serializes to camelCase JSON so the TypeScript side can
//! mirror it one-to-one (see `src/lib/types.ts`).

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Serial line settings
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Parity {
    #[default]
    None,
    Odd,
    Even,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StopBits {
    #[default]
    One,
    Two,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FlowControl {
    #[default]
    None,
    Software,
    Hardware,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SerialSettings {
    pub baud_rate: u32,
    pub data_bits: u8,
    pub parity: Parity,
    pub stop_bits: StopBits,
    pub flow_control: FlowControl,
}

impl Default for SerialSettings {
    fn default() -> Self {
        Self {
            baud_rate: 115_200,
            data_bits: 8,
            parity: Parity::None,
            stop_bits: StopBits::One,
            flow_control: FlowControl::None,
        }
    }
}

impl SerialSettings {
    /// Short form such as `115200 8N1`.
    pub fn summary(&self) -> String {
        let p = match self.parity {
            Parity::None => 'N',
            Parity::Odd => 'O',
            Parity::Even => 'E',
        };
        let s = match self.stop_bits {
            StopBits::One => 1,
            StopBits::Two => 2,
        };
        format!("{} {}{}{}", self.baud_rate, self.data_bits, p, s)
    }
}

// ---------------------------------------------------------------------------
// Endpoints and hubs
// ---------------------------------------------------------------------------

/// Something a hub can read bytes from and write bytes to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum EndpointSpec {
    /// A real serial device: built-in, USB or Bluetooth.
    Serial {
        port: String,
        #[serde(default)]
        settings: SerialSettings,
    },
    /// A SerialLoom virtual port. The hub talks to its control side, while
    /// other programs open `port` (e.g. COM20) as a normal serial port.
    Virtual { port: String },
    /// Listen for one TCP client (the newest client replaces older ones).
    TcpServer { bind: String },
    /// Connect to a remote TCP server and reconnect when the link drops.
    TcpClient { address: String },
}

impl EndpointSpec {
    pub fn label(&self) -> String {
        match self {
            EndpointSpec::Serial { port, .. } | EndpointSpec::Virtual { port } => port.clone(),
            EndpointSpec::TcpServer { bind } => format!("tcp://{bind} (listen)"),
            EndpointSpec::TcpClient { address } => format!("tcp://{address}"),
        }
    }

    /// COM port name this endpoint occupies, real or virtual.
    pub fn port_name(&self) -> Option<&str> {
        match self {
            EndpointSpec::Serial { port, .. } | EndpointSpec::Virtual { port } => Some(port),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HubEndpoint {
    pub spec: EndpointSpec,
    /// True when this hub created the virtual port and may remove it with itself.
    #[serde(default)]
    pub created: bool,
}

/// How a hub routes data. Endpoint index 0 is the "master".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HubKind {
    /// One source port shared by several virtual ports.
    /// master -> every member, any member -> master.
    Split,
    /// Several ports merged into one virtual port (master).
    /// Same routing as `Split`; only the direction of intent differs.
    Join,
    /// Exactly two endpoints wired together. Two virtual ports bridged
    /// together form a classic null-modem pair.
    Bridge,
    /// Every endpoint hears every other endpoint.
    Bus,
}

impl HubKind {
    pub fn is_broadcast(self) -> bool {
        matches!(self, HubKind::Bus)
    }
}

/// Indices that receive data read from endpoint `src`.
pub fn route_targets(kind: HubKind, count: usize, src: usize) -> Vec<usize> {
    if kind.is_broadcast() {
        (0..count).filter(|&i| i != src).collect()
    } else if src == 0 {
        (1..count).collect()
    } else {
        vec![0]
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HubSpec {
    pub id: String,
    pub name: String,
    pub kind: HubKind,
    pub master: HubEndpoint,
    pub members: Vec<HubEndpoint>,
    #[serde(default)]
    pub autostart: bool,
}

impl HubSpec {
    /// Master first, then members, matching the indices used in stats.
    pub fn endpoints(&self) -> impl Iterator<Item = &HubEndpoint> {
        std::iter::once(&self.master).chain(self.members.iter())
    }

    pub fn endpoint_count(&self) -> usize {
        1 + self.members.len()
    }

    /// Virtual ports this hub created.
    pub fn created_ports(&self) -> Vec<String> {
        self.endpoints()
            .filter(|e| e.created)
            .filter_map(|e| e.spec.port_name().map(str::to_string))
            .collect()
    }
}

/// What the UI asks for. `NewVirtual` makes the engine create a virtual port first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum EndpointRequest {
    Serial {
        port: String,
        #[serde(default)]
        settings: SerialSettings,
    },
    /// An existing SerialLoom virtual port.
    Virtual {
        port: String,
    },
    TcpServer {
        bind: String,
    },
    TcpClient {
        address: String,
    },
    NewVirtual {
        /// `COM20`, or `COM#` to pick the next free number.
        name: String,
    },
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateHubRequest {
    pub name: String,
    pub kind: HubKind,
    pub master: EndpointRequest,
    pub members: Vec<EndpointRequest>,
    #[serde(default)]
    pub autostart: bool,
    #[serde(default = "yes")]
    pub start: bool,
}

// ---------------------------------------------------------------------------
// Runtime status
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EndpointStatus {
    /// Bytes read from this endpoint.
    pub rx_bytes: u64,
    /// Bytes written to this endpoint.
    pub tx_bytes: u64,
    /// Bytes that could not be delivered to this endpoint.
    pub dropped_bytes: u64,
    pub connected: bool,
    /// For virtual ports: whether a program has the COM side open.
    #[serde(default)]
    pub peer_open: bool,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HubStatus {
    pub id: String,
    pub running: bool,
    pub endpoints: Vec<EndpointStatus>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HubView {
    pub spec: HubSpec,
    pub status: HubStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MonitorRecord {
    pub seq: u64,
    pub timestamp_ms: u64,
    /// Endpoint index the bytes were read from.
    pub source: usize,
    pub data: Vec<u8>,
}

// ---------------------------------------------------------------------------
// Ports and drivers
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PortKind {
    Usb,
    Pci,
    Bluetooth,
    Virtual,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PortInfo {
    pub name: String,
    pub kind: PortKind,
    pub description: Option<String>,
    pub manufacturer: Option<String>,
    pub vid: Option<u16>,
    pub pid: Option<u16>,
    pub serial_number: Option<String>,
    /// Hub that opens this port (a real port, or the control side of a virtual one).
    pub in_use_by: Option<String>,
    /// False when the driver knows the port but the OS does not list it yet.
    pub present: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum VirtualPortState {
    /// Driver loaded; programs can open the port.
    Ready,
    /// Windows reports a problem (see `problem`).
    Problem,
    /// Device exists but has not started.
    Stopped,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VirtualPortInfo {
    /// Device instance ID, e.g. `ROOT\PORTS\0003`.
    pub id: String,
    /// COM name, e.g. `COM20`.
    pub name: String,
    pub state: VirtualPortState,
    pub problem: Option<String>,
    /// Windows device problem code (e.g. 10 = failed to start).
    #[serde(default)]
    pub problem_code: Option<u32>,
    /// NTSTATUS behind the problem, e.g. `0xC0000034`.
    #[serde(default)]
    pub problem_status: Option<String>,
    /// What the driver recorded while starting (`SerialLoomStartup`).
    #[serde(default)]
    pub driver_report: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DriverInfo {
    pub id: String,
    pub name: String,
    /// Ports can be created and carry data.
    pub available: bool,
    /// The driver package exists next to the app and can be installed.
    pub installable: bool,
    pub location: Option<String>,
    pub message: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn star_routing() {
        assert_eq!(route_targets(HubKind::Split, 4, 0), vec![1, 2, 3]);
        assert_eq!(route_targets(HubKind::Split, 4, 2), vec![0]);
        assert_eq!(route_targets(HubKind::Bridge, 2, 1), vec![0]);
    }

    #[test]
    fn bus_routing() {
        assert_eq!(route_targets(HubKind::Bus, 3, 1), vec![0, 2]);
    }

    #[test]
    fn settings_summary() {
        assert_eq!(SerialSettings::default().summary(), "115200 8N1");
    }

    #[test]
    fn endpoint_json_shape() {
        let req: EndpointRequest = serde_json::from_str(r#"{"kind":"newVirtual","name":"COM#"}"#).unwrap();
        assert!(matches!(req, EndpointRequest::NewVirtual { .. }));
        let spec = EndpointSpec::Virtual { port: "COM20".into() };
        assert_eq!(
            serde_json::to_string(&spec).unwrap(),
            r#"{"kind":"virtual","port":"COM20"}"#
        );
    }
}
