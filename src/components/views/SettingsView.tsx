import { useEffect, useState } from "react";
import {
  CircleAlert,
  CircleArrowUp,
  CircleCheck,
  Monitor,
  Moon,
  PanelBottom,
  ShieldCheck,
  SquareTerminal,
  Sun,
} from "lucide-react";
import clsx from "clsx";
import { useStore } from "../../store";
import type { AppSettings, AutomationInfo, DriverKind } from "../../lib/types";
import { api, errorText } from "../../lib/api";
import { DriverMissing } from "./ListViews";
import { useTheme, type ThemePref } from "../../lib/theme";
import { useUpdates } from "../../lib/updates";

const THEMES: { id: ThemePref; label: string; icon: typeof Sun }[] = [
  { id: "system", label: "Same as Windows", icon: Monitor },
  { id: "light", label: "Light", icon: Sun },
  { id: "dark", label: "Dark", icon: Moon },
];

export function SettingsView() {
  const { settings, driver, saveSettings, busy } = useStore();
  const [draft, setDraft] = useState<AppSettings | undefined>(settings);
  useEffect(() => setDraft(settings), [settings]);
  if (!draft) return null;

  const dirty = JSON.stringify(draft) !== JSON.stringify(settings);
  const setDriver = (d: DriverKind) => setDraft({ ...draft, driver: d });

  return (
    <div className="h-full overflow-y-auto px-8 py-7">
      <div className="mx-auto max-w-3xl space-y-6">
        <div>
          <h1 className="text-xl font-semibold text-ink-0">Settings</h1>
          <p className="mt-1 text-sm text-ink-300">
            Appearance, running in the background, updates, administrator permission, automation, and where virtual
            ports come from.
          </p>
        </div>

        <Appearance />

        <section className="panel p-5">
          <h2 className="flex items-center gap-2 font-semibold text-ink-0">
            <PanelBottom size={16} className="text-cyan-300" /> Running in the background
          </h2>
          <p className="mt-1.5 text-sm text-ink-300">
            Closing the window keeps SerialLoom running in the notification area (the arrow next to the clock), so
            virtual ports stay open for your programs. Click the SerialLoom icon there to open the window again.
          </p>
          <p className="mt-1.5 text-sm text-ink-300">
            To stop all ports and exit, right-click the icon and choose <b className="text-ink-100">Quit SerialLoom</b>.
          </p>
          <label className="mt-3 flex items-center gap-2 text-sm text-ink-100">
            <input
              type="checkbox"
              checked={draft.startWithWindows}
              onChange={(e) => setDraft({ ...draft, startWithWindows: e.target.checked })}
              className="accent-loom-500"
            />
            Start SerialLoom in the background when I sign in to Windows
          </label>
        </section>

        <Updates />

        <AdminPermission />

        <Automation draft={draft} setDraft={setDraft} />

        <section className="panel p-5">
          <h2 className="font-semibold text-ink-0">Virtual port driver</h2>
          <p className="mt-1 text-sm text-ink-300">
            Choose how SerialLoom creates COM ports. Everything needed is included with SerialLoom.
          </p>
          <div className="mt-4 grid gap-3 sm:grid-cols-3">
            <DriverCard
              active={draft.driver === "usb"}
              onClick={() => setDriver("usb")}
              title="Virtual USB serial (recommended)"
              text="Ports appear as USB serial devices using Windows' own driver. Works on any PC. Ports exist while SerialLoom runs."
            />
            <DriverCard
              active={draft.driver === "native"}
              onClick={() => setDriver("native")}
              title="SerialLoom driver"
              text="SerialLoom's own port driver. Needs a Microsoft signature to load, so it only works on test-mode PCs for now."
            />
            <DriverCard
              active={draft.driver === "demo"}
              onClick={() => setDriver("demo")}
              title="Demo (simulated)"
              text="Try the interface without installing anything. Ports exist only inside SerialLoom."
            />
          </div>

          {driver && (
            <div
              className={clsx(
                "mt-4 flex gap-2 rounded-lg border px-3 py-2.5 text-sm",
                driver.available
                  ? "border-emerald-500/30 bg-emerald-500/10 text-emerald-100"
                  : "border-amber-500/30 bg-amber-500/10 text-amber-100",
              )}
            >
              {driver.available ? (
                <CircleCheck size={16} className="mt-0.5 shrink-0" />
              ) : (
                <CircleAlert size={16} className="mt-0.5 shrink-0" />
              )}
              <div className="min-w-0">
                <div className="font-medium">
                  {driver.name}: {driver.available ? "ready" : "not installed"}
                </div>
                {driver.location && <div className="truncate font-mono text-xs opacity-80">{driver.location}</div>}
              </div>
            </div>
          )}
          <div className="mt-4">
            <DriverMissing />
          </div>
        </section>

        <div className="flex justify-end gap-2">
          <button className="btn-ghost" disabled={!dirty} onClick={() => setDraft(settings)}>
            Discard
          </button>
          <button className="btn-primary" disabled={!dirty || !!busy} onClick={() => saveSettings(draft)}>
            Save settings
          </button>
        </div>
      </div>
    </div>
  );
}

function Appearance() {
  const { pref, setPref } = useTheme();
  return (
    <section className="panel p-5">
      <h2 className="font-semibold text-ink-0">Appearance</h2>
      <p className="mt-1 text-sm text-ink-300">Applies right away.</p>
      <div className="mt-4 grid gap-3 sm:grid-cols-3">
        {THEMES.map((t) => (
          <button
            key={t.id}
            onClick={() => setPref(t.id)}
            aria-pressed={pref === t.id}
            className={clsx(
              "flex items-center gap-2.5 rounded-xl border px-4 py-3 text-left text-sm font-medium transition-colors",
              pref === t.id ? "border-loom-400 bg-loom-500/10 text-ink-0" : "border-ink-700 text-ink-200 hover:border-ink-500",
            )}
          >
            <t.icon size={16} className={pref === t.id ? "text-cyan-300" : "text-ink-400"} />
            {t.label}
          </button>
        ))}
      </div>
    </section>
  );
}

function Updates() {
  const { enabled, setEnabled } = useUpdates();
  return (
    <section className="panel p-5">
      <h2 className="flex items-center gap-2 font-semibold text-ink-0">
        <CircleArrowUp size={16} className="text-cyan-300" /> Updates
      </h2>
      <p className="mt-1.5 text-sm text-ink-300">
        When a new version is out, a notice in the bottom-left corner links to the download page. To find out,
        SerialLoom asks GitHub for its latest release when it starts and once a day. The request contains no personal
        data, and nothing is installed without you.
      </p>
      <label className="mt-3 flex items-center gap-2 text-sm text-ink-100">
        <input
          type="checkbox"
          checked={enabled}
          onChange={(e) => setEnabled(e.target.checked)}
          className="accent-loom-500"
        />
        Check for new versions
      </label>
    </section>
  );
}

function DriverCard({ active, onClick, title, text }: { active: boolean; onClick: () => void; title: string; text: string }) {
  return (
    <button
      onClick={onClick}
      className={clsx(
        "rounded-xl border p-4 text-left transition-colors",
        active ? "border-loom-400 bg-loom-500/10" : "border-ink-700 hover:border-ink-500",
      )}
    >
      <div className="flex items-center gap-2 font-semibold text-ink-0">
        <span className={clsx("h-3.5 w-3.5 rounded-full border-2", active ? "border-loom-400 bg-loom-400" : "border-ink-500")} />
        {title}
      </div>
      <p className="mt-1.5 text-xs text-ink-300">{text}</p>
    </button>
  );
}

function AdminPermission() {
  const { helper, refreshHelper, run, busy } = useStore();
  if (!helper?.supported) return null;

  const change = async (install: boolean) => {
    await run(
      install ? "Installing SerialLoom Helper (Windows asks for administrator permission)" : "Removing SerialLoom Helper (Windows asks for administrator permission)",
      () => (install ? api.installHelper() : api.removeHelper()),
      install ? "SerialLoom Helper installed" : "SerialLoom Helper removed",
    );
    await refreshHelper();
  };

  return (
    <section className="panel p-5">
      <h2 className="flex items-center gap-2 font-semibold text-ink-0">
        <ShieldCheck size={16} className="text-cyan-300" /> Administrator permission
      </h2>
      <p className="mt-1.5 text-sm text-ink-300">
        Creating, renumbering and removing virtual ports changes Windows device settings, which needs administrator
        rights. <b className="text-ink-100">SerialLoom Helper</b> is a small Windows service, installed with SerialLoom,
        that makes these changes so Windows does not ask every time. It can only create, renumber and remove
        SerialLoom&apos;s own virtual ports, serves only SerialLoom, and stops itself when it is not needed.
      </p>
      <div
        className={clsx(
          "mt-3 flex items-center gap-2 rounded-lg border px-3 py-2.5 text-sm",
          helper.installed
            ? "border-emerald-500/30 bg-emerald-500/10 text-emerald-100"
            : "border-amber-500/30 bg-amber-500/10 text-amber-100",
        )}
      >
        {helper.installed ? <CircleCheck size={16} className="shrink-0" /> : <CircleAlert size={16} className="shrink-0" />}
        <span className="min-w-0 flex-1">
          {helper.installed
            ? `Installed${helper.running ? " and running" : ""}. Port changes need no permission prompt.`
            : "Not installed. Windows asks for administrator permission for each port change."}
        </span>
        <button className="btn-ghost shrink-0" disabled={!!busy} onClick={() => change(!helper.installed)}>
          {helper.installed ? "Remove" : "Install"}
        </button>
      </div>
      <p className="mt-2 text-xs text-ink-400">Installing or removing the helper asks for administrator permission once.</p>
    </section>
  );
}

function Automation({ draft, setDraft }: { draft: AppSettings; setDraft: (s: AppSettings) => void }) {
  const settings = useStore((s) => s.settings);
  const [info, setInfo] = useState<AutomationInfo>();
  useEffect(() => {
    api
      .automationInfo()
      .then(setInfo)
      .catch((e) => useStore.getState().toast("error", errorText(e)));
  }, [settings]);

  const cli = info?.cliPath ? `"${info.cliPath}"` : "serialloom-cli";
  return (
    <section className="panel p-5">
      <h2 className="flex items-center gap-2 font-semibold text-ink-0">
        <SquareTerminal size={16} className="text-cyan-300" /> Automation
      </h2>
      <p className="mt-1.5 text-sm text-ink-300">
        Lets test scripts and other tools create virtual port pairs and start, stop or delete connections while
        SerialLoom runs, with <span className="font-mono text-ink-100">serialloom-cli</span> or the local API. Only
        programs running under your Windows account can connect, and nothing is reachable from the network.
      </p>
      <label className="mt-3 flex items-center gap-2 text-sm text-ink-100">
        <input
          type="checkbox"
          checked={draft.automation}
          onChange={(e) => setDraft({ ...draft, automation: e.target.checked })}
          className="accent-loom-500"
        />
        Allow other programs to control SerialLoom
      </label>
      {info?.error && (
        <div className="mt-3 flex gap-2 rounded-lg border border-rose-500/30 bg-rose-500/10 px-3 py-2.5 text-sm text-rose-100">
          <CircleAlert size={16} className="mt-0.5 shrink-0" />
          <span className="min-w-0">{info.error}</span>
        </div>
      )}
      {info?.running && (
        <div className="mt-3 space-y-1.5 text-xs text-ink-300">
          <div>Example: create a virtual null-modem pair and print its ports</div>
          <pre className="overflow-x-auto rounded-lg bg-ink-900 px-3 py-2 font-mono text-ink-100">{`${cli} pair COM9 COM10`}</pre>
          <div className="truncate">
            Pipe: <span className="font-mono">{info.pipe}</span>
          </div>
        </div>
      )}
    </section>
  );
}
