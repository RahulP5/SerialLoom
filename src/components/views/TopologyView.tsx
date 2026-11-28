import { useCallback, useEffect, useEffectEvent, useMemo, useRef, useState } from "react";
import {
  Background,
  BackgroundVariant,
  Controls,
  ConnectionMode,
  MiniMap,
  ReactFlow,
  ReactFlowProvider,
  applyNodeChanges,
  useReactFlow,
  type NodeChange,
} from "@xyflow/react";
import {
  ArrowLeftRight,
  Cable,
  ChevronDown,
  Download,
  FileJson,
  GripVertical,
  LayoutGrid,
  Merge,
  Plus,
  RadioTower,
  Split,
  Upload,
} from "lucide-react";
import clsx from "clsx";
import { listen } from "@tauri-apps/api/event";
import { inTauri } from "../../lib/api";
import { useStore } from "../../store";
import { loadMirroredHubs, loadPositions, saveMirroredHubs, savePositions } from "../../lib/topologyPositions";
import { exportTopology, importTopology } from "../../lib/topologyBackup";
import { useTheme } from "../../lib/theme";
import { buildGraph, type GraphNode } from "../graph";
import { EndpointNode } from "../nodes/EndpointNode";
import { HubNode } from "../nodes/HubNode";
import { TrafficEdge } from "../nodes/TrafficEdge";
import { KindBadge, PortIcon } from "../PortIcon";
import { replaySource } from "../dialogs/EndpointEditor";
import { DEFAULT_SERIAL, SOURCE_FORMATS, isGps, type HubKind, type PortInfo } from "../../lib/types";
import { HUB_COLOR, HUB_KINDS, hubDescription, hubLabel } from "../../lib/format";
import { useI18n, useT } from "../../i18n";

const nodeTypes = { endpoint: EndpointNode, hub: HubNode };
const edgeTypes = { traffic: TrafficEdge };
const REPLAY_MIME = "application/x-serialloom-replay";
export const DRAG_MIME = "application/x-serialloom-port";

export function TopologyView() {
  // Bumped after an import, so the canvas reloads the restored layout.
  const [layoutVersion, setLayoutVersion] = useState(0);
  return (
    <ReactFlowProvider>
      <div className="flex h-full min-h-0">
        <PortPalette />
        <Canvas key={layoutVersion} onImported={() => setLayoutVersion((v) => v + 1)} />
      </div>
    </ReactFlowProvider>
  );
}

function Canvas({ onImported }: { onImported: () => void }) {
  const { hubs, ports, rates, selection, select, openDialog } = useStore();
  const theme = useTheme((s) => s.resolved);
  const lang = useI18n((s) => s.lang);
  const t = useT();
  const { fitView } = useReactFlow();
  const saved = useRef(loadPositions());
  const [mirroredHubs, setMirroredHubs] = useState(loadMirroredHubs);
  const [nodes, setNodes] = useState<GraphNode[]>([]);
  const [dropHub, setDropHub] = useState<string | null>(null);

  // Node subtitles are text, so the graph is rebuilt when the language changes.
  const graph = useMemo(() => buildGraph(hubs, ports, rates, mirroredHubs), [hubs, ports, rates, lang, mirroredHubs]);
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

  const flipHub = useEffectEvent((hubId: string) => {
    const hub = nodes.find((node) => node.id === hubId);
    if (!hub) return;
    const axis = hub.position.x + (hub.measured?.width ?? 224) / 2;
    const positions = { ...saved.current };
    const moved = nodes.map((node) => {
      if (node.type !== "endpoint" || node.data.hubId !== hubId) return node;
      const width = node.measured?.width ?? 220;
      const position = { ...node.position, x: 2 * axis - node.position.x - width };
      positions[node.id] = position;
      return { ...node, position };
    });
    const next = new Set(mirroredHubs);
    if (!next.delete(hubId)) next.add(hubId);
    saved.current = positions;
    savePositions(positions);
    saveMirroredHubs(next);
    setNodes(moved);
    setMirroredHubs(next);
  });

  useEffect(() => {
    if (!inTauri) return;
    let disposed = false;
    const unsubscribe = listen<string>("topology-flip-horizontal", ({ payload }) => {
      if (!disposed) flipHub(payload);
    });
    return () => {
      disposed = true;
      void unsubscribe.then((unlisten) => unlisten()).catch(console.error);
    };
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
    if (!e.dataTransfer.types.some((type) => type === DRAG_MIME || type === REPLAY_MIME)) return;
    e.preventDefault();
    e.dataTransfer.dropEffect = "copy";
    setDropHub(hubAt(e.clientX, e.clientY));
  };

  const onDrop = (e: React.DragEvent) => {
    const format = SOURCE_FORMATS.find((format) => format === e.dataTransfer.getData(REPLAY_MIME));
    const port = e.dataTransfer.getData(DRAG_MIME);
    setDropHub(null);
    if (!port && !format) return;
    e.preventDefault();
    const hubId = hubAt(e.clientX, e.clientY);
    const isVirtual = ports.find((p) => p.name === port)?.kind === "virtual";
    const request = format ? replaySource(format) : isVirtual
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
    <div className="relative min-w-0 flex-1" data-mirror-label={t("topology.flipHorizontal")} onDragOver={onDragOver} onDrop={onDrop} onDragLeave={() => setDropHub(null)}>
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
        connectionMode={ConnectionMode.Loose}
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
        {HUB_KINDS.map((k) => (
          <ToolbarButton key={k} kind={k} />
        ))}
        <button className="btn-outline bg-ink-850/90" onClick={() => openDialog({ type: "pair" })}>
          <Cable size={15} /> {t("topology.virtualPair")}
        </button>
      </div>
      <div className="absolute top-3 right-3 flex gap-2">
        <button className="btn-outline bg-ink-850/90" onClick={resetLayout} title={t("topology.autoLayoutTip")}>
          <LayoutGrid size={15} /> {t("topology.autoLayout")}
        </button>
        <BackupMenu nodes={nodes} mirroredHubs={mirroredHubs} onImported={onImported} />
      </div>

      {empty && <EmptyState />}
    </div>
  );
}

/** Saves or restores connections, virtual ports and the layout as a JSON file. */
function BackupMenu({
  nodes,
  mirroredHubs,
  onImported,
}: {
  nodes: GraphNode[];
  mirroredHubs: ReadonlySet<string>;
  onImported: () => void;
}) {
  const { run, toast } = useStore();
  const t = useT();
  const [open, setOpen] = useState(false);
  const menu = useRef<HTMLDivElement>(null);
  const fileInput = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (!open) return;
    const onPointer = (e: PointerEvent) => {
      if (!menu.current?.contains(e.target as Node)) setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && setOpen(false);
    window.addEventListener("pointerdown", onPointer);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("pointerdown", onPointer);
      window.removeEventListener("keydown", onKey);
    };
  }, [open]);

  const exportJson = () =>
    run(t("topology.export"), async () => {
      const positions = Object.fromEntries(nodes.map(({ id, position }) => [id, position]));
      if (await exportTopology(positions, mirroredHubs)) toast("success", t("topology.backupExported"));
    });

  const importJson = (file: File) =>
    run(t("topology.import"), async () => {
      const { positions, mirroring } = await importTopology(await file.text());
      savePositions({ ...loadPositions(), ...positions });
      if (mirroring.size) {
        const mirrored = loadMirroredHubs();
        for (const [id, on] of mirroring) {
          if (on) mirrored.add(id);
          else mirrored.delete(id);
        }
        saveMirroredHubs(mirrored);
      }
      onImported();
      toast("success", t("topology.backupImported"));
    });

  const item =
    "flex w-full items-center gap-2 rounded-md px-2.5 py-1.5 text-left text-sm text-ink-200 hover:bg-ink-800 hover:text-ink-0";
  return (
    <div ref={menu} className="relative">
      <input
        ref={fileInput}
        type="file"
        accept=".json,application/json"
        hidden
        onChange={(e) => {
          const file = e.currentTarget.files?.[0];
          e.currentTarget.value = "";
          if (file) void importJson(file);
        }}
      />
      <button
        className="btn-outline bg-ink-850/90"
        aria-haspopup="menu"
        aria-expanded={open}
        onClick={() => setOpen((o) => !o)}
        title={t("topology.backupTip")}
      >
        <FileJson size={15} /> {t("topology.backup")} <ChevronDown size={14} />
      </button>
      {open && (
        <div role="menu" className="panel absolute right-0 z-20 mt-1.5 w-52 p-1 shadow-xl shadow-black/40">
          <button
            role="menuitem"
            className={item}
            onClick={() => {
              setOpen(false);
              void exportJson();
            }}
          >
            <Upload size={14} /> {t("topology.export")}
          </button>
          <button
            role="menuitem"
            className={item}
            onClick={() => {
              setOpen(false);
              fileInput.current?.click();
            }}
          >
            <Download size={14} /> {t("topology.import")}
          </button>
        </div>
      )}
    </div>
  );
}

const KIND_ICON = { split: Split, join: Merge, bridge: ArrowLeftRight, bus: RadioTower };

function ToolbarButton({ kind }: { kind: HubKind }) {
  const openDialog = useStore((s) => s.openDialog);
  const Icon = KIND_ICON[kind];
  useT();
  return (
    <button
      className="btn-outline bg-ink-850/90"
      onClick={() => openDialog({ type: "hub", preset: { kind } })}
      title={hubDescription(kind)}
    >
      <Icon size={15} style={{ color: HUB_COLOR[kind] }} /> {hubLabel(kind)}
    </button>
  );
}

function EmptyState() {
  const openDialog = useStore((s) => s.openDialog);
  const t = useT();
  return (
    <div className="pointer-events-none absolute inset-0 grid place-items-center">
      <div className="panel pointer-events-auto max-w-md p-6 text-center">
        <img src="/logo.svg" alt="" className="mx-auto mb-3 h-14 w-14" />
        <h2 className="text-lg font-semibold text-ink-0">{t("topology.emptyTitle")}</h2>
        <p className="mt-1.5 text-sm text-ink-300">{t("topology.emptyText")}</p>
        <div className="mt-4 flex justify-center gap-2">
          <button className="btn-primary" onClick={() => openDialog({ type: "hub", preset: { kind: "split" } })}>
            <Split size={15} /> {t("topology.splitPort")}
          </button>
          <button className="btn-outline" onClick={() => openDialog({ type: "pair" })}>
            <Plus size={15} /> {t("topology.virtualPair")}
          </button>
        </div>
      </div>
    </div>
  );
}

function PortPalette() {
  const { ports, hubs, openDialog } = useStore();
  const t = useT();
  const visible = ports;
  const hubName = (id: string | null) => hubs.find((h) => h.spec.id === id)?.spec.name;

  return (
    <aside className="flex w-64 shrink-0 flex-col border-r border-ink-700/70 bg-ink-900/60">
      <div className="px-4 pt-4 pb-2">
        <h3 className="text-xs font-semibold tracking-wider text-ink-300 uppercase">{t("nav.ports")}</h3>
        <p className="mt-1 text-[11px] leading-snug text-ink-400">{t("topology.paletteHint")}</p>
      </div>
      <div className="min-h-0 flex-1 space-y-1.5 overflow-y-auto px-3 pb-3">
        {SOURCE_FORMATS.map((format) => (
          <button key={format} type="button" draggable className="btn-outline w-full justify-start"
            onDragStart={(event) => {
              event.dataTransfer.setData(REPLAY_MIME, format);
              event.dataTransfer.effectAllowed = "copy";
            }}
            onClick={() => openDialog({ type: "hub", preset: { kind: "split", master: replaySource(format) } })}>
            <PortIcon kind={isGps(format) ? "gps" : "replay"} size={14} />
            {t(`editor.${format}`)}
          </button>
        ))}
        {visible.length === 0 && <p className="px-1 py-6 text-center text-xs text-ink-400">{t("common.noSerialPorts")}</p>}
        {visible.map((p) => (
          <PaletteItem key={p.name} port={p} busyBy={hubName(p.inUseBy)} />
        ))}
      </div>
    </aside>
  );
}

function PaletteItem({ port, busyBy }: { port: PortInfo; busyBy?: string }) {
  const t = useT();
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
          {busyBy ? t("topology.inUse", { hub: busyBy }) : (port.description ?? t("topology.serialPort"))}
        </div>
      </div>
    </div>
  );
}
