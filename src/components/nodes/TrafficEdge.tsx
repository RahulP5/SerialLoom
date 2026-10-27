import { memo } from "react";
import { BaseEdge, EdgeLabelRenderer, getBezierPath, type EdgeProps } from "@xyflow/react";
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
  const active = running && fwd + back > 0;

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
      {(data?.label || active) && (
        <EdgeLabelRenderer>
          <div
            className="pointer-events-none absolute rounded-md border border-ink-700 bg-ink-900/90 px-1.5 py-0.5 font-mono text-[10px] text-ink-200"
            style={{ transform: `translate(-50%, -50%) translate(${labelX}px, ${labelY}px)` }}
          >
            {data?.label ?? (
              <>
                <span style={{ color }}>→ {formatRate(fwd)}</span>
                {back > 0 && <span className="ml-1.5 text-ink-300">← {formatRate(back)}</span>}
              </>
            )}
          </div>
        </EdgeLabelRenderer>
      )}
    </>
  );
}

export const TrafficEdge = memo(TrafficEdgeImpl);
