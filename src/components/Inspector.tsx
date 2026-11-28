import { useEffect, useRef, useState } from "react";
import { Activity, Eraser, Pause, Pencil, Play, Plus, Square, Trash, X } from "lucide-react";
import clsx from "clsx";
import { useStore } from "../store";
import { api } from "../lib/api";
import { HUB_COLOR, hubLabel, ascii, endpointSubtitle, endpointTitle, formatBytes, formatRate, hex, speedMismatch } from "../lib/format";
import { HUB_ICON } from "./nodes/HubNode";
import { isGps, withGpsDefaults, type HubView, type MonitorRecord, type ReplaySettings } from "../lib/types";
import { useT } from "../i18n";
import { GpsFields } from "./dialogs/EndpointEditor";

export function Inspector() {
  const { selection, hubs, select } = useStore();
  if (!selection) return null;
  const hub = hubs.find((h) => h.spec.id === selection.id);
  if (!hub) return null;
  return (
    <Shell onClose={() => select(null)}>
      <HubInspector hub={hub} />
    </Shell>
  );
}

function Shell({ children, onClose }: { children: React.ReactNode; onClose: () => void }) {
  const t = useT();
  return (
    <aside className="relative flex w-[380px] shrink-0 flex-col border-l border-ink-700/70 bg-ink-900/80">
      <button className="btn-ghost absolute top-3 right-3 z-10 px-2" onClick={onClose} aria-label={t("inspector.close")}>
        <X size={16} />
      </button>
      {children}
    </aside>
  );
}

function HubInspector({ hub }: { hub: HubView }) {
  const { run, rates, openDialog, select } = useStore();
  const t = useT();
  const { spec, status } = hub;
  const color = HUB_COLOR[spec.kind];
  const Icon = HUB_ICON[spec.kind];
  const endpoints = [spec.master, ...spec.members];
  const mismatch = status.running ? speedMismatch(endpoints, status.endpoints) : null;
  const hubRates = rates[spec.id] ?? [];
  const [editing, setEditing] = useState(false);
  const [name, setName] = useState(spec.name);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [removeVirtual, setRemoveVirtual] = useState(true);
  const hasVirtual = endpoints.some((e) => e.created);

  useEffect(() => setName(spec.name), [spec.name]);

  const saveName = async () => {
    setEditing(false);
    if (name.trim() && name !== spec.name) await run(t("inspector.renaming"), () => api.updateHub(spec.id, name.trim()));
  };

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="border-b border-ink-700/70 px-5 pt-5 pb-4">
        <div className="flex items-center gap-3 pr-8">
          <div className="grid h-10 w-10 shrink-0 place-items-center rounded-xl" style={{ background: `${color}22`, color }}>
            <Icon size={20} />
          </div>
          <div className="min-w-0 flex-1">
            {editing ? (
              <input
                autoFocus
                className="input py-1 text-sm"
                value={name}
                onChange={(e) => setName(e.target.value)}
                onBlur={saveName}
                onKeyDown={(e) => e.key === "Enter" && saveName()}
              />
            ) : (
              <button className="group flex max-w-full items-center gap-1.5 text-left" onClick={() => setEditing(true)}>
                <span className="truncate text-base font-semibold text-ink-0">{spec.name}</span>
                <Pencil size={12} className="shrink-0 text-ink-400 opacity-0 group-hover:opacity-100" />
              </button>
            )}
            <div className="text-xs" style={{ color }}>
              {hubLabel(spec.kind)} · {status.running ? t("common.running") : t("common.stopped")}
            </div>
          </div>
        </div>

        <div className="mt-4 flex flex-wrap gap-2">
          {status.running ? (
            <button className="btn-outline" onClick={() => run(t("inspector.stopping"), () => api.stopHub(spec.id))}>
              <Square size={13} fill="currentColor" /> {t("common.stop")}
            </button>
          ) : (
            <button className="btn-primary" onClick={() => run(t("inspector.starting"), () => api.startHub(spec.id))}>
              <Play size={13} fill="currentColor" /> {t("common.start")}
            </button>
          )}
          {spec.kind !== "bridge" && (
            <button className="btn-outline" onClick={() => openDialog({ type: "addMember", hubId: spec.id })}>
              <Plus size={14} /> {t("inspector.addEndpoint")}
            </button>
          )}
          <button className="btn-danger ml-auto" onClick={() => setConfirmDelete(true)}>
            <Trash size={14} /> {t("common.delete")}
          </button>
        </div>
        <label className="mt-3 flex items-center gap-2 text-xs text-ink-300">
          <input
            type="checkbox"
            className="accent-loom-500"
            checked={spec.autostart}
            onChange={(e) => run(t("inspector.saving"), () => api.updateHub(spec.id, undefined, e.target.checked))}
          />
          {t("common.startOnOpen")}
        </label>

        {confirmDelete && (
          <div className="mt-3 rounded-lg border border-rose-500/40 bg-rose-500/10 p-3 text-sm">
            <p className="text-rose-100">{t("inspector.deleteConfirm", { name: spec.name })}</p>
            {hasVirtual && (
              <label className="mt-2 flex items-center gap-2 text-xs text-rose-200">
                <input type="checkbox" checked={removeVirtual} onChange={(e) => setRemoveVirtual(e.target.checked)} />
                {t("inspector.removeVirtual")}
              </label>
            )}
            <div className="mt-3 flex justify-end gap-2">
              <button className="btn-ghost" onClick={() => setConfirmDelete(false)}>
                {t("common.cancel")}
              </button>
              <button
                className="btn bg-rose-500 text-white hover:bg-rose-400"
                onClick={async () => {
                  await run(
                    t("inspector.deleting"),
                    () => api.deleteHub(spec.id, hasVirtual && removeVirtual),
                    t("inspector.deleted"),
                  );
                  select(null);
                }}
              >
                {t("common.delete")}
              </button>
            </div>
          </div>
        )}
      </div>

      <div className="min-h-0 flex-1 overflow-y-auto">
        <section className="px-5 py-4">
          <h4 className="label">{t("inspector.endpoints")}</h4>
          <div className="space-y-2">
            {endpoints.map((e, i) => {
              const st = status.endpoints[i];
              const r = hubRates[i];
              const err = status.running ? st?.lastError : null;
              const warning = status.running ? st?.warning : null;
              return (
                <div key={i} className="rounded-lg border border-ink-700 bg-ink-850 px-3 py-2">
                  <div className="flex items-center gap-2">
                    <span
                      className="h-2 w-2 shrink-0 rounded-full"
                      style={{
                        background: !status.running ? "#475569" : st?.connected ? "#34d399" : err ? "#f43f5e" : "#fbbf24",
                      }}
                    />
                    <span className="font-mono text-sm font-semibold text-ink-0">{endpointTitle(e)}</span>
                    {i === 0 && <span className="chip bg-ink-700 text-ink-200">{t("inspector.master")}</span>}
                    <span className="ml-auto truncate text-[11px] text-ink-400">{endpointSubtitle(e, status.running, st)}</span>
                    {i > 0 && spec.kind !== "bridge" && spec.members.length > 1 && (
                      <button
                        className="text-ink-500 hover:text-rose-300"
                        title={t("inspector.removeFromHub")}
                        onClick={() => run(t("inspector.removingEndpoint"), () => api.removeMember(spec.id, i - 1, true))}
                      >
                        <X size={14} />
                      </button>
                    )}
                  </div>
                  {st && (
                    <div className="mt-1.5 grid grid-cols-3 gap-1 font-mono text-[10.5px] text-ink-300">
                      <span title={t("node.received")}>
                        ▲ {formatBytes(st.rxBytes)}
                        <br />
                        <span className="text-ink-400">{formatRate(r?.rx ?? 0)}</span>
                      </span>
                      <span title={t("node.sent")}>
                        ▼ {formatBytes(st.txBytes)}
                        <br />
                        <span className="text-ink-400">{formatRate(r?.tx ?? 0)}</span>
                      </span>
                      <span title={t("inspector.droppedTip")} className={st.droppedBytes ? "text-amber-300" : ""}>
                        ✕ {formatBytes(st.droppedBytes)}
                        <br />
                        <span className="text-ink-400">{t("inspector.dropped")}</span>
                      </span>
                    </div>
                  )}
                  {err && <p className="mt-1.5 text-[11px] break-words text-rose-300">{err}</p>}
                  {warning && <p className="mt-1.5 text-[11px] break-words text-amber-300">{warning}</p>}
                </div>
              );
            })}
          </div>
          {mismatch && (
            <p className="mt-2 rounded-lg border border-amber-500/30 bg-amber-500/10 px-3 py-2 text-[11px] text-amber-100">
              {t("inspector.mismatch", { ports: mismatch })}
            </p>
          )}
        </section>
        {endpoints.map((e, i) =>
          e.spec.kind === "replay" && isGps(e.spec.settings.format) ? (
            // Remounts with the saved values after each change.
            <GpsPanel key={`${i}:${JSON.stringify(e.spec.settings)}`} hubId={spec.id} index={i}
              settings={e.spec.settings} running={status.running} />
          ) : null,
        )}
        <TrafficMonitor hub={hub} />
      </div>
    </div>
  );
}

/** A GPS simulator's settings, which can change while it runs. */
function GpsPanel({ hubId, index, settings, running }: {
  hubId: string;
  index: number;
  settings: ReplaySettings;
  running: boolean;
}) {
  const { run, refresh } = useStore();
  const t = useT();
  const saved = withGpsDefaults(settings.gps);
  const [gps, setGps] = useState(saved);
  const [cycle, setCycle] = useState(settings.cycleMs);
  const dirty = JSON.stringify(gps) !== JSON.stringify(saved) || cycle !== settings.cycleMs;
  const nmea0183 = settings.format === "gps0183";
  const apply = () =>
    run(t("inspector.gpsApply"), async () => {
      await api.updateSimulator(hubId, index, gps, cycle);
      await refresh();
    }, t("inspector.gpsApplied"));

  return (
    <section className="border-t border-ink-700/70 px-5 py-4">
      <h4 className="label">{t(nmea0183 ? "editor.gps0183" : "editor.gps2000")}</h4>
      <p className="mb-3 text-[11px] text-ink-400">{t(running ? "inspector.gpsLive" : "inspector.gpsStopped")}</p>
      <GpsFields gps={gps} onChange={setGps} nmea0183={nmea0183} />
      <label className="mt-3 flex items-center gap-2 text-xs">
        {t("gps.interval")}
        <input type="number" className="input w-28" min={1} max={60_000} step={1} value={cycle}
          onChange={(event) => setCycle(Number(event.target.value))} />
      </label>
      <div className="mt-3 flex justify-end gap-2">
        <button className="btn-ghost" disabled={!dirty} onClick={() => { setGps(saved); setCycle(settings.cycleMs); }}>
          {t("settings.discard")}
        </button>
        <button className="btn-primary" disabled={!dirty} onClick={() => void apply()}>
          {t("inspector.gpsApply")}
        </button>
      </div>
    </section>
  );
}

function TrafficMonitor({ hub }: { hub: HubView }) {
  const t = useT();
  const [on, setOn] = useState(true);
  const [mode, setMode] = useState<"ascii" | "hex">("ascii");
  const [records, setRecords] = useState<MonitorRecord[]>([]);
  const last = useRef(0);
  const box = useRef<HTMLDivElement>(null);
  const endpoints = [hub.spec.master, ...hub.spec.members];

  useEffect(() => {
    last.current = 0;
    setRecords([]);
  }, [hub.spec.id]);

  useEffect(() => {
    if (!on || !hub.status.running) return;
    let alive = true;
    const tick = async () => {
      try {
        const fresh = await api.hubMonitor(hub.spec.id, last.current);
        if (!alive || fresh.length === 0) return;
        last.current = fresh[fresh.length - 1].seq;
        setRecords((r) => [...r, ...fresh].slice(-300));
      } catch {
        /* hub may have been deleted */
      }
    };
    tick();
    const t = setInterval(tick, 400);
    return () => {
      alive = false;
      clearInterval(t);
    };
  }, [on, hub.spec.id, hub.status.running]);

  useEffect(() => {
    const el = box.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [records]);

  return (
    <section className="border-t border-ink-700/70 px-5 py-4">
      <div className="mb-2 flex items-center gap-2">
        <h4 className="label mb-0 flex items-center gap-1.5">
          <Activity size={12} /> {t("monitor.title")}
        </h4>
        <div className="ml-auto flex overflow-hidden rounded-md border border-ink-700 text-[11px]">
          {(["ascii", "hex"] as const).map((m) => (
            <button
              key={m}
              onClick={() => setMode(m)}
              className={clsx("px-2 py-0.5 uppercase", mode === m ? "bg-ink-700 text-ink-0" : "text-ink-300")}
            >
              {m}
            </button>
          ))}
        </div>
        <button className="btn-ghost px-1.5 py-1" onClick={() => setOn(!on)} title={on ? t("monitor.pause") : t("monitor.resume")}>
          {on ? <Pause size={13} /> : <Play size={13} />}
        </button>
        <button className="btn-ghost px-1.5 py-1" onClick={() => setRecords([])} title={t("monitor.clear")}>
          <Eraser size={13} />
        </button>
      </div>
      <div
        ref={box}
        className="selectable h-64 overflow-y-auto rounded-lg border border-ink-700 bg-ink-950 p-2 font-mono text-[11px] leading-relaxed"
      >
        {!hub.status.running && <p className="text-ink-400">{t("monitor.startHint")}</p>}
        {hub.status.running && records.length === 0 && <p className="text-ink-400">{t("monitor.waiting")}</p>}
        {records.map((r) => (
          <div key={r.seq} className="flex gap-2">
            <span className="shrink-0 text-ink-500">{new Date(r.timestampMs).toLocaleTimeString([], { hour12: false })}</span>
            <span className="shrink-0 font-semibold" style={{ color: r.source === 0 ? HUB_COLOR[hub.spec.kind] : "#c4b5fd" }}>
              {endpoints[r.source] ? endpointTitle(endpoints[r.source]) : `#${r.source}`}
            </span>
            <span className="break-all text-ink-200">{mode === "hex" ? hex(r.data) : ascii(r.data)}</span>
          </div>
        ))}
      </div>
    </section>
  );
}
