import { useEffect, useState } from "react";
import { CircleAlert, CircleCheck, Monitor, Moon, PanelBottom, Sun } from "lucide-react";
import clsx from "clsx";
import { useStore } from "../../store";
import type { AppSettings, DriverKind } from "../../lib/types";
import { DriverMissing } from "./ListViews";
import { useTheme, type ThemePref } from "../../lib/theme";

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
          <p className="mt-1 text-sm text-ink-300">Appearance, running in the background, and where virtual ports come from.</p>
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
        </section>

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
