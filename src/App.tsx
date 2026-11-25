import { useCallback, useEffect, useState } from "react";
import {
  Cable,
  CircleAlert,
  CircleArrowUp,
  CircleCheck,
  Download,
  Info,
  List,
  LoaderCircle,
  Moon,
  PanelLeftClose,
  PanelLeftOpen,
  RefreshCw,
  Settings,
  Sun,
  Workflow,
  X,
} from "lucide-react";
import clsx from "clsx";
import { useStore, type View } from "./store";
import { api, openExternal } from "./lib/api";
import { useTheme } from "./lib/theme";
import { useUpdateChecks, useUpdates } from "./lib/updates";
import { WEBSITE } from "./lib/project";
import { message, t, useI18n, useT, type MessageKey } from "./i18n";
import { TopologyView } from "./components/views/TopologyView";
import { PortsView, VirtualPortsView } from "./components/views/ListViews";
import { SettingsView } from "./components/views/SettingsView";
import { AboutView } from "./components/views/AboutView";
import { Inspector } from "./components/Inspector";
import { HubWizard } from "./components/dialogs/HubWizard";
import { AddMemberDialog, DiagnosticsDialog, PairDialog, VirtualPortDialog } from "./components/dialogs/SimpleDialogs";

const NAV: { id: View; label: MessageKey; icon: typeof Workflow }[] = [
  { id: "topology", label: "nav.topology", icon: Workflow },
  { id: "ports", label: "nav.ports", icon: List },
  { id: "virtual", label: "nav.virtual", icon: Cable },
  { id: "settings", label: "nav.settings", icon: Settings },
  { id: "about", label: "nav.about", icon: Info },
];

/** Plural forms of a message as the tray needs them: one hub, or several. */
function trayForms(key: MessageKey) {
  const m = message(key);
  if (typeof m === "string") return { one: m, other: m };
  return { one: m.one ?? m.other, other: m.other };
}

/** Puts the notification-area menu in the app's language. */
function useTrayLanguage() {
  const lang = useI18n((s) => s.lang);
  useEffect(() => {
    const hubs = trayForms("tray.hubsRunning");
    api
      .setTrayText({
        open: t("tray.open"),
        quit: t("tray.quit"),
        starting: t("tray.starting"),
        noHubs: t("tray.noHubs"),
        one: hubs.one,
        other: hubs.other,
      })
      .catch(() => {
        // An older backend keeps its English menu.
      });
  }, [lang]);
}

export default function App() {
  const { view, init, refresh } = useStore();
  // Re-render everything when the language changes.
  useI18n((s) => s.lang);
  useTrayLanguage();

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

const COLLAPSED_KEY = "serialloom.sidebar.collapsed";

function loadCollapsed() {
  try {
    return localStorage.getItem(COLLAPSED_KEY) === "1";
  } catch {
    return false;
  }
}

/** The name of an icon-only control, shown at once beside it on hover or keyboard focus. */
function Tip({ text, show }: { text: string; show: boolean }) {
  if (!show) return null;
  return (
    <span
      role="tooltip"
      className="pointer-events-none absolute top-1/2 left-full z-50 ml-3 -translate-y-1/2 rounded-md bg-ink-0 px-2.5 py-1.5 text-xs font-medium whitespace-nowrap text-ink-950 opacity-0 shadow-lg transition-opacity duration-100 group-hover:opacity-100 group-focus-visible:opacity-100"
    >
      {text}
    </span>
  );
}

function Sidebar() {
  const { view, setView, refresh, hubs, info } = useStore();
  const running = hubs.filter((h) => h.status.running).length;
  const [collapsed, setCollapsed] = useState(loadCollapsed);
  const t = useT();
  useUpdateChecks(info?.version);

  const toggle = useCallback(
    () =>
      setCollapsed((was) => {
        try {
          localStorage.setItem(COLLAPSED_KEY, was ? "0" : "1");
        } catch {
          // Remembering the choice is a convenience.
        }
        return !was;
      }),
    [],
  );

  // Ctrl+B opens and closes the panel, as in most editors.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.ctrlKey && !e.shiftKey && !e.altKey && !e.metaKey && e.key.toLowerCase() === "b") {
        e.preventDefault();
        toggle();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [toggle]);

  const hubText = t("sidebar.hubsRunning", { running, count: hubs.length });
  const ToggleIcon = collapsed ? PanelLeftOpen : PanelLeftClose;
  const toggleText = collapsed ? t("sidebar.expand") : t("sidebar.collapse");
  const item = "group relative flex w-full items-center gap-2.5 rounded-lg py-2 text-sm transition-colors";

  return (
    <nav
      className={clsx(
        "flex shrink-0 flex-col border-r border-ink-700/70 bg-ink-950/70 transition-[width] duration-200",
        collapsed ? "w-16" : "w-[212px]",
      )}
    >
      <div className={clsx("flex items-center gap-2.5 pt-5 pb-6", collapsed ? "justify-center" : "px-4")}>
        <div className="group relative shrink-0">
          <img src="/logo.svg" alt="SerialLoom" className="h-9 w-9" />
          <Tip text="SerialLoom" show={collapsed} />
        </div>
        {!collapsed && (
          <div className="min-w-0">
            <div className="text-[15px] leading-tight font-semibold text-ink-0">SerialLoom</div>
            <div className="text-[11px] whitespace-nowrap text-ink-400">{t("sidebar.tagline")}</div>
          </div>
        )}
      </div>
      <div className="space-y-0.5 px-2">
        {NAV.map((n) => (
          <button
            key={n.id}
            onClick={() => setView(n.id)}
            aria-current={view === n.id ? "page" : undefined}
            aria-label={collapsed ? t(n.label) : undefined}
            className={clsx(
              item,
              collapsed ? "justify-center px-0" : "px-3",
              view === n.id ? "bg-ink-700/70 text-ink-0" : "text-ink-300 hover:bg-ink-800 hover:text-ink-100",
            )}
          >
            <n.icon size={collapsed ? 18 : 16} className={clsx("shrink-0", view === n.id && "text-cyan-300")} />
            {!collapsed && <span className="whitespace-nowrap">{t(n.label)}</span>}
            <Tip text={t(n.label)} show={collapsed} />
          </button>
        ))}
      </div>
      <div className="mt-auto px-2 pb-1">
        <button
          onClick={toggle}
          aria-label={toggleText}
          aria-expanded={!collapsed}
          title={collapsed ? undefined : toggleText}
          className={clsx(
            item,
            "text-ink-400 hover:bg-ink-800 hover:text-ink-100",
            collapsed ? "justify-center px-0" : "px-3",
          )}
        >
          <ToggleIcon size={collapsed ? 18 : 16} className="shrink-0" />
          {!collapsed && <span className="whitespace-nowrap">{t("sidebar.collapseShort")}</span>}
          <Tip text={toggleText} show={collapsed} />
        </button>
      </div>
      {collapsed ? (
        <div className="flex flex-col items-center gap-3 pt-2 pb-4 text-ink-400">
          <UpdateNotice compact />
          <div className="group relative flex h-4 items-center" aria-label={hubText} role="status">
            <span className={clsx("h-2 w-2 rounded-full", running ? "bg-emerald-400" : "bg-ink-500")} />
            <Tip text={`${hubText} · v${info?.version ?? "…"}`} show />
          </div>
          <div className="group relative flex">
            <ThemeToggle />
          </div>
          <button className="group relative hover:text-ink-200" onClick={() => refresh()} aria-label={t("sidebar.refresh")}>
            <RefreshCw size={13} />
            <Tip text={t("sidebar.refresh")} show />
          </button>
        </div>
      ) : (
        <div className="space-y-2 px-4 pt-2 pb-4 text-[11px] text-ink-400">
          <UpdateNotice />
          <div className="flex items-center gap-2 whitespace-nowrap">
            <span className={clsx("h-2 w-2 rounded-full", running ? "bg-emerald-400" : "bg-ink-500")} />
            {hubText}
          </div>
          <div className="flex items-center justify-between">
            <span>v{info?.version ?? "…"}</span>
            <div className="flex items-center gap-2.5">
              <ThemeToggle />
              <button className="hover:text-ink-200" onClick={() => refresh()} title={t("sidebar.refresh")}>
                <RefreshCw size={13} />
              </button>
            </div>
          </div>
        </div>
      )}
    </nav>
  );
}

function UpdateNotice({ compact = false }: { compact?: boolean }) {
  const { available, dismiss } = useUpdates();
  const t = useT();
  if (!available) return null;
  if (compact) {
    const text = t("update.availableLink", { version: available.version });
    return (
      <button
        className="group relative rounded-md p-1 text-cyan-300 hover:bg-cyan-500/10"
        onClick={() => openExternal(WEBSITE.download)}
        aria-label={text}
      >
        <CircleArrowUp size={16} />
        <Tip text={text} show />
      </button>
    );
  }
  return (
    <div className="-mx-1 mb-1 rounded-lg border border-cyan-500/30 bg-cyan-500/10 p-2.5" role="status">
      <div className="flex items-start gap-2">
        <CircleArrowUp size={14} className="mt-px shrink-0 text-cyan-300" />
        <span className="flex-1 text-xs leading-snug font-medium text-ink-0">
          {t("update.available", { version: available.version })}
        </span>
        <button
          className="text-ink-400 hover:text-ink-0"
          onClick={dismiss}
          aria-label={t("common.dismiss")}
          title={t("update.hide")}
        >
          <X size={13} />
        </button>
      </div>
      <div className="mt-1.5 flex gap-3 pl-[22px]">
        <button className="font-medium text-cyan-300 hover:underline" onClick={() => openExternal(WEBSITE.download)}>
          {t("update.download")}
        </button>
        <button className="text-ink-300 hover:text-ink-100 hover:underline" onClick={() => openExternal(available.notesUrl)}>
          {t("update.whatsNew")}
        </button>
      </div>
    </div>
  );
}

function ThemeToggle() {
  const { resolved, setPref } = useTheme();
  const t = useT();
  const next = resolved === "dark" ? "light" : "dark";
  return (
    <button
      className="hover:text-ink-200"
      onClick={() => setPref(next)}
      title={next === "light" ? t("theme.toLight") : t("theme.toDark")}
    >
      {resolved === "dark" ? <Sun size={13} /> : <Moon size={13} />}
    </button>
  );
}

function DriverBanner() {
  const { driver, settings, saveSettings, setView, run } = useStore();
  const t = useT();
  if (!driver) return null;
  if (driver.id === "demo") {
    return (
      <div className="flex items-center gap-2 border-b border-violet-500/30 bg-violet-500/10 px-4 py-2 text-xs text-violet-100">
        <Info size={14} className="shrink-0" />
        <span className="flex-1">{t("driver.demoMessage")}</span>
        <button className="underline hover:text-ink-0" onClick={() => setView("settings")}>
          {t("driver.change")}
        </button>
      </div>
    );
  }
  if (driver.available) return null;
  return (
    <div className="flex flex-wrap items-center gap-2 border-b border-amber-500/30 bg-amber-500/10 px-4 py-2 text-xs text-amber-100">
      <CircleAlert size={14} className="shrink-0" />
      <span className="flex-1">
        {driver.message ?? t("driver.notInstalled")} {t("driver.tcpWorks")}
      </span>
      {driver.installable && (
        <button
          className="btn-outline border-amber-400/40 px-2 py-1 text-xs"
          onClick={() =>
            run(
              t("common.adminNote", { action: t("driver.installing") }),
              () => api.installDriver(),
              t("driver.installed"),
            )
          }
        >
          <Download size={13} /> {t("driver.install")}
        </button>
      )}
      {settings && (
        <button
          className="btn-outline border-amber-400/40 px-2 py-1 text-xs"
          onClick={() => saveSettings({ ...settings, driver: "demo" })}
        >
          {t("driver.tryDemo")}
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
  const t = useT();
  return (
    <div className="pointer-events-none fixed right-4 bottom-4 z-[60] flex w-96 flex-col gap-2">
      {toasts.map((toast) => (
        <div
          key={toast.id}
          className={clsx(
            "panel pointer-events-auto flex items-start gap-2.5 px-3.5 py-3 text-sm shadow-2xl shadow-black/50",
            toast.kind === "error" && "border-rose-500/50",
            toast.kind === "success" && "border-emerald-500/40",
          )}
        >
          {toast.kind === "error" ? (
            <CircleAlert size={16} className="mt-0.5 shrink-0 text-rose-400" />
          ) : toast.kind === "success" ? (
            <CircleCheck size={16} className="mt-0.5 shrink-0 text-emerald-400" />
          ) : (
            <Info size={16} className="mt-0.5 shrink-0 text-cyan-400" />
          )}
          <span className="selectable flex-1 break-words text-ink-100">{toast.text}</span>
          <button className="text-ink-400 hover:text-ink-0" onClick={() => dismissToast(toast.id)} aria-label={t("common.dismiss")}>
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
