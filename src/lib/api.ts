import { invoke, isTauri } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { mockApi } from "./mock";
import type {
  AppInfo,
  AppSettings,
  CreateHubRequest,
  DriverInfo,
  EndpointRequest,
  HubStatus,
  HubView,
  MonitorRecord,
  PortInfo,
  VirtualPortInfo,
} from "./types";

/** Typed wrapper over the Tauri commands in src-tauri/src/commands.rs. */
export interface Api {
  appInfo(): Promise<AppInfo>;
  getSettings(): Promise<AppSettings>;
  setSettings(settings: AppSettings): Promise<DriverInfo>;
  driverInfo(): Promise<DriverInfo>;
  listPorts(): Promise<PortInfo[]>;
  installDriver(): Promise<DriverInfo>;
  diagnostics(): Promise<string>;
  listVirtualPorts(): Promise<VirtualPortInfo[]>;
  createVirtualPorts(names: string[]): Promise<VirtualPortInfo[]>;
  renameVirtualPort(name: string, newName: string): Promise<VirtualPortInfo>;
  removeVirtualPort(name: string): Promise<void>;
  listHubs(): Promise<HubView[]>;
  createHub(request: CreateHubRequest): Promise<HubView>;
  startHub(id: string): Promise<HubView>;
  stopHub(id: string): Promise<HubView>;
  updateHub(id: string, name?: string, autostart?: boolean): Promise<HubView>;
  deleteHub(id: string, removeVirtualPorts: boolean): Promise<void>;
  addMember(id: string, request: EndpointRequest): Promise<HubView>;
  removeMember(id: string, index: number, removeVirtualPort: boolean): Promise<HubView>;
  hubMonitor(id: string, after: number): Promise<MonitorRecord[]>;
  onStatus(cb: (status: HubStatus[]) => void): Promise<UnlistenFn>;
}

const tauriApi: Api = {
  appInfo: () => invoke("app_info"),
  getSettings: () => invoke("get_settings"),
  setSettings: (settings) => invoke("set_settings", { settings }),
  driverInfo: () => invoke("driver_info"),
  listPorts: () => invoke("list_ports"),
  installDriver: () => invoke("install_driver"),
  diagnostics: () => invoke("diagnostics"),
  listVirtualPorts: () => invoke("list_virtual_ports"),
  createVirtualPorts: (names) => invoke("create_virtual_ports", { names }),
  renameVirtualPort: (name, newName) => invoke("rename_virtual_port", { name, newName }),
  removeVirtualPort: (name) => invoke("remove_virtual_port", { name }),
  listHubs: () => invoke("list_hubs"),
  createHub: (request) => invoke("create_hub", { request }),
  startHub: (id) => invoke("start_hub", { id }),
  stopHub: (id) => invoke("stop_hub", { id }),
  updateHub: (id, name, autostart) => invoke("update_hub", { id, name, autostart }),
  deleteHub: (id, removeVirtualPorts) => invoke("delete_hub", { id, removeVirtualPorts }),
  addMember: (id, request) => invoke("add_member", { id, request }),
  removeMember: (id, index, removeVirtualPort) =>
    invoke("remove_member", { id, index, removeVirtualPort }),
  hubMonitor: (id, after) => invoke("hub_monitor", { id, after }),
  onStatus: (cb) => listen<HubStatus[]>("hub-status", (e) => cb(e.payload)),
};

/** True inside the desktop app; false in a plain browser (`npm run dev`). */
export const inTauri = isTauri();

/** In a plain browser the UI runs against an in-memory mock so it can be developed without Rust. */
export const api: Api = inTauri ? tauriApi : mockApi;

export async function openExternal(url: string) {
  if (!inTauri) {
    window.open(url, "_blank", "noopener");
    return;
  }
  const { openUrl } = await import("@tauri-apps/plugin-opener");
  await openUrl(url);
}

export function errorText(e: unknown): string {
  if (typeof e === "string") return e;
  if (e instanceof Error) return e.message;
  return JSON.stringify(e);
}
