import { memo, useEffect, useState, type MouseEvent } from "react";
import { createPortal } from "react-dom";
import { listen } from "@tauri-apps/api/event";
import { inTauri } from "../../lib/api";
import { useT } from "../../i18n";
import { capture, openTrace, traceChanged, traceTime, type TraceStatus } from "../../lib/trace";
import { BaseEdge, EdgeLabelRenderer, Position, getBezierPath, type EdgeProps } from "@xyflow/react";
import type { TrafficEdge as TrafficEdgeType } from "../graph";
import { formatRate } from "../../lib/format";

/** Bezier edge that animates in the direction(s) data is moving. */
function TrafficEdgeImpl(props: EdgeProps<TrafficEdgeType>) {
  const { id, sourceX, sourceY, targetX, targetY, sourcePosition, targetPosition, data } = props;
  const [path, labelX, labelY] = getBezierPath({ sourceX, sourceY, targetX, targetY, sourcePosition, targetPosition });
  const color = data?.color ?? "#64748b";
  const fwd = data?.forward ?? 0;
  const back = data?.backward ?? 0;
  const running = data?.running ?? false;
  const reversed = sourcePosition === Position.Left;
  const active = running && fwd + back > 0;
  const t = useT();
  const trace = data?.trace;
  const hubId = trace?.id;
  const index = trace?.index;
  const [menu, setMenu] = useState<{ x: number; y: number }>();
  const [status, setStatus] = useState<TraceStatus>();
  const [error, setError] = useState("");
  const pending = status?.pending === true;
  const recording = status?.running === true;

  useEffect(() => {
    if (!inTauri || hubId === undefined || index === undefined) return;
    let closed = false;
    let unlisten: (() => void) | undefined;
    const refresh = () => void capture({ id: hubId, index }, { type: "summary" })
      .then((next) => { if (!closed) setStatus(next); })
      .catch(() => { if (!closed) setStatus(undefined); });
    void listen<{ id: string; index: number }>(traceChanged, ({ payload }) => {
      if (payload.id === hubId && payload.index === index) refresh();
    }).then((dispose) => { if (closed) dispose(); else unlisten = dispose; }).catch(() => {});
    return () => { closed = true; unlisten?.(); };
  }, [hubId, index]);

  useEffect(() => {
    if ((!recording && !pending) || hubId === undefined || index === undefined) return;
    let closed = false;
    const timer = window.setInterval(() => {
      void capture({ id: hubId, index }, { type: "summary" }).then((next) => {
        if (!closed) setStatus(next);
      }).catch(() => { if (!closed) setStatus(undefined); });
    }, 1000);
    return () => { closed = true; window.clearInterval(timer); };
  }, [recording, pending, hubId, index]);

  const showMenu = (event: MouseEvent) => {
    if (!trace || !inTauri) return;
    event.preventDefault();
    event.stopPropagation();
    setError("");
    setMenu({ x: Math.max(0, Math.min(event.clientX, window.innerWidth - 260)),
      y: Math.max(0, Math.min(event.clientY, window.innerHeight - 120)) });
  };

  const open = async (fileMode: boolean) => {
    if (!trace) return;
    try {
      await openTrace(trace, fileMode);
      setMenu(undefined);
    } catch (failure) {
      setError(String(failure));
    }
  };

  return (
    <>
      <BaseEdge
        id={id}
        path={path}
        style={{
          stroke: color,
          strokeOpacity: running ? 0.35 : 0.2,
          strokeWidth: 2,
          strokeDasharray: data?.dashed ? "6 6" : undefined,
        }}
      />
      {active && fwd > 0 && (
        <path d={path} fill="none" stroke={color} strokeWidth={2.5} strokeDasharray="4 8" className="edge-flow" />
      )}
      {active && back > 0 && (
        <path
          d={path}
          fill="none"
          stroke="var(--color-ink-200)"
          strokeOpacity={0.55}
          strokeWidth={1.5}
          strokeDasharray="2 10"
          className="edge-flow-reverse"
        />
      )}
      {trace && inTauri && <path d={path} fill="none" stroke="transparent" strokeWidth={18}
        pointerEvents="stroke" onContextMenu={showMenu} />}
      {(data?.label || active || recording || pending || status?.reason === "space") && (
        <EdgeLabelRenderer>
          <div
            className={`absolute rounded-md border border-ink-700 bg-ink-900/90 px-1.5 py-0.5 font-mono text-[10px] text-ink-200 ${trace && inTauri ? "pointer-events-auto nopan nodrag" : "pointer-events-none"}`}
            onContextMenu={showMenu}
            style={{ transform: `translate(-50%, -50%) translate(${labelX}px, ${labelY}px)` }}
          >
            {data?.label ?? (
              <>
                <span style={{ color }}>{reversed ? "←" : "→"} {formatRate(fwd)}</span>
                {back > 0 && <span className="ml-1.5 text-ink-300">{reversed ? "→" : "←"} {formatRate(back)}</span>}
                {menu && createPortal(
        <div className="fixed inset-0 z-[100]" onPointerDown={() => setMenu(undefined)}
          onContextMenu={(event) => { event.preventDefault(); setMenu(undefined); }}
          onKeyDown={(event) => { if (event.key === "Escape") setMenu(undefined); }}>
          <div className="absolute w-64 rounded-lg border border-ink-600 bg-ink-900 p-1 shadow-lg"
            style={{ left: menu.x, top: menu.y }} onPointerDown={(event) => event.stopPropagation()}>
            <button autoFocus className="btn-ghost w-full justify-start" onClick={() => void open(false)}>
              {t("trace.interactive")}…
            </button>
            <button className="btn-ghost w-full justify-start" onClick={() => void open(true)}>
              {t("trace.file")}…
            </button>
            {error && <p role="alert" className="p-2 text-xs text-rose-400">{error}</p>}
          </div>
        </div>, document.body)}
    </>
            )}
            {pending && <span className="ml-2 text-amber-400">{t("trace.pending")}</span>}
            {recording && <span className="ml-2 text-rose-400">● REC {traceTime(status.remainingSeconds)}</span>}
            {status?.reason === "space" && <span className="ml-2 text-amber-400">{t("trace.space")}</span>}
          </div>
        </EdgeLabelRenderer>
      )}
      {menu && createPortal(
        <div className="fixed inset-0 z-[100]" onPointerDown={() => setMenu(undefined)}
          onContextMenu={(event) => { event.preventDefault(); setMenu(undefined); }}
          onKeyDown={(event) => { if (event.key === "Escape") setMenu(undefined); }}>
          <div className="absolute w-64 rounded-lg border border-ink-600 bg-ink-900 p-1 shadow-lg"
            style={{ left: menu.x, top: menu.y }} onPointerDown={(event) => event.stopPropagation()}>
            <button autoFocus className="btn-ghost w-full justify-start" onClick={() => void open(false)}>
              {t("trace.interactive")}…
            </button>
            <button className="btn-ghost w-full justify-start" onClick={() => void open(true)}>
              {t("trace.file")}…
            </button>
            {error && <p role="alert" className="p-2 text-xs text-rose-400">{error}</p>}
          </div>
        </div>, document.body)}
    </>
  );
}

export const TrafficEdge = memo(TrafficEdgeImpl);
