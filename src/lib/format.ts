import type { EndpointRequest, EndpointSpec, EndpointStatus, HubEndpoint, HubKind, SerialSettings } from "./types";

export function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  if (n < 1024 ** 3) return `${(n / 1024 / 1024).toFixed(1)} MB`;
  return `${(n / 1024 ** 3).toFixed(2)} GB`;
}

export function formatRate(bps: number): string {
  if (bps < 1) return "0 B/s";
  return `${formatBytes(Math.round(bps))}/s`;
}

export function serialSummary(s: SerialSettings): string {
  const p = s.parity === "none" ? "N" : s.parity === "odd" ? "O" : "E";
  return `${s.baudRate} ${s.dataBits}${p}${s.stopBits === "one" ? 1 : 2}`;
}

export function endpointTitle(e: HubEndpoint): string {
  return specTitle(e.spec);
}

export function specTitle(s: EndpointSpec | EndpointRequest): string {
  switch (s.kind) {
    case "serial":
    case "virtual":
      return s.port;
    case "tcpServer":
      return `:${s.bind.split(":").pop()}`;
    case "tcpClient":
      return s.address;
    case "newVirtual":
      return s.name === "COM#" ? "COM (auto)" : s.name;
  }
}

/**
 * When a hub's ports run at different speeds, lists them (e.g. "COM20 9600, COM21 115200").
 * Real ports use their configured speed; virtual ports the speed their program chose.
 */
export function speedMismatch(endpoints: HubEndpoint[], statuses: EndpointStatus[]): string | null {
  const speeds = endpoints.flatMap((e, i) => {
    if (e.spec.kind === "serial") return [{ port: e.spec.port, baud: e.spec.settings.baudRate }];
    const line = statuses[i]?.line;
    if (e.spec.kind === "virtual" && line) return [{ port: e.spec.port, baud: line.baudRate }];
    return [];
  });
  if (new Set(speeds.map((s) => s.baud)).size < 2) return null;
  return speeds.map((s) => `${s.port} ${s.baud}`).join(", ");
}

export function endpointSubtitle(e: HubEndpoint, status?: EndpointStatus): string {
  const s = e.spec;
  switch (s.kind) {
    case "serial":
      return serialSummary(s.settings);
    case "virtual":
      if (!status?.peerOpen) return "waiting for an app";
      return status.line ? `app · ${status.line.baudRate} ${status.line.framing}` : "app connected";
    case "tcpServer":
      return `listen ${s.bind}`;
    case "tcpClient":
      return "TCP client";
  }
}

export const HUB_META: Record<HubKind, { label: string; verb: string; color: string; describe: string }> = {
  split: {
    label: "Split",
    verb: "Split a port",
    color: "#22d3ee",
    describe: "Share one port with several applications. Each one gets its own virtual COM port.",
  },
  join: {
    label: "Join",
    verb: "Join ports",
    color: "#a78bfa",
    describe: "Merge several ports into one virtual COM port. Data from all of them arrives in one place.",
  },
  bridge: {
    label: "Bridge",
    verb: "Bridge two endpoints",
    color: "#34d399",
    describe: "Wire two endpoints together, e.g. a COM port and a TCP socket, or two COM ports.",
  },
  bus: {
    label: "Bus",
    verb: "Shared bus",
    color: "#fbbf24",
    describe: "Every endpoint hears every other one, like an RS-485 multidrop line.",
  },
};

export function hex(bytes: number[]): string {
  return bytes.map((b) => b.toString(16).padStart(2, "0")).join(" ");
}

export function ascii(bytes: number[]): string {
  return bytes
    .map((b) => (b === 13 ? "␍" : b === 10 ? "␊" : b >= 32 && b < 127 ? String.fromCharCode(b) : "·"))
    .join("");
}
