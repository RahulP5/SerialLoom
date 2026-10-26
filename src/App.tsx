import { useEffect } from "react";
import { Cable, CircleAlert, CircleCheck, Download, Info, List, LoaderCircle, RefreshCw, Settings, Workflow, X } from "lucide-react";
import clsx from "clsx";
import { useStore, type View } from "./store";
import { api } from "./lib/api";
import { TopologyView } from "./components/views/TopologyView";
import { PortsView, VirtualPortsView } from "./components/views/ListViews";
import { SettingsView } from "./components/views/SettingsView";
import { AboutView } from "./components/views/AboutView";
import { Inspector } from "./components/Inspector";
import { HubWizard } from "./components/dialogs/HubWizard";
import { AddMemberDialog, DiagnosticsDialog, PairDialog, VirtualPortDialog } from "./components/dialogs/SimpleDialogs";

const NAV: { id: View; label: string; icon: typeof Workflow }[] = [
  { id: "topology", label: "Topology", icon: Workflow },
  { id: "ports", label: "Ports", icon: List },
  { id: "virtual", label: "Virtual ports", icon: Cable },
  { id: "settings", label: "Settings", icon: Settings },
  { id: "about", label: "About", icon: Info },
];

export default function App() {
  const { view, init, refresh } = useStore();

  useEffect(() => {
    init();
    // Pick up ports that were plugged in or removed.
    const t = setInterval(() => {
      if (!useStore.getState().busy) refresh();
    }, 5000);
    return () => clearInterval(t);
  }, [init, refresh]);

  return (
    <div className="flex h-full">
      <Sidebar />
      <main className="flex min-w-0 flex-1 flex-col">
        <DriverBanner />
        <div className="flex min-h-0 flex-1">
          <div className="min-w-0 flex-1">
            {view === "topology" && <TopologyView />}
            {view === "ports" && <PortsView />}
            {view === "virtual" && <VirtualPortsView />}
            {view === "settings" && <SettingsView />}
            {view === "about" && <AboutView />}
          </div>
          {view === "topology" && <Inspector />}
        </div>
      </main>
      <Dialogs />
      <Toasts />
      <BusyOverlay />
    </div>
  );
}

function Sidebar() {
  const { view, setView, refresh, hubs, info } = useStore();
  const running = hubs.filter((h) => h.status.running).length;

  return (
    <nav className="flex w-[212px] shrink-0 flex-col border-r border-ink-700/70 bg-ink-950/70">
      <div className="flex items-center gap-2.5 px-4 pt-5 pb-6">
        <img src="/logo.svg" alt="" className="h-9 w-9" />
        <div>
          <div className="text-[15px] leading-tight font-semibold text-white">SerialLoom</div>
          <div className="text-[11px] text-ink-400">Virtual COM ports</div>
        </div>
      </div>
      <div className="space-y-0.5 px-2">
        {NAV.map((n) => (
          <button
            key={n.id}
            onClick={() => setView(n.id)}
            className={clsx(
              "flex w-full items-center gap-2.5 rounded-lg px-3 py-2 text-sm transition-colors",
              view === n.id ? "bg-ink-700/70 text-white" : "text-ink-300 hover:bg-ink-800 hover:text-ink-100",
            )}
          >
            <n.icon size={16} className={view === n.id ? "text-cyan-300" : ""} />
            {n.label}
          </button>
        ))}
      </div>
      <div className="mt-auto space-y-2 px-4 pb-4 text-[11px] text-ink-400">
        <div className="flex items-center gap-2">
          <span className={clsx("h-2 w-2 rounded-full", running ? "bg-emerald-400" : "bg-ink-500")} />
          {running} of {hubs.length} hub{hubs.length === 1 ? "" : "s"} running
        </div>
        <div className="flex items-center justify-between">
          <span>v{info?.version ?? "…"}</span>
          <button className="hover:text-ink-200" onClick={() => refresh()} title="Refresh ports">
            <RefreshCw size={13} />
          </button>
        </div>
      </div>
    </nav>
  );
}

function DriverBanner() {
  const { driver, settings, saveSettings, setView, run } = useStore();
  if (!driver) return null;
  if (driver.id === "demo") {
    return (
      <div className="flex items-center gap-2 border-b border-violet-500/30 bg-violet-500/10 px-4 py-2 text-xs text-violet-100">
        <Info size={14} className="shrink-0" />
        <span className="flex-1">{driver.message ?? "Demo mode: ports are simulated."}</span>
        <button className="underline hover:text-white" onClick={() => setView("settings")}>
          Change driver
        </button>
      </div>
    );
  }
  if (driver.available) return null;
  return (
    <div className="flex flex-wrap items-center gap-2 border-b border-amber-500/30 bg-amber-500/10 px-4 py-2 text-xs text-amber-100">
      <CircleAlert size={14} className="shrink-0" />
      <span className="flex-1">
        {driver.message ?? "The SerialLoom driver is not installed."} Bridging real ports to TCP works without it.
      </span>
      {driver.installable && (
        <button
          className="btn-outline border-amber-400/40 px-2 py-1 text-xs"
          onClick={() =>
            run("Installing driver (Windows asks for administrator permission)", () => api.installDriver(), "Driver installed")
          }
        >
          <Download size={13} /> Install driver
        </button>
      )}
      {settings && (
        <button
          className="btn-outline border-amber-400/40 px-2 py-1 text-xs"
          onClick={() => saveSettings({ ...settings, driver: "demo" })}
        >
          Try demo mode
        </button>
      )}
    </div>
  );
}

function Dialogs() {
  const { dialog, openDialog } = useStore();
  const close = () => openDialog(null);
  if (!dialog) return null;
  switch (dialog.type) {
    case "hub":
      return <HubWizard preset={dialog.preset} onClose={close} />;
    case "pair":
      return <PairDialog onClose={close} />;
    case "virtualPort":
      return <VirtualPortDialog onClose={close} />;
    case "diagnostics":
      return <DiagnosticsDialog onClose={close} />;
    case "addMember":
      return <AddMemberDialog hubId={dialog.hubId} preset={dialog.preset} onClose={close} />;
  }
}

function Toasts() {
  const { toasts, dismissToast } = useStore();
  return (
    <div className="pointer-events-none fixed right-4 bottom-4 z-[60] flex w-96 flex-col gap-2">
      {toasts.map((t) => (
        <div
          key={t.id}
          className={clsx(
            "panel pointer-events-auto flex items-start gap-2.5 px-3.5 py-3 text-sm shadow-2xl shadow-black/50",
            t.kind === "error" && "border-rose-500/50",
            t.kind === "success" && "border-emerald-500/40",
          )}
        >
          {t.kind === "error" ? (
            <CircleAlert size={16} className="mt-0.5 shrink-0 text-rose-400" />
          ) : t.kind === "success" ? (
            <CircleCheck size={16} className="mt-0.5 shrink-0 text-emerald-400" />
          ) : (
            <Info size={16} className="mt-0.5 shrink-0 text-cyan-400" />
          )}
          <span className="selectable flex-1 break-words text-ink-100">{t.text}</span>
          <button className="text-ink-400 hover:text-white" onClick={() => dismissToast(t.id)} aria-label="Dismiss">
            <X size={14} />
          </button>
        </div>
      ))}
    </div>
  );
}

function BusyOverlay() {
  const busy = useStore((s) => s.busy);
  if (!busy) return null;
  return (
    <div className="pointer-events-none fixed bottom-4 left-1/2 z-[60] -translate-x-1/2">
      <div className="panel flex items-center gap-2 px-4 py-2 text-sm text-ink-100 shadow-xl">
        <LoaderCircle size={15} className="animate-spin text-cyan-300" />
        {busy}
      </div>
    </div>
  );
}
