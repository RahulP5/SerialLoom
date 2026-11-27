import { useEffect, useRef, useState, type PointerEvent } from "react";
import { emit } from "@tauri-apps/api/event";
import { save } from "@tauri-apps/plugin-dialog";
import { useT } from "../../i18n";
import { ascii, hex } from "../../lib/format";
import { capture, traceChanged, traceTime, type Direction, type FileFormat, type TraceOptions, type TraceRecord, type TraceStatus } from "../../lib/trace";

const columnKeys = ["trace.utc", "trace.delta", "trace.direction", "trace.bytes", "trace.preview"] as const;
const initialWidths = [158, 100, 80, 84, 440];

function drag(event: PointerEvent<HTMLElement>, move: (x: number, y: number) => void) {
  if (event.button !== 0) return;
  event.preventDefault();
  const element = event.currentTarget;
  const { clientX, clientY, pointerId } = event;
  element.setPointerCapture(pointerId);
  const onMove = (next: globalThis.PointerEvent) => move(next.clientX - clientX, next.clientY - clientY);
  element.addEventListener("pointermove", onMove);
  element.addEventListener("lostpointercapture", () => {
    element.removeEventListener("pointermove", onMove);
  }, { once: true });
}

export function TraceWindow({ options }: { options: TraceOptions }) {
  const t = useT();
  const [direction, setDirection] = useState<Direction>("both");
  const [seconds, setSeconds] = useState(60);
  const [fileMode, setFileMode] = useState(options.fileMode);
  const [status, setStatus] = useState<TraceStatus>();
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [paused, setPaused] = useState<TraceRecord[] | null>(null);
  const [selected, setSelected] = useState<TraceRecord>();
  const [fileFormat, setFileFormat] = useState<FileFormat>("hex");
  const [hexMode, setHexMode] = useState(false);
  const [split, setSplit] = useState(0.52);
  const [widths, setWidths] = useState(initialWidths);
  const body = useRef<HTMLDivElement>(null);
  const table = useRef<HTMLDivElement>(null);
  const records = paused ?? status?.records ?? [];
  const displayed = selected ?? records.at(-1);
  const engaged = status?.running || status?.pending;
  const canStart = !engaged && !busy;

  useEffect(() => {
    let active = true;
    let pending = false;
    const poll = async () => {
      if (pending) return;
      pending = true;
      try {
        const next = await capture(options, { type: "status" });
        if (active) setStatus(next);
      } catch (failure) {
        if (active) {
          setError(String(failure));

        }
      } finally {
        pending = false;
      }
    };
    const timer = window.setInterval(() => void poll(), 400);
    void poll();
    return () => {
      active = false;
      window.clearInterval(timer);
    };
  }, [options]);

  useEffect(() => {
    if (!paused && !selected && table.current) {
      table.current.scrollTop = table.current.scrollHeight;
    }
  }, [records, paused, selected]);

  const update = async (start: boolean) => {
    setBusy(true);
    setError("");
    try {
      if (start) {
        const path = fileMode ? await save({
          defaultPath: `SerialLoom-${new Date().toISOString().replace(/[:.]/g, "-")}.txt`,
          filters: [{ name: "TXT", extensions: ["txt"] }],
        }) : null;
        if (fileMode && !path) return;
        setPaused(null);
        setSelected(undefined);
        setStatus(await capture(options, { type: "start", direction, seconds, path, format: fileFormat }));
      } else {
        setStatus(await capture(options, { type: "stop" }));
      }
      await emit(traceChanged, { id: options.id, index: options.index });
    } catch (failure) {
      setError(String(failure));
    } finally {
      setBusy(false);
    }
  };

  const reasons = {
    stopped: "trace.stopped", expired: "trace.expired", space: "trace.space",
    size: "trace.size", write: "trace.writeError",
  } as const;
  const delta = { rx: -1, tx: -1 };
  const resizeSplit = (next: number) => setSplit(Math.min(0.78, Math.max(0.23, next)));

  return (
    <main className="flex h-dvh flex-col gap-3 overflow-hidden bg-ink-950 p-3 text-ink-100">
      <h1 className="truncate text-xl font-semibold">{options.source} ↔ {options.target}</h1>
      <div className="flex flex-wrap items-center gap-2 border-b border-ink-700 pb-2 text-sm">
        <select className="input w-auto" aria-label={t("trace.destination")} disabled={!canStart}
          value={fileMode ? "file" : "window"} onChange={(e) => setFileMode(e.target.value === "file")}>
          <option value="window">{t("trace.interactive")}</option>
          <option value="file">{t("trace.file")}</option>
        </select>
        <select className="input w-auto max-w-64" aria-label={t("trace.direction")} disabled={!canStart}
          value={direction} onChange={(e) => setDirection(e.target.value as Direction)}>
          <option value="both">{t("trace.both")}</option>
          <option value={options.forwardRx ? "rx" : "tx"}>{options.source} → {options.target}</option>
          <option value={options.forwardRx ? "tx" : "rx"}>{options.target} → {options.source}</option>
        </select>
        <select className="input w-auto" aria-label={t("trace.duration")} value={seconds}
          disabled={!canStart} onChange={(e) => setSeconds(Number(e.target.value))}>
          {[1, 3, 5, 10].map((minutes) => <option key={minutes} value={minutes * 60}>{minutes} min</option>)}
        </select>
        {fileMode && <select className="input w-auto" aria-label={t("replay.format")} disabled={!canStart}
          value={fileFormat} onChange={(e) => setFileFormat(e.target.value as FileFormat)}>
          <option value="hex">{t("trace.hex")}</option>
          <option value="text">{t("trace.text")}</option>
          <option value="both">{t("trace.text")} + {t("trace.hex")}</option>
        </select>}
        {status?.pending && <span role="status" className="text-amber-400">{t("trace.pending")}</span>}
        {status?.running && <span className="font-mono text-rose-400">● REC {traceTime(status.remainingSeconds)}</span>}
        <button className="btn-outline" disabled={busy} onClick={() => void update(!engaged)}>
          {t(engaged ? "common.stop" : "common.start")}
        </button>
        {!status?.isFile && <button className="btn-outline" onClick={() => {
          setPaused(paused ? null : records);
          if (paused) setSelected(undefined);
        }}>{t(paused ? "monitor.resume" : "monitor.pause")}</button>}
      </div>
      {(error || status?.reason) && <p role="status" className="text-sm text-ink-300">
        {error || (status?.reason ? t(reasons[status.reason]) : "")} {status?.detail}
      </p>}
      <div ref={body} className="grid min-h-0 flex-1" style={{ gridTemplateRows: `minmax(0, ${split}fr) 12px minmax(0, ${1 - split}fr)` }}>
        <div ref={table} className="min-h-0 overflow-auto rounded border border-ink-700">
          <table className="w-full table-fixed border-collapse text-left text-sm" style={{ minWidth: widths.reduce((a, b) => a + b, 0) }}>
            <colgroup>{widths.map((width, index) => <col key={index} style={{ width }} />)}</colgroup>
            <thead className="sticky top-0 bg-ink-800">
              <tr>{columnKeys.map((key, index) => (
                <th key={key} className="relative p-2">
                  {t(key)}
                  <div role="separator" aria-label={t(key)} aria-orientation="vertical" tabIndex={0}
                    className="absolute right-0 top-0 h-full w-2 cursor-col-resize touch-none border-r border-ink-600"
                    onPointerDown={(event) => drag(event, (x) => setWidths((previous) =>
                      previous.map((value, i) => i === index ? Math.max(60, widths[index] + x) : value)))}
                    onKeyDown={(event) => {
                      if (event.key !== "ArrowLeft" && event.key !== "ArrowRight") return;
                      event.preventDefault();
                      setWidths((previous) => previous.map((value, i) => i === index
                        ? Math.max(60, value + (event.key === "ArrowLeft" ? -10 : 10)) : value));
                    }} />
                </th>
              ))}</tr>
            </thead>
            <tbody>{records.map((record) => {
              const prior = delta[record.direction];
              delta[record.direction] = record.elapsedMs;
              return (
                <tr key={record.sequence} className={`cursor-pointer border-t border-ink-700 font-mono ${displayed?.sequence === record.sequence ? "bg-ink-700" : "hover:bg-ink-800"}`}
                  tabIndex={0} aria-selected={displayed?.sequence === record.sequence}
                  onClick={() => setSelected(record)} onKeyDown={(e) => {
                    if (e.key === "Enter") setSelected(record);
                  }}>
                  <td className="p-2">{record.timestamp.slice(11, 23)}</td>
                  <td className="p-2">{prior < 0 ? "—" : `+${record.elapsedMs - prior} ms`}</td>
                  <td className="p-2">{record.direction.toUpperCase()}</td>
                  <td className="p-2">{record.bytes.length}</td>
                  <td className="truncate p-2">{ascii(record.bytes)}</td>
                </tr>
              );
            })}</tbody>
          </table>
        </div>
        <div role="separator" aria-orientation="horizontal" aria-label={t("trace.resize")} tabIndex={0}
          className="flex cursor-row-resize touch-none items-center justify-center hover:bg-ink-700"
          onPointerDown={(event) => {
            const height = body.current?.clientHeight;
            if (height) drag(event, (_, y) => resizeSplit(split + y / height));
          }}
          onKeyDown={(event) => {
            if (event.key !== "ArrowUp" && event.key !== "ArrowDown") return;
            event.preventDefault();
            resizeSplit(split + (event.key === "ArrowUp" ? -0.04 : 0.04));
          }}>
          <span className="h-1 w-10 rounded bg-loom-400" />
        </div>
        <section className="flex min-h-0 flex-col rounded border border-ink-700">
          <div className="flex flex-wrap items-center gap-2 border-b border-ink-700 bg-ink-800 p-2 text-sm">
            <h2 className="mr-auto font-semibold">{t("trace.detail")}</h2>
            <button className="btn-outline" aria-pressed={!hexMode} onClick={() => setHexMode(false)}>{t("trace.text")}</button>
            <button className="btn-outline" aria-pressed={hexMode} onClick={() => setHexMode(true)}>{t("trace.hex")}</button>
            <button className="btn-outline" disabled={!displayed} onClick={() => {
              if (displayed) void navigator.clipboard.writeText(hexMode ? hex(displayed.bytes, true) : ascii(displayed.bytes))
                .catch((failure: unknown) => setError(String(failure)));
            }}>{t("trace.copy")}</button>
          </div>
          <pre className="min-h-0 flex-1 overflow-auto whitespace-pre-wrap break-all p-3 font-mono text-sm">
            {displayed ? (hexMode ? hex(displayed.bytes, true) : ascii(displayed.bytes)) : ""}
          </pre>
        </section>
      </div>
      <footer className="text-xs text-ink-300">
        {t(status?.isFile ? "trace.file" : "trace.memory")}
        {!!status?.dropped && ` · ${t("trace.dropped")} ${status.dropped}`}
      </footer>
    </main>
  );
}
