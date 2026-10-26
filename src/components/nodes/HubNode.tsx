import { memo } from "react";
import { Handle, Position, type NodeProps } from "@xyflow/react";
import { ArrowLeftRight, Merge, Play, RadioTower, Split, Square } from "lucide-react";
import clsx from "clsx";
import type { HubNode as HubNodeType } from "../graph";
import { HUB_META, formatRate } from "../../lib/format";
import { api } from "../../lib/api";
import { useStore } from "../../store";
import type { HubKind } from "../../lib/types";

export const HUB_ICON: Record<HubKind, typeof Split> = {
  split: Split,
  join: Merge,
  bridge: ArrowLeftRight,
  bus: RadioTower,
};

function HubNodeImpl({ data, selected }: NodeProps<HubNodeType>) {
  const { spec, status } = data.hub;
  const Icon = HUB_ICON[spec.kind];
  const run = useStore((s) => s.run);

  const toggle = (e: React.MouseEvent) => {
    e.stopPropagation();
    if (status.running) run("Stopping hub", () => api.stopHub(spec.id));
    else run("Starting hub", () => api.startHub(spec.id));
  };

  return (
    <div
      data-hub-id={spec.id}
      className={clsx(
        "w-[224px] rounded-2xl border bg-ink-800/95 px-3 py-2.5 shadow-xl shadow-black/40",
        selected ? "border-loom-400" : "border-ink-600",
      )}
      style={{ boxShadow: status.running ? `0 0 0 1px ${data.color}33, 0 0 28px -6px ${data.color}66` : undefined }}
    >
      <Handle id="l" type="target" position={Position.Left} style={{ background: data.color }} />
      <div className="flex items-center gap-2.5">
        <div
          className="grid h-9 w-9 shrink-0 place-items-center rounded-xl"
          style={{ background: `${data.color}22`, color: data.color }}
        >
          <Icon size={18} />
        </div>
        <div className="min-w-0 flex-1">
          <div className="truncate text-sm font-semibold text-white">{spec.name}</div>
          <div className="text-[11px]" style={{ color: data.color }}>
            {HUB_META[spec.kind].label} · {spec.members.length + 1} ends
          </div>
        </div>
        <button
          className={clsx(
            "nodrag grid h-7 w-7 place-items-center rounded-lg border transition-colors",
            status.running
              ? "border-rose-400/40 text-rose-300 hover:bg-rose-500/15"
              : "border-emerald-400/40 text-emerald-300 hover:bg-emerald-500/15",
          )}
          onClick={toggle}
          title={status.running ? "Stop" : "Start"}
        >
          {status.running ? <Square size={12} fill="currentColor" /> : <Play size={13} fill="currentColor" />}
        </button>
      </div>
      <div className="mt-2 flex items-center justify-between text-[10.5px] text-ink-300">
        <span className={status.running ? "text-emerald-300" : "text-ink-400"}>
          {status.running ? "● running" : "○ stopped"}
        </span>
        <span className="font-mono">{formatRate(data.totalRate)}</span>
      </div>
      <Handle id="r" type="source" position={Position.Right} style={{ background: data.color }} />
    </div>
  );
}

export const HubNode = memo(HubNodeImpl);
