// In-memory backend used when the UI runs in a plain browser (`npm run dev`).
// It lets the UI be developed and previewed without building the Rust side.
import type { Api } from "./api";
import type {
  AppSettings,
  EndpointRequest,
  HubEndpoint,
  HubSpec,
  HubStatus,
  HubView,
  PortInfo,
  VirtualPortInfo,
} from "./types";
import { DEFAULT_SERIAL } from "./types";

let settings: AppSettings = { driver: "demo" };
const virtualPorts: VirtualPortInfo[] = [];
const hubs: HubView[] = [];
const statusListeners = new Set<(s: HubStatus[]) => void>();
const monitor: Record<string, { seq: number; source: number; data: number[]; timestampMs: number }[]> = {};

const physical: PortInfo[] = [
  port("COM1", "pci", "Communications Port"),
  port("COM3", "usb", "u-blox GNSS receiver", "u-blox AG", 0x1546, 0x01a8),
  port("COM4", "usb", "USB Serial Port", "FTDI", 0x0403, 0x6001),
  port("COM7", "bluetooth", "Standard Serial over Bluetooth link"),
];

function port(
  name: string,
  kind: PortInfo["kind"],
  description: string,
  manufacturer: string | null = null,
  vid: number | null = null,
  pid: number | null = null,
): PortInfo {
  return { name, kind, description, manufacturer, vid, pid, serialNumber: null, inUseBy: null, present: true };
}

function newVirtual(name: string): VirtualPortInfo {
  const taken = (n: number) => virtualPorts.some((p) => p.name === `COM${n}`) || physical.some((p) => p.name === `COM${n}`);
  let finalName = name;
  if (name === "COM#") {
    let n = 20;
    while (taken(n)) n++;
    finalName = `COM${n}`;
  } else if (taken(Number(name.slice(3)))) {
    throw `${name} already exists`;
  }
  const p: VirtualPortInfo = {
    id: `ROOT\\PORTS\\${String(virtualPorts.length).padStart(4, "0")}`,
    name: finalName,
    state: "ready",
    problem: null,
    problemCode: null,
    problemStatus: null,
    driverReport: null,
  };
  virtualPorts.push(p);
  return p;
}

function resolve(r: EndpointRequest): HubEndpoint {
  if (r.kind === "newVirtual") return { spec: { kind: "virtual", port: newVirtual(r.name).name }, created: true };
  return { spec: r, created: false };
}

function emptyStatus(spec: HubSpec, running: boolean): HubStatus {
  return {
    id: spec.id,
    running,
    endpoints: [spec.master, ...spec.members].map((e, i) => ({
      rxBytes: 0,
      txBytes: 0,
      droppedBytes: 0,
      connected: running,
      peerOpen: running && e.spec.kind === "virtual" && i !== 2,
      lastError: null,
      line:
        running && e.spec.kind === "virtual" && i !== 2
          ? { baudRate: i === 0 ? 115200 : 9600, framing: "8N1", dtr: true, rts: true }
          : null,
    })),
  };
}

function find(id: string) {
  const h = hubs.find((x) => x.spec.id === id);
  if (!h) throw `hub ${id} not found`;
  return h;
}

const delay = <T,>(v: T, ms = 120) => new Promise<T>((r) => setTimeout(() => r(structuredClone(v)), ms));

function seed() {
  const split: HubSpec = {
    id: "demo-gps",
    name: "GPS splitter",
    kind: "split",
    master: { spec: { kind: "serial", port: "COM3", settings: { ...DEFAULT_SERIAL, baudRate: 9600 } }, created: false },
    members: ["COM20", "COM21", "COM22"].map((n) => resolve({ kind: "newVirtual", name: n })),
    autostart: true,
  };
  const bridge: HubSpec = {
    id: "demo-bridge",
    name: "PLC over network",
    kind: "bridge",
    master: { spec: { kind: "serial", port: "COM4", settings: DEFAULT_SERIAL }, created: false },
    members: [{ spec: { kind: "tcpServer", bind: "0.0.0.0:7000" }, created: false }],
    autostart: false,
  };
  const pair: HubSpec = {
    id: "demo-pair",
    name: "COM30 ⇄ COM31",
    kind: "bridge",
    master: resolve({ kind: "newVirtual", name: "COM30" }),
    members: [resolve({ kind: "newVirtual", name: "COM31" })],
    autostart: true,
  };
  for (const s of [split, bridge, pair]) hubs.push({ spec: s, status: emptyStatus(s, true) });
}
seed();

// Simulated traffic: NMEA-like bursts from the split master, chatter on the bridges.
setInterval(() => {
  for (const h of hubs) {
    if (!h.status.running) continue;
    const eps = h.status.endpoints;
    const n = h.spec.kind === "split" ? 60 + Math.floor(Math.random() * 40) : Math.floor(Math.random() * 30);
    eps[0].rxBytes += n;
    for (let i = 1; i < eps.length; i++) eps[i].txBytes += n;
    if (Math.random() < 0.3) {
      const m = 1 + Math.floor(Math.random() * (eps.length - 1));
      const k = 8 + Math.floor(Math.random() * 12);
      eps[m].rxBytes += k;
      eps[0].txBytes += k;
    }
    const log = (monitor[h.spec.id] ??= []);
    const text =
      h.spec.kind === "split" ? "$GPGGA,123519,4807.038,N,01131.000,E,1,08,0.9,545.4,M,46.9,M,,*47\r\n" : "READ D100\r\n";
    log.push({
      seq: (log.at(-1)?.seq ?? 0) + 1,
      source: 0,
      timestampMs: Date.now(),
      data: Array.from(new TextEncoder().encode(text)),
    });
    if (log.length > 400) log.shift();
  }
  const snapshot = hubs.map((h) => structuredClone(h.status));
  statusListeners.forEach((cb) => cb(snapshot));
}, 500);

function portsList(): PortInfo[] {
  const list: PortInfo[] = [
    ...physical.map((p) => ({ ...p })),
    ...virtualPorts.map((v) => port(v.name, "virtual", "SerialLoom virtual port")),
  ];
  for (const h of hubs.filter((x) => x.status.running)) {
    for (const ep of [h.spec.master, ...h.spec.members]) {
      if (ep.spec.kind === "serial" || ep.spec.kind === "virtual") {
        const name = ep.spec.port;
        const p = list.find((x) => x.name === name);
        if (p) p.inUseBy = h.spec.id;
      }
    }
  }
  return list.sort((a, b) => a.name.localeCompare(b.name, undefined, { numeric: true }));
}

export const mockApi: Api = {
  appInfo: () =>
    delay({ name: "SerialLoom", version: "0.1.0-browser", repository: "https://github.com/RahulP5/SerialLoom", platform: "browser" }),
  getSettings: () => delay(settings),
  setSettings: async (s) => {
    settings = s;
    return mockApi.driverInfo();
  },
  driverInfo: () =>
    delay({
      id: "demo",
      name: "Browser preview (simulated)",
      available: true,
      installable: false,
      location: null,
      message: "You are running the UI in a browser. Data is simulated.",
    }),
  installDriver: () => mockApi.driverInfo(),
  diagnostics: () => delay("SerialLoom browser preview\nNo diagnostics: data is simulated."),
  listPorts: () => delay(portsList()),
  listVirtualPorts: () => delay(virtualPorts),
  createVirtualPorts: async (names) => delay(names.map(newVirtual), 400),
  renameVirtualPort: async (name, newName) => {
    const port = virtualPorts.find((p) => p.name === name);
    if (!port) throw `virtual port ${name} not found`;
    if (virtualPorts.some((p) => p.name === newName) || physical.some((p) => p.name === newName)) throw `${newName} is already used`;
    for (const h of hubs) {
      for (const e of [h.spec.master, ...h.spec.members]) {
        if (e.spec.kind === "virtual" && e.spec.port === name) e.spec.port = newName;
      }
    }
    port.name = newName;
    return delay(port, 400);
  },
  removeVirtualPort: async (name) => {
    const used = hubs.find((h) => [h.spec.master, ...h.spec.members].some((e) => e.spec.kind === "virtual" && e.spec.port === name));
    if (used) throw `${name} is used by hub '${used.spec.name}'`;
    virtualPorts.splice(virtualPorts.findIndex((p) => p.name === name), 1);
  },
  listHubs: () => delay(hubs),
  createHub: async (req) => {
    const [master, ...members] = [req.master, ...req.members].map(resolve);
    const spec: HubSpec = { id: Math.random().toString(16).slice(2, 14), name: req.name, kind: req.kind, master, members, autostart: req.autostart };
    const view = { spec, status: emptyStatus(spec, req.start) };
    hubs.push(view);
    return delay(view, 500);
  },
  startHub: async (id) => {
    const h = find(id);
    h.status = emptyStatus(h.spec, true);
    return delay(h);
  },
  stopHub: async (id) => {
    const h = find(id);
    h.status = emptyStatus(h.spec, false);
    return delay(h);
  },
  updateHub: async (id, name, autostart) => {
    const h = find(id);
    if (name) h.spec.name = name;
    if (autostart !== undefined) h.spec.autostart = autostart;
    return delay(h);
  },
  deleteHub: async (id, removeVirtual) => {
    const h = find(id);
    hubs.splice(hubs.indexOf(h), 1);
    if (removeVirtual) {
      for (const e of [h.spec.master, ...h.spec.members]) {
        if (e.created && e.spec.kind === "virtual") {
          const name = e.spec.port;
          virtualPorts.splice(virtualPorts.findIndex((p) => p.name === name), 1);
        }
      }
    }
  },
  addMember: async (id, req) => {
    const h = find(id);
    h.spec.members.push(resolve(req));
    h.status = emptyStatus(h.spec, h.status.running);
    return delay(h);
  },
  removeMember: async (id, index, removeVirtual) => {
    const h = find(id);
    const [m] = h.spec.members.splice(index, 1);
    if (removeVirtual && m.created && m.spec.kind === "virtual") {
      const name = m.spec.port;
      virtualPorts.splice(virtualPorts.findIndex((p) => p.name === name), 1);
    }
    h.status = emptyStatus(h.spec, h.status.running);
    return delay(h);
  },
  hubMonitor: async (id, after) => delay((monitor[id] ?? []).filter((r) => r.seq > after), 30),
  onStatus: async (cb) => {
    statusListeners.add(cb);
    return () => statusListeners.delete(cb);
  },
};
