import { create } from "zustand";
import { api, errorText } from "./lib/api";
import type {
  AppInfo,
  AppSettings,
  CreateHubRequest,
  DriverInfo,
  EndpointRequest,
  HubStatus,
  HubView,
  PortInfo,
  VirtualPortInfo,
} from "./lib/types";

export type View = "topology" | "ports" | "virtual" | "settings" | "about";

export type Selection = { type: "hub"; id: string } | null;

export interface Rate {
  rx: number;
  tx: number;
}

export interface Toast {
  id: number;
  kind: "info" | "error" | "success";
  text: string;
}

/** Dialogs that can be opened from anywhere (canvas, tables, toolbar). */
export type DialogState =
  | { type: "hub"; preset?: Partial<CreateHubRequest> }
  | { type: "pair" }
  | { type: "virtualPort" }
  | { type: "diagnostics" }
  | { type: "addMember"; hubId: string; preset?: EndpointRequest }
  | null;

interface State {
  view: View;
  info?: AppInfo;
  driver?: DriverInfo;
  settings?: AppSettings;
  ports: PortInfo[];
  virtualPorts: VirtualPortInfo[];
  hubs: HubView[];
  rates: Record<string, Rate[]>;
  selection: Selection;
  dialog: DialogState;
  busy: string | null;
  toasts: Toast[];

  setView(v: View): void;
  select(s: Selection): void;
  openDialog(d: DialogState): void;
  toast(kind: Toast["kind"], text: string): void;
  dismissToast(id: number): void;

  init(): Promise<void>;
  refresh(): Promise<void>;
  applyStatus(list: HubStatus[]): void;
  /** Runs an action with a busy indicator and error toast; returns undefined on failure. */
  run<T>(label: string, fn: () => Promise<T>, success?: string): Promise<T | undefined>;
  saveSettings(s: AppSettings): Promise<void>;
}

let lastStatus: { at: number; byHub: Record<string, HubStatus> } = { at: 0, byHub: {} };
let toastId = 0;
let initialized = false;

export const useStore = create<State>((set, get) => ({
  view: "topology",
  ports: [],
  virtualPorts: [],
  hubs: [],
  rates: {},
  selection: null,
  dialog: null,
  busy: null,
  toasts: [],

  setView: (view) => set({ view }),
  select: (selection) => set({ selection }),
  openDialog: (dialog) => set({ dialog }),

  toast(kind, text) {
    const id = ++toastId;
    set((s) => ({ toasts: [...s.toasts, { id, kind, text }] }));
    setTimeout(() => get().dismissToast(id), kind === "error" ? 9000 : 4000);
  },
  dismissToast: (id) => set((s) => ({ toasts: s.toasts.filter((t) => t.id !== id) })),

  async init() {
    // React StrictMode mounts twice in development; subscribe only once.
    if (initialized) return;
    initialized = true;
    try {
      const [info, settings] = await Promise.all([api.appInfo(), api.getSettings()]);
      set({ info, settings });
    } catch (e) {
      get().toast("error", errorText(e));
    }
    await get().refresh();
    await api.onStatus((list) => get().applyStatus(list));
  },

  async refresh() {
    try {
      const [driver, ports, virtualPorts, hubs] = await Promise.all([
        api.driverInfo(),
        api.listPorts(),
        api.listVirtualPorts().catch(() => [] as VirtualPortInfo[]),
        api.listHubs(),
      ]);
      set({ driver, ports, virtualPorts, hubs });
    } catch (e) {
      get().toast("error", errorText(e));
    }
  },

  applyStatus(list) {
    const now = performance.now();
    const dt = (now - lastStatus.at) / 1000;
    const rates: Record<string, Rate[]> = {};
    for (const s of list) {
      const prev = lastStatus.byHub[s.id];
      rates[s.id] = s.endpoints.map((e, i) => {
        const p = prev?.endpoints[i];
        if (!p || dt <= 0 || dt > 5) return { rx: 0, tx: 0 };
        return {
          rx: Math.max(0, (e.rxBytes - p.rxBytes) / dt),
          tx: Math.max(0, (e.txBytes - p.txBytes) / dt),
        };
      });
    }
    lastStatus = { at: now, byHub: Object.fromEntries(list.map((s) => [s.id, s])) };
    set((st) => ({
      rates,
      hubs: st.hubs.map((h) => {
        const s = lastStatus.byHub[h.spec.id];
        return s && s.endpoints.length === h.status.endpoints.length ? { ...h, status: s } : h;
      }),
    }));
  },

  async run(label, fn, success) {
    set({ busy: label });
    try {
      const r = await fn();
      if (success) get().toast("success", success);
      await get().refresh();
      return r;
    } catch (e) {
      get().toast("error", errorText(e));
      return undefined;
    } finally {
      set({ busy: null });
    }
  },

  async saveSettings(s) {
    await get().run(
      "Saving settings",
      async () => {
        const driver = await api.setSettings(s);
        set({ settings: s, driver });
      },
      "Settings saved",
    );
  },
}));
