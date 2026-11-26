import type { Edge, Node } from "@xyflow/react";
import type { Rate } from "../store";
import type { EndpointStatus, HubEndpoint, HubView, PortInfo, PortKind } from "../lib/types";
import { endpointSubtitle, endpointTitle, HUB_COLOR } from "../lib/format";

export type EndpointIcon = PortKind | "tcp";

export interface EndpointNodeData extends Record<string, unknown> {
  title: string;
  subtitle: string;
  icon: EndpointIcon;
  role: "master" | "member";
  hubId?: string;
  mirrored: boolean;
  color: string;
  running: boolean;
  status?: EndpointStatus;
  rate?: Rate;
}

export interface HubNodeData extends Record<string, unknown> {
  hub: HubView;
  mirrored: boolean;
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
  mirroredHubs: ReadonlySet<string> = new Set(),
): { nodes: GraphNode[]; edges: Edge[] } {
  const nodes: GraphNode[] = [];
  const edges: Edge[] = [];
  const tcpServers = new Map<string, string[]>();
  const tcpClients: { id: string; address: string; isMaster: boolean }[] = [];
  let y = 0;

  for (const hub of hubs) {
    const { spec, status } = hub;
    const color = HUB_COLOR[spec.kind];
    const hubRates = rates[spec.id] ?? [];
    const rows = Math.max(1, spec.members.length);
    const bandHeight = rows * ROW;
    const centerY = y + (bandHeight - ROW) / 2;
    const all = [spec.master, ...spec.members];
    const mirrored = mirroredHubs.has(spec.id);

    const epNode = (e: HubEndpoint, i: number, x: number, ny: number): EndpointNode => ({
      id: `${spec.id}:${i}`,
      type: "endpoint",
      position: { x: mirrored ? 2 * X_HUB + 4 - x : x, y: ny },
      data: {
        title: endpointTitle(e),
        subtitle: endpointSubtitle(e, status.running, status.endpoints[i]),
        icon: iconFor(e, ports),
        role: i === 0 ? "master" : "member",
        hubId: spec.id,
        mirrored,
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
        mirrored,
        color,
        totalRate: hubRates.reduce((acc, r) => acc + r.rx, 0),
      },
    });
    spec.members.forEach((m, j) => nodes.push(epNode(m, j + 1, X_MEMBER, y + j * ROW)));

    all.forEach(({ spec: endpoint }, i) => {
      const id = `${spec.id}:${i}`;
      if (endpoint.kind === "tcpServer") {
        const address = endpoint.bind.trim().toLowerCase();
        if (address) {
          const servers = tcpServers.get(address) ?? [];
          servers.push(id);
          tcpServers.set(address, servers);
        }
      } else if (endpoint.kind === "tcpClient") {
        tcpClients.push({ id, address: endpoint.address.trim().toLowerCase(), isMaster: i === 0 });
      }

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

  // TCP links represent configured targets, even when either connection is stopped.
  for (const { id, address, isMaster } of tcpClients) {
    const servers = tcpServers.get(address);
    if (servers?.length !== 1) continue;
    edges.push({
      id: `tcp:${servers[0]}:${id}`,
      source: servers[0],
      target: id,
      sourceHandle: "r",
      // The hub uses the right handle of a master and the left handle of a member.
      targetHandle: isMaster ? "l" : "r",
      selectable: false,
      ariaLabel: `TCP ${address}`,
      style: { stroke: "#34d399", strokeWidth: 2, strokeDasharray: "6 6" },
    });
  }

  return { nodes, edges };
}
