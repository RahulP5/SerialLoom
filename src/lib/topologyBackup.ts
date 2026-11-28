import { invoke } from "@tauri-apps/api/core";
import { t } from "../i18n";
import { api, errorText, inTauri } from "./api";
import { HUB_KINDS } from "./format";
import { REPLAY_FORMATS, isGps } from "./types";
import type { ReplayFormat, CreateHubRequest, EndpointSpec, HubKind, HubSpec } from "./types";

type Position = { x: number; y: number };
type SavedHub = Omit<CreateHubRequest, "start" | "master" | "members"> & {
  id: string;
  master: EndpointSpec;
  members: EndpointSpec[];
};

function savedHub(spec: HubSpec): SavedHub {
  return {
    id: spec.id, name: spec.name, kind: spec.kind, autostart: spec.autostart,
    master: spec.master.spec, members: spec.members.map(({ spec }) => spec),
  };
}

export async function exportTopology(
  positions: Record<string, Position>,
  mirroredHubs: ReadonlySet<string>,
): Promise<boolean> {
  const [hubs, ports] = await Promise.all([api.listHubs(), api.listVirtualPorts()]);
  const text = JSON.stringify({
    format: "serialloom.topology",
    version: 1,
    hubs: hubs.map(({ spec }) => savedHub(spec)),
    virtualPorts: ports.map(({ name }) => name),
    positions,
    mirroredHubs: hubs.flatMap(({ spec }) => mirroredHubs.has(spec.id) ? [spec.id] : []),
  }, null, 2) + "\n";

  if (inTauri) return invoke<boolean>("save_topology_backup", { text });

  const url = URL.createObjectURL(new Blob([text], { type: "application/json" }));
  const link = document.createElement("a");
  link.href = url;
  link.download = "serialloom-topology.json";
  link.click();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
  return true;
}

function invalid(): never {
  throw new Error(t("topology.backupInvalid"));
}

function record(value: unknown): Record<string, unknown> {
  if (!value || typeof value !== "object" || Array.isArray(value)) return invalid();
  return value as Record<string, unknown>;
}

function textValue(value: unknown): string {
  if (typeof value !== "string" || !value.trim()) return invalid();
  return value.trim();
}

function comName(value: unknown): string {
  const name = textValue(value).toUpperCase();
  if (!/^COM[1-9][0-9]*$/.test(name)) return invalid();
  return name;
}

function endpoint(value: unknown): EndpointSpec {
  const e = record(value);
  switch (e.kind) {
    case "virtual": return { kind: e.kind, port: comName(e.port) };
    case "tcpServer": return { kind: e.kind, bind: textValue(e.bind) };
    case "tcpClient": return { kind: e.kind, address: textValue(e.address) };
    case "udp": {
      const { bind, destination, receive, framing } = record(e.settings);
      if (typeof destination !== "string" || typeof receive !== "boolean" ||
          (framing !== "raw" && framing !== "nmea0183") ||
          (!receive && !destination.trim())) return invalid();
      return { kind: "udp", settings: { bind: textValue(bind), destination: destination.trim(), receive, framing } };
    }
    case "replay": {
      const settings = record(e.settings);
      const { format, repeat, timestamped, bytesPerSecond, cycleMs, linesPerCycle } = settings;
      if (!REPLAY_FORMATS.includes(format as ReplayFormat) ||
          typeof repeat !== "boolean" || typeof timestamped !== "boolean" ||
          (format === "file" && timestamped)) return invalid();
      const rate = (value: unknown, max: number): number => {
        if (typeof value !== "number" || !Number.isInteger(value) || value < 1 || value > max) return invalid();
        return value;
      };
      const simulation = isGps(format as ReplayFormat);
      let gps;
      if (simulation) {
        const config = record(settings.gps);
        const field = (key: string, min: number, max: number) => {
          const value = textValue(config[key]);
          const number = Number(value);
          if (!/^[+-]?(?:\d+\.?\d*|\.\d+)(?:e[+-]?\d+)?$/i.test(value) ||
              !Number.isFinite(number) || number < min || number > max) return invalid();
          return value;
        };
        gps = { latitude: field("latitude", -90, 90), longitude: field("longitude", -180, 180),
          course: field("course", 0, 360), speed: field("speed", 0, 100) };
      }
      return { kind: e.kind, settings: {
        path: simulation ? "" : textValue(settings.path), format: format as ReplayFormat, repeat, timestamped,
        ...(gps ? { gps } : {}),
        bytesPerSecond: rate(bytesPerSecond, 1_000_000),
        cycleMs: rate(cycleMs, 60_000),
        linesPerCycle: rate(linesPerCycle, 256),
      } };
    }
    case "serial": {
      const s = record(e.settings);
      const { baudRate, dataBits, parity, stopBits, flowControl } = s;
      if (typeof baudRate !== "number" || !Number.isInteger(baudRate) || baudRate <= 0 || baudRate > 12_000_000 ||
          typeof dataBits !== "number" || ![5, 6, 7, 8].includes(dataBits) ||
          (parity !== "none" && parity !== "odd" && parity !== "even") ||
          (stopBits !== "one" && stopBits !== "two") ||
          (flowControl !== "none" && flowControl !== "software" && flowControl !== "hardware")) return invalid();
      return { kind: e.kind, port: comName(e.port), settings: { baudRate, dataBits, parity, stopBits, flowControl } };
    }
    default: return invalid();
  }
}

function parseBackup(text: string) {
  let json: unknown;
  try { json = JSON.parse(text); } catch { return invalid(); }
  const backup = record(json);
  if (backup.format !== "serialloom.topology" || backup.version !== 1 ||
      !Array.isArray(backup.hubs) || !Array.isArray(backup.virtualPorts)) return invalid();

  const hubs: SavedHub[] = backup.hubs.map((value: unknown) => {
    const h = record(value);
    const kind = h.kind as HubKind;
    if (!HUB_KINDS.includes(kind) || typeof h.autostart !== "boolean" ||
        !Array.isArray(h.members) || h.members.length < 1 || h.members.length > 32 ||
        (kind === "bridge" && h.members.length !== 1)) return invalid();
    return {
      id: textValue(h.id), name: textValue(h.name), kind, autostart: h.autostart,
      master: endpoint(h.master), members: h.members.map(endpoint),
    };
  });
  if (new Set(hubs.map((h) => h.id)).size !== hubs.length) return invalid();
  const virtualPorts = new Set(backup.virtualPorts.map(comName));
  for (const hub of hubs) {
    for (const ep of [hub.master, ...hub.members]) {
      if (ep.kind === "virtual") virtualPorts.add(ep.port);
    }
  }
  const positions = record(backup.positions);
  for (const value of Object.values(positions)) {
    const { x, y } = record(value);
    if (typeof x !== "number" || !Number.isFinite(x) || typeof y !== "number" || !Number.isFinite(y)) return invalid();
  }
  let mirroredHubs: Set<string> | undefined;
  if (backup.mirroredHubs !== undefined) {
    if (!Array.isArray(backup.mirroredHubs)) return invalid();
    mirroredHubs = new Set(backup.mirroredHubs.map(textValue));
    const hubIds = new Set(hubs.map(({ id }) => id));
    for (const id of mirroredHubs) {
      if (!hubIds.has(id)) return invalid();
    }
  }
  return { hubs, virtualPorts, positions: positions as Record<string, Position>, mirroredHubs };
}

function signature({ name, kind, autostart, master, members }: SavedHub) {
  return JSON.stringify({ name, kind, autostart, master: endpoint(master), members: members.map(endpoint) });
}

export async function importTopology(text: string) {
  const backup = parseBackup(text);
  const [hubs, ports, virtualPorts] = await Promise.all([api.listHubs(), api.listPorts(), api.listVirtualPorts()]);
  const existing = hubs.map(({ spec }) => savedHub(spec));
  const virtualNames = new Set(virtualPorts.map(({ name }) => name.toUpperCase()));
  const physicalNames = new Set(ports.filter(({ kind }) => kind !== "virtual").map(({ name }) => name.toUpperCase()));

  const matched = new Set<string>();
  const plan = backup.hubs.map((hub) => {
    const key = signature(hub);
    const exact = existing.find((item) => !matched.has(item.id) && signature(item) === key);
    if (exact) matched.add(exact.id);
    if (!exact && existing.some((item) => item.name === hub.name && signature(item) !== key)) {
      throw new Error(t("topology.backupHubConflict", { name: hub.name }));
    }
    return { hub, existingId: exact?.id };
  });
  for (const name of backup.virtualPorts) {
    if (!virtualNames.has(name) && physicalNames.has(name)) throw new Error(t("topology.backupPortConflict", { port: name }));
  }

  const createdPorts: string[] = [];
  const createdHubs: string[] = [];
  const restoredPositions: Record<string, Position> = {};
  const restoredMirroring = new Map<string, boolean>();
  try {
    // Create one port at a time so a partial driver failure can be rolled back.
    for (const name of backup.virtualPorts) {
      if (virtualNames.has(name)) continue;
      const [port] = await api.createVirtualPorts([name]);
      if (port && !virtualNames.has(port.name.toUpperCase())) createdPorts.push(port.name);
      if (port?.name.toUpperCase() !== name) throw new Error(t("topology.backupPortConflict", { port: name }));
    }
    for (const { hub, existingId } of plan) {
      const { id: savedId, ...request } = hub;
      let id = existingId;
      if (!id) {
        const created = await api.createHub({ ...request, start: false });
        id = created.spec.id;
        createdHubs.push(id);
      }
      if (backup.mirroredHubs) restoredMirroring.set(id, backup.mirroredHubs.has(savedId));
      for (const suffix of ["", ...Array.from({ length: hub.members.length + 1 }, (_, i) => `:${i}`)]) {
        const position = backup.positions[savedId + suffix];
        if (position) restoredPositions[id + suffix] = position;
      }
    }
  } catch (error) {
    const cleanupErrors: string[] = [];
    for (const id of createdHubs.reverse()) {
      try { await api.deleteHub(id, false); } catch (e) { cleanupErrors.push(errorText(e)); }
    }
    for (const port of createdPorts.reverse()) {
      try { await api.removeVirtualPort(port); } catch (e) { cleanupErrors.push(errorText(e)); }
    }
    if (cleanupErrors.length) throw new Error([errorText(error), ...cleanupErrors].join("\n"));
    throw error;
  }
  return { positions: restoredPositions, mirroring: restoredMirroring };
}
