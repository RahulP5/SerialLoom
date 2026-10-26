import { Bluetooth, Cable, Cpu, Globe, Plug, Usb } from "lucide-react";
import type { EndpointIcon } from "./graph";

const ICONS = {
  usb: Usb,
  pci: Cpu,
  bluetooth: Bluetooth,
  virtual: Cable,
  tcp: Globe,
  unknown: Plug,
} as const;

export const KIND_LABEL: Record<EndpointIcon, string> = {
  usb: "USB",
  pci: "Built-in",
  bluetooth: "Bluetooth",
  virtual: "Virtual",
  tcp: "Network",
  unknown: "Serial",
};

export const KIND_COLOR: Record<EndpointIcon, string> = {
  usb: "#60a5fa",
  pci: "#94a3b8",
  bluetooth: "#38bdf8",
  virtual: "#a78bfa",
  tcp: "#34d399",
  unknown: "#94a3b8",
};

export function PortIcon({ kind, size = 16 }: { kind: EndpointIcon; size?: number }) {
  const Icon = ICONS[kind] ?? Plug;
  return <Icon size={size} strokeWidth={2} />;
}

export function KindBadge({ kind }: { kind: EndpointIcon }) {
  const color = KIND_COLOR[kind];
  return (
    <span className="chip" style={{ color, background: `${color}1f` }}>
      <PortIcon kind={kind} size={12} />
      {KIND_LABEL[kind]}
    </span>
  );
}
