import { memo } from "react";
import { Handle, Position, type NodeProps } from "@xyflow/react";
import clsx from "clsx";
import type { EndpointNode as EndpointNodeType } from "../graph";
import { PortIcon, KIND_COLOR } from "../PortIcon";
import { formatRate } from "../../lib/format";
import { useT } from "../../i18n";

function EndpointNodeImpl({ data, selected }: NodeProps<EndpointNodeType>) {
  const t = useT();
  const kindColor = KIND_COLOR[data.icon];
  const error = data.status?.lastError;
  const warning = data.status?.warning;
  const connected = data.status?.connected;
  const state = !data.running ? "idle" : error && !connected ? "error" : connected ? "live" : "waiting";
  const dot = { idle: "#475569", error: "#f43f5e", live: "#34d399", waiting: "#fbbf24" }[state];
  const busy = (data.rate?.rx ?? 0) + (data.rate?.tx ?? 0) > 0;

  return (
    <div
      className={clsx(
        "w-[220px] rounded-xl border bg-ink-850/95 px-3 py-2.5 shadow-xl shadow-black/30 transition-colors",
        selected ? "border-loom-400" : "border-ink-700 hover:border-ink-500",
      )}
      title={error ?? warning ?? undefined}
    >
      <Handle id="l" type="target" position={Position.Left} style={{ background: data.color }} />
      <div className="flex items-center gap-2.5">
        <div
          className="grid h-9 w-9 shrink-0 place-items-center rounded-lg"
          style={{ color: kindColor, background: `${kindColor}1c` }}
        >
          <PortIcon kind={data.icon} size={18} />
        </div>
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-1.5">
            <span className="truncate font-mono text-[15px] font-semibold text-ink-0">{data.title}</span>
            {data.role === "master" && (
              <span className="chip bg-ink-700 text-ink-200" title={t("node.master")}>
                {t("node.masterShort")}
              </span>
            )}
          </div>
          <div className="truncate text-[11px] text-ink-300">
            {t(`kind.${data.icon}`)} · {data.subtitle}
          </div>
        </div>
        <span
          className={clsx("h-2.5 w-2.5 shrink-0 rounded-full", state === "live" && busy && "pulse")}
          style={{ background: dot, ["--ring" as string]: `${dot}88` }}
          title={t(`node.state.${state}`)}
        />
      </div>
      {data.running && (
        <div className="mt-2 flex justify-between border-t border-ink-700/70 pt-1.5 font-mono text-[10.5px] text-ink-300">
          <span title={t("node.received")}>▲ {formatRate(data.rate?.rx ?? 0)}</span>
          <span title={t("node.sent")}>▼ {formatRate(data.rate?.tx ?? 0)}</span>
        </div>
      )}
      {error && data.running && (
        <div className="mt-1.5 truncate text-[10.5px] text-rose-300">{error}</div>
      )}
      {!error && warning && data.running && (
        <div className="mt-1.5 truncate text-[10.5px] text-amber-300">{warning}</div>
      )}
      <Handle id="r" type="source" position={Position.Right} style={{ background: data.color }} />
    </div>
  );
}

export const EndpointNode = memo(EndpointNodeImpl);
