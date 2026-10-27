import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  Background,
  BackgroundVariant,
  Controls,
  MiniMap,
  ReactFlow,
  ReactFlowProvider,
  applyNodeChanges,
  useReactFlow,
  type NodeChange,
} from "@xyflow/react";
import { ArrowLeftRight, Cable, GripVertical, LayoutGrid, Merge, Plus, RadioTower, Split } from "lucide-react";
import clsx from "clsx";
import { useStore } from "../../store";
import { useTheme } from "../../lib/theme";
import { buildGraph, type GraphNode } from "../graph";
import { EndpointNode } from "../nodes/EndpointNode";
import { HubNode } from "../nodes/HubNode";
import { TrafficEdge } from "../nodes/TrafficEdge";
import { KindBadge } from "../PortIcon";
import { DEFAULT_SERIAL, type HubKind, type PortInfo } from "../../lib/types";
import { HUB_META } from "../../lib/format";

const nodeTypes = { endpoint: EndpointNode, hub: HubNode };
const edgeTypes = { traffic: TrafficEdge };
const POSITIONS_KEY = "serialloom.positions.v1";
export const DRAG_MIME = "application/x-serialloom-port";

type XY = { x: number; y: number };

function loadPositions(): Record<string, XY> {
  try {
    return JSON.parse(localStorage.getItem(POSITIONS_KEY) ?? "{}");
  } catch {
    return {};
  }
}

function savePositions(p: Record<string, XY>) {
  try {
    localStorage.setItem(POSITIONS_KEY, JSON.stringify(p));
  } catch {
    /* storage unavailable: positions just won't persist */
  }
}

export function TopologyView() {
  return (
    <ReactFlowProvider>
      <div className="flex h-full min-h-0">
        <PortPalette />
        <Canvas />
      </div>
    </ReactFlowProvider>
  );
}

function Canvas() {
  const { hubs, ports, rates, selection, select, openDialog } = useStore();
  const theme = useTheme((s) => s.resolved);
  const { fitView } = useReactFlow();
  const saved = useRef<Record<string, XY>>(loadPositions());
  const [nodes, setNodes] = useState<GraphNode[]>([]);
  const [dropHub, setDropHub] = useState<string | null>(null);

  const graph = useMemo(() => buildGraph(hubs, ports, rates), [hubs, ports, rates]);
  const structureKey = graph.nodes.map((n) => n.id).join("|");

  // Merge fresh data into existing nodes so drag positions and measurements survive status updates.
  useEffect(() => {
    setNodes((prev) => {
      const byId = new Map(prev.map((n) => [n.id, n]));
      return graph.nodes.map((n) => {
        const old = byId.get(n.id);
        const selected = selection?.type === "hub" && (n.id === selection.id || n.data.hubId === selection.id);
        if (old) return { ...old, data: n.data, selected } as GraphNode;
        return { ...n, position: saved.current[n.id] ?? n.position, selected } as GraphNode;
      });
    });
  }, [graph, selection]);

  useEffect(() => {
    const t = setTimeout(() => fitView({ padding: 0.2, duration: 300, maxZoom: 1.1 }), 60);
    return () => clearTimeout(t);
  }, [structureKey, fitView]);

  const onNodesChange = useCallback(
    (changes: NodeChange<GraphNode>[]) => setNodes((ns) => applyNodeChanges(changes, ns)),
    [],
  );

  const onNodeDragStop = useCallback((_: unknown, node: GraphNode) => {
    saved.current = { ...saved.current, [node.id]: node.position };
    savePositions(saved.current);
  }, []);

  const resetLayout = () => {
    saved.current = {};
    savePositions({});
    setNodes(graph.nodes);
    setTimeout(() => fitView({ padding: 0.2, duration: 300, maxZoom: 1.1 }), 50);
  };

  const hubAt = (x: number, y: number) =>
    (document.elementFromPoint(x, y)?.closest("[data-hub-id]") as HTMLElement | null)?.dataset.hubId ?? null;

  const onDragOver = (e: React.DragEvent) => {
    if (!e.dataTransfer.types.includes(DRAG_MIME)) return;
    e.preventDefault();
    e.dataTransfer.dropEffect = "copy";
    setDropHub(hubAt(e.clientX, e.clientY));
  };

  const onDrop = (e: React.DragEvent) => {
    const port = e.dataTransfer.getData(DRAG_MIME);
    setDropHub(null);
    if (!port) return;
    e.preventDefault();
    const hubId = hubAt(e.clientX, e.clientY);
    const isVirtual = ports.find((p) => p.name === port)?.kind === "virtual";
    const request = isVirtual
      ? ({ kind: "virtual", port } as const)
      : ({ kind: "serial", port, settings: DEFAULT_SERIAL } as const);
    if (hubId) openDialog({ type: "addMember", hubId, preset: request });
    else openDialog({ type: "hub", preset: { kind: "split", master: request } });
  };

  useEffect(() => {
    document.querySelectorAll(".drop-target").forEach((el) => el.classList.remove("drop-target"));
    if (dropHub) document.querySelector(`[data-hub-id="${dropHub}"]`)?.classList.add("drop-target");
  }, [dropHub]);

  const empty = hubs.length === 0;

  return (
    <div className="relative min-w-0 flex-1" onDragOver={onDragOver} onDrop={onDrop} onDragLeave={() => setDropHub(null)}>
      <ReactFlow<GraphNode>
        nodes={nodes}
        edges={graph.edges}
        nodeTypes={nodeTypes}
        edgeTypes={edgeTypes}
        onNodesChange={onNodesChange}
        onNodeDragStop={onNodeDragStop}
        onNodeClick={(_, n) => {
          if (n.type === "hub") select({ type: "hub", id: n.id });
          else if (n.data.hubId) select({ type: "hub", id: n.data.hubId });
        }}
        onPaneClick={() => select(null)}
        nodesConnectable={false}
        colorMode={theme}
        minZoom={0.3}
        maxZoom={2}
        proOptions={{ hideAttribution: true }}
      >
        <Background variant={BackgroundVariant.Dots} gap={22} size={1.2} color="var(--color-ink-600)" />
        <Controls position="bottom-left" showInteractive={false} />
        <MiniMap
          position="bottom-right"
          pannable
          zoomable
          nodeColor={(n) => ((n.data as { color?: string }).color ?? "#475569")}
          maskColor="var(--sl-minimap-mask)"
          style={{ width: 160, height: 100, borderRadius: 10, border: "1px solid var(--color-ink-700)" }}
        />
      </ReactFlow>

      <div className="absolute top-3 left-3 flex flex-wrap gap-2">
        {(["split", "join", "bridge", "bus"] as HubKind[]).map((k) => (
          <ToolbarButton key={k} kind={k} />
        ))}
        <button className="btn-outline bg-ink-850/90" onClick={() => openDialog({ type: "pair" })}>
          <Cable size={15} /> Virtual pair
        </button>
      </div>
      <div className="absolute top-3 right-3">
        <button className="btn-outline bg-ink-850/90" onClick={resetLayout} title="Reset node positions">
          <LayoutGrid size={15} /> Auto-layout
        </button>
      </div>

      {empty && <EmptyState />}
    </div>
  );
}

const KIND_ICON = { split: Split, join: Merge, bridge: ArrowLeftRight, bus: RadioTower };

function ToolbarButton({ kind }: { kind: HubKind }) {
  const openDialog = useStore((s) => s.openDialog);
  const Icon = KIND_ICON[kind];
  const meta = HUB_META[kind];
  return (
    <button
      className="btn-outline bg-ink-850/90"
      onClick={() => openDialog({ type: "hub", preset: { kind } })}
      title={meta.describe}
    >
      <Icon size={15} style={{ color: meta.color }} /> {meta.label}
    </button>
  );
}

function EmptyState() {
  const openDialog = useStore((s) => s.openDialog);
  return (
    <div className="pointer-events-none absolute inset-0 grid place-items-center">
      <div className="panel pointer-events-auto max-w-md p-6 text-center">
        <img src="/logo.svg" alt="" className="mx-auto mb-3 h-14 w-14" />
        <h2 className="text-lg font-semibold text-ink-0">Weave your first connection</h2>
        <p className="mt-1.5 text-sm text-ink-300">
          Drag a port from the left onto the canvas to split it, or start from one of the templates.
        </p>
        <div className="mt-4 flex justify-center gap-2">
          <button className="btn-primary" onClick={() => openDialog({ type: "hub", preset: { kind: "split" } })}>
            <Split size={15} /> Split a port
          </button>
          <button className="btn-outline" onClick={() => openDialog({ type: "pair" })}>
            <Plus size={15} /> Virtual pair
          </button>
        </div>
      </div>
    </div>
  );
}

function PortPalette() {
  const { ports, hubs } = useStore();
  const visible = ports;
  const hubName = (id: string | null) => hubs.find((h) => h.spec.id === id)?.spec.name;

  return (
    <aside className="flex w-64 shrink-0 flex-col border-r border-ink-700/70 bg-ink-900/60">
      <div className="px-4 pt-4 pb-2">
        <h3 className="text-xs font-semibold tracking-wider text-ink-300 uppercase">Ports</h3>
        <p className="mt-1 text-[11px] leading-snug text-ink-400">
          Drag onto the canvas to split, or onto a hub to add it.
        </p>
      </div>
      <div className="min-h-0 flex-1 space-y-1.5 overflow-y-auto px-3 pb-3">
        {visible.length === 0 && <p className="px-1 py-6 text-center text-xs text-ink-400">No serial ports found.</p>}
        {visible.map((p) => (
          <PaletteItem key={p.name} port={p} busyBy={hubName(p.inUseBy)} />
        ))}
      </div>
    </aside>
  );
}

function PaletteItem({ port, busyBy }: { port: PortInfo; busyBy?: string }) {
  const draggable = !port.inUseBy;
  return (
    <div
      draggable={draggable}
      onDragStart={(e) => {
        e.dataTransfer.setData(DRAG_MIME, port.name);
        e.dataTransfer.effectAllowed = "copy";
      }}
      className={clsx(
        "group flex items-center gap-2 rounded-lg border px-2.5 py-2 transition-colors",
        draggable
          ? "cursor-grab border-ink-700 bg-ink-850 hover:border-loom-400/60 active:cursor-grabbing"
          : "border-ink-800 bg-ink-900/40 opacity-60",
      )}
      title={port.description ?? undefined}
    >
      <GripVertical size={14} className="shrink-0 text-ink-500 group-hover:text-ink-300" />
      <div className="min-w-0 flex-1">
        <div className="flex items-center justify-between gap-2">
          <span className="font-mono text-sm font-semibold text-ink-0">{port.name}</span>
          <KindBadge kind={port.kind} />
        </div>
        <div className="truncate text-[11px] text-ink-400">
          {busyBy ? `in use · ${busyBy}` : (port.description ?? "Serial port")}
        </div>
      </div>
    </div>
  );
}
