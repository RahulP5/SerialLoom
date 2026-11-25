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
import { LANGUAGES, systemLanguage, tr, useI18n, useT, type LanguagePref, type MessageKey } from "../../i18n";

const THEMES: { id: ThemePref; label: MessageKey; icon: typeof Sun }[] = [
  { id: "system", label: "theme.system", icon: Monitor },
  { id: "light", label: "theme.light", icon: Sun },
  { id: "dark", label: "theme.dark", icon: Moon },
];

export function SettingsView() {
  const { settings, driver, saveSettings, busy } = useStore();
  const t = useT();
  const [draft, setDraft] = useState<AppSettings | undefined>(settings);
  useEffect(() => setDraft(settings), [settings]);
  if (!draft) return null;

  const dirty = JSON.stringify(draft) !== JSON.stringify(settings);
  const setDriver = (d: DriverKind) => setDraft({ ...draft, driver: d });

  return (
    <div className="h-full overflow-y-auto px-8 py-7">
      <div className="mx-auto max-w-3xl space-y-6">
        <div>
          <h1 className="text-xl font-semibold text-ink-0">{t("settings.title")}</h1>
          <p className="mt-1 text-sm text-ink-300">{t("settings.subtitle")}</p>
        </div>

        <Appearance />

        <section className="panel p-5">
          <h2 className="flex items-center gap-2 font-semibold text-ink-0">
            <PanelBottom size={16} className="text-cyan-300" /> {t("background.title")}
          </h2>
          <p className="mt-1.5 text-sm text-ink-300">{t("background.text")}</p>
          <p className="mt-1.5 text-sm text-ink-300">
            {tr("background.quit", { quit: <b className="text-ink-100">{t("background.quitItem")}</b> })}
          </p>
          <label className="mt-3 flex items-center gap-2 text-sm text-ink-100">
            <input
              type="checkbox"
              checked={draft.startWithWindows}
              onChange={(e) => setDraft({ ...draft, startWithWindows: e.target.checked })}
              className="accent-loom-500"
            />
            {t("background.startWithWindows")}
          </label>
        </section>

        <Updates />

        <AdminPermission />

        <Automation draft={draft} setDraft={setDraft} />

        <section className="panel p-5">
          <h2 className="font-semibold text-ink-0">{t("driverSettings.title")}</h2>
          <p className="mt-1 text-sm text-ink-300">{t("driverSettings.text")}</p>
          <div className="mt-4 grid gap-3 sm:grid-cols-3">
            <DriverCard
              active={draft.driver === "usb"}
              onClick={() => setDriver("usb")}
              title={t("driverSettings.usbTitle")}
              text={t("driverSettings.usbText")}
            />
            <DriverCard
              active={draft.driver === "native"}
              onClick={() => setDriver("native")}
              title={t("driverSettings.nativeTitle")}
              text={t("driverSettings.nativeText")}
            />
            <DriverCard
              active={draft.driver === "demo"}
              onClick={() => setDriver("demo")}
              title={t("driverSettings.demoTitle")}
              text={t("driverSettings.demoText")}
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
                  {t(driver.available ? "driverSettings.ready" : "driverSettings.notInstalled", { name: driver.name })}
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
            {t("settings.discard")}
          </button>
          <button className="btn-primary" disabled={!dirty || !!busy} onClick={() => saveSettings(draft)}>
            {t("settings.save")}
          </button>
        </div>
      </div>
    </div>
  );
}

function Appearance() {
  const { pref, setPref } = useTheme();
  const t = useT();
  return (
    <section className="panel p-5">
      <h2 className="font-semibold text-ink-0">{t("appearance.title")}</h2>
      <p className="mt-1 text-sm text-ink-300">{t("appearance.note")}</p>
      <div className="mt-4 grid gap-3 sm:grid-cols-3">
        {THEMES.map((theme) => (
          <button
            key={theme.id}
            onClick={() => setPref(theme.id)}
            aria-pressed={pref === theme.id}
            className={clsx(
              "flex items-center gap-2.5 rounded-xl border px-4 py-3 text-left text-sm font-medium transition-colors",
              pref === theme.id ? "border-loom-400 bg-loom-500/10 text-ink-0" : "border-ink-700 text-ink-200 hover:border-ink-500",
            )}
          >
            <theme.icon size={16} className={pref === theme.id ? "text-cyan-300" : "text-ink-400"} />
            {t(theme.label)}
          </button>
        ))}
      </div>
      <LanguagePicker />
    </section>
  );
}

function LanguagePicker() {
  const { pref, setPref } = useI18n();
  const t = useT();
  const system = LANGUAGES.find((l) => l.code === systemLanguage())?.name ?? "English";
  return (
    <div className="mt-5">
      <label className="label" htmlFor="language">
        {t("language.label")}
      </label>
      <select
        id="language"
        className="input sm:w-72"
        value={pref}
        onChange={(e) => setPref(e.target.value as LanguagePref)}
      >
        <option value="system">{t("language.system", { language: system })}</option>
        {LANGUAGES.map((l) => (
          <option key={l.code} value={l.code} lang={l.code}>
            {l.name}
          </option>
        ))}
      </select>
      <p className="mt-1.5 text-xs text-ink-400">{t("language.note")}</p>
    </div>
  );
}

function Updates() {
  const { enabled, setEnabled } = useUpdates();
  const t = useT();
  return (
    <section className="panel p-5">
      <h2 className="flex items-center gap-2 font-semibold text-ink-0">
        <CircleArrowUp size={16} className="text-cyan-300" /> {t("updates.title")}
      </h2>
      <p className="mt-1.5 text-sm text-ink-300">{t("updates.text")}</p>
      <label className="mt-3 flex items-center gap-2 text-sm text-ink-100">
        <input
          type="checkbox"
          checked={enabled}
          onChange={(e) => setEnabled(e.target.checked)}
          className="accent-loom-500"
        />
        {t("updates.check")}
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
  const t = useT();
  if (!helper?.supported) return null;

  const change = async (install: boolean) => {
    await run(
      t("common.adminNote", { action: t(install ? "admin.installing" : "admin.removing") }),
      () => (install ? api.installHelper() : api.removeHelper()),
      t(install ? "admin.installedToast" : "admin.removedToast"),
    );
    await refreshHelper();
  };

  return (
    <section className="panel p-5">
      <h2 className="flex items-center gap-2 font-semibold text-ink-0">
        <ShieldCheck size={16} className="text-cyan-300" /> {t("admin.title")}
      </h2>
      <p className="mt-1.5 text-sm text-ink-300">
        {tr("admin.text", { helper: <b className="text-ink-100">SerialLoom Helper</b> })}
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
            ? t(helper.running ? "admin.installedRunning" : "admin.installed")
            : t("admin.notInstalled")}
        </span>
        <button className="btn-ghost shrink-0" disabled={!!busy} onClick={() => change(!helper.installed)}>
          {helper.installed ? t("common.remove") : t("common.install")}
        </button>
      </div>
      <p className="mt-2 text-xs text-ink-400">{t("admin.once")}</p>
    </section>
  );
}

function Automation({ draft, setDraft }: { draft: AppSettings; setDraft: (s: AppSettings) => void }) {
  const settings = useStore((s) => s.settings);
  const t = useT();
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
        <SquareTerminal size={16} className="text-cyan-300" /> {t("automation.title")}
      </h2>
      <p className="mt-1.5 text-sm text-ink-300">
        {tr("automation.text", { cli: <span className="font-mono text-ink-100">serialloom-cli</span> })}
      </p>
      <label className="mt-3 flex items-center gap-2 text-sm text-ink-100">
        <input
          type="checkbox"
          checked={draft.automation}
          onChange={(e) => setDraft({ ...draft, automation: e.target.checked })}
          className="accent-loom-500"
        />
        {t("automation.allow")}
      </label>
      {info?.error && (
        <div className="mt-3 flex gap-2 rounded-lg border border-rose-500/30 bg-rose-500/10 px-3 py-2.5 text-sm text-rose-100">
          <CircleAlert size={16} className="mt-0.5 shrink-0" />
          <span className="min-w-0">{info.error}</span>
        </div>
      )}
      {info?.running && (
        <div className="mt-3 space-y-1.5 text-xs text-ink-300">
          <div>{t("automation.example")}</div>
          <pre className="overflow-x-auto rounded-lg bg-ink-900 px-3 py-2 font-mono text-ink-100">{`${cli} pair COM9 COM10`}</pre>
          <div className="truncate">
            {t("automation.pipe")} <span className="font-mono">{info.pipe}</span>
          </div>
        </div>
      )}
    </section>
  );
}
