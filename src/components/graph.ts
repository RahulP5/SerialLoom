import type { Edge, Node } from "@xyflow/react";
import type { Rate } from "../store";
import type { EndpointStatus, HubEndpoint, HubView, PortInfo, PortKind } from "../lib/types";
import { endpointSubtitle, endpointTitle, HUB_META } from "../lib/format";

export type EndpointIcon = PortKind | "tcp";

export interface EndpointNodeData extends Record<string, unknown> {
  title: string;
  subtitle: string;
  icon: EndpointIcon;
  role: "master" | "member";
  hubId?: string;
  color: string;
  running: boolean;
  status?: EndpointStatus;
  rate?: Rate;
}

export interface HubNodeData extends Record<string, unknown> {
  hub: HubView;
  color: string;
  totalRate: number;
}

export interface TrafficEdgeData extends Record<string, unknown> {
  /** Bytes/s flowing source → target. */
  forward: number;
  /** Bytes/s flowing target → source. */
  backward: number;
  color: string;
  running: boolean;
  dashed?: boolean;
  label?: string;
}

export type EndpointNode = Node<EndpointNodeData, "endpoint">;
export type HubNode = Node<HubNodeData, "hub">;
export type GraphNode = EndpointNode | HubNode;
export type TrafficEdge = Edge<TrafficEdgeData, "traffic">;

const X_MASTER = 0;
const X_HUB = 320;
const X_MEMBER = 640;
const ROW = 104;
const GAP = 72;

function iconFor(e: HubEndpoint, ports: PortInfo[]): EndpointIcon {
  if (e.spec.kind === "virtual") return "virtual";
  if (e.spec.kind !== "serial") return "tcp";
  const name = e.spec.port.toUpperCase();
  return ports.find((p) => p.name.toUpperCase() === name)?.kind ?? "unknown";
}

/** Builds a left-to-right layout: master → hub → members, one band per hub. */
export function buildGraph(
  hubs: HubView[],
  ports: PortInfo[],
  rates: Record<string, Rate[]>,
): { nodes: GraphNode[]; edges: TrafficEdge[] } {
  const nodes: GraphNode[] = [];
  const edges: TrafficEdge[] = [];
  let y = 0;

  for (const hub of hubs) {
    const { spec, status } = hub;
    const color = HUB_META[spec.kind].color;
    const hubRates = rates[spec.id] ?? [];
    const rows = Math.max(1, spec.members.length);
    const bandHeight = rows * ROW;
    const centerY = y + (bandHeight - ROW) / 2;
    const all = [spec.master, ...spec.members];

    const epNode = (e: HubEndpoint, i: number, x: number, ny: number): EndpointNode => ({
      id: `${spec.id}:${i}`,
      type: "endpoint",
      position: { x, y: ny },
      data: {
        title: endpointTitle(e),
        subtitle: endpointSubtitle(e, status.endpoints[i]?.peerOpen),
        icon: iconFor(e, ports),
        role: i === 0 ? "master" : "member",
        hubId: spec.id,
        color,
        running: status.running,
        status: status.endpoints[i],
        rate: hubRates[i],
      },
    });

    nodes.push(epNode(spec.master, 0, X_MASTER, centerY));
    nodes.push({
      id: spec.id,
      type: "hub",
      position: { x: X_HUB, y: centerY + 4 },
      data: {
        hub,
        color,
        totalRate: hubRates.reduce((acc, r) => acc + r.rx, 0),
      },
    });
    spec.members.forEach((m, j) => nodes.push(epNode(m, j + 1, X_MEMBER, y + j * ROW)));

    all.forEach((_, i) => {
      const r = hubRates[i] ?? { rx: 0, tx: 0 };
      const isMaster = i === 0;
      edges.push({
        id: `e:${spec.id}:${i}`,
        type: "traffic",
        source: isMaster ? `${spec.id}:0` : spec.id,
        target: isMaster ? spec.id : `${spec.id}:${i}`,
        sourceHandle: "r",
        targetHandle: "l",
        // Source → target is "into the hub" for the master, "out to the port" for members.
        data: {
          forward: isMaster ? r.rx : r.tx,
          backward: isMaster ? r.tx : r.rx,
          color,
          running: status.running,
        },
      });
    });

    y += bandHeight + GAP;
  }

  return { nodes, edges };
}
