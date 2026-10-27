// Mirrors crates/serialloom-core/src/model.rs and config.rs.

export type Parity = "none" | "odd" | "even";
export type StopBits = "one" | "two";
export type FlowControl = "none" | "software" | "hardware";

export interface SerialSettings {
  baudRate: number;
  dataBits: number;
  parity: Parity;
  stopBits: StopBits;
  flowControl: FlowControl;
}

export type EndpointSpec =
  | { kind: "serial"; port: string; settings: SerialSettings }
  | { kind: "virtual"; port: string }
  | { kind: "tcpServer"; bind: string }
  | { kind: "tcpClient"; address: string };

export type EndpointRequest = EndpointSpec | { kind: "newVirtual"; name: string };

export interface HubEndpoint {
  spec: EndpointSpec;
  /** True when the hub created this virtual port. */
  created: boolean;
}

export type HubKind = "split" | "join" | "bridge" | "bus";

export interface HubSpec {
  id: string;
  name: string;
  kind: HubKind;
  master: HubEndpoint;
  members: HubEndpoint[];
  autostart: boolean;
}

export interface EndpointStatus {
  rxBytes: number;
  txBytes: number;
  droppedBytes: number;
  connected: boolean;
  /** Virtual ports: a program has the COM side open. */
  peerOpen: boolean;
  lastError: string | null;
  /** Virtual ports: the line settings the program chose (null while no program has the port open). */
  line?: LineStatus | null;
}

export interface LineStatus {
  baudRate: number;
  /** Data bits, parity, stop bits, e.g. "8N1". */
  framing: string;
  dtr: boolean;
  rts: boolean;
}

export interface HubStatus {
  id: string;
  running: boolean;
  endpoints: EndpointStatus[];
}

export interface HubView {
  spec: HubSpec;
  status: HubStatus;
}

export interface CreateHubRequest {
  name: string;
  kind: HubKind;
  master: EndpointRequest;
  members: EndpointRequest[];
  autostart: boolean;
  start: boolean;
}

export interface MonitorRecord {
  seq: number;
  timestampMs: number;
  source: number;
  data: number[];
}

export type PortKind = "usb" | "pci" | "bluetooth" | "virtual" | "unknown";

export interface PortInfo {
  name: string;
  kind: PortKind;
  description: string | null;
  manufacturer: string | null;
  vid: number | null;
  pid: number | null;
  serialNumber: string | null;
  inUseBy: string | null;
  present: boolean;
}

export interface DriverInfo {
  id: string;
  name: string;
  available: boolean;
  installable: boolean;
  location: string | null;
  message: string | null;
}

export type VirtualPortState = "ready" | "problem" | "stopped";

export interface VirtualPortInfo {
  id: string;
  name: string;
  state: VirtualPortState;
  problem: string | null;
  problemCode: number | null;
  problemStatus: string | null;
  /** What the driver recorded while starting. */
  driverReport: string | null;
}

export type DriverKind = "usb" | "native" | "demo";

export interface AppSettings {
  driver: DriverKind;
}

export interface AppInfo {
  name: string;
  version: string;
  repository: string;
  platform: string;
}

export const DEFAULT_SERIAL: SerialSettings = {
  baudRate: 115200,
  dataBits: 8,
  parity: "none",
  stopBits: "one",
  flowControl: "none",
};
