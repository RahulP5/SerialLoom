import { useMemo, useState } from "react";
import { Plus } from "lucide-react";
import clsx from "clsx";
import { Modal } from "./Modal";
import { EndpointEditor, defaultFor } from "./EndpointEditor";
import { useStore } from "../../store";
import { api } from "../../lib/api";
import { HUB_META, specTitle } from "../../lib/format";
import { HUB_ICON } from "../nodes/HubNode";
import type { CreateHubRequest, EndpointRequest, HubKind } from "../../lib/types";

const ROLE_LABELS: Record<HubKind, [string, string]> = {
  split: ["Source", "Output"],
  join: ["Combined", "Input"],
  bridge: ["Side A", "Side B"],
  bus: ["Node 1", "Node"],
};

function defaults(kind: HubKind, firstFree?: string): Pick<CreateHubRequest, "master" | "members"> {
  const existing = defaultFor("serial", firstFree);
  const virt = defaultFor("newVirtual");
  switch (kind) {
    case "split":
      return { master: existing, members: [virt, virt] };
    case "join":
      return { master: virt, members: [existing, defaultFor("serial")] };
    case "bridge":
      return { master: existing, members: [defaultFor("tcpServer")] };
    case "bus":
      return { master: virt, members: [virt, virt] };
  }
}

export function HubWizard({ preset, onClose }: { preset?: Partial<CreateHubRequest>; onClose: () => void }) {
  const { ports, run, select, busy } = useStore();
  const firstFree = ports.find((p) => !p.inUseBy && p.kind !== "virtual")?.name;

  const initialKind = preset?.kind ?? "split";
  const base = defaults(initialKind, firstFree);
  const [kind, setKind] = useState<HubKind>(initialKind);
  const [name, setName] = useState(preset?.name ?? "");
  const [master, setMaster] = useState<EndpointRequest>(preset?.master ?? base.master);
  const [members, setMembers] = useState<EndpointRequest[]>(preset?.members ?? base.members);
  const [autostart, setAutostart] = useState(true);
  const [start, setStart] = useState(true);

  const switchKind = (k: HubKind) => {
    const d = defaults(k, firstFree);
    setKind(k);
    // Keep a chosen existing source port when switching between templates.
    setMaster(k === "split" || k === "bridge" ? (master.kind === "serial" ? master : d.master) : d.master);
    setMembers(d.members);
  };

  const [masterLabel, memberLabel] = ROLE_LABELS[kind];
  const autoName = useMemo(() => `${HUB_META[kind].label} ${specTitle(master)}`, [kind, master]);
  const virtualCount = [master, ...members].filter((e) => e.kind === "newVirtual").length;

  const submit = async () => {
    const request: CreateHubRequest = {
      name: name.trim() || autoName,
      kind,
      master,
      members,
      autostart,
      start,
    };
    const created = await run(
      virtualCount ? "Creating virtual ports (Windows may ask for permission)" : "Creating hub",
      () => api.createHub(request),
      `${HUB_META[kind].label} "${request.name}" created`,
    );
    if (created) {
      select({ type: "hub", id: created.spec.id });
      onClose();
    }
  };

  const Icon = HUB_ICON[kind];
  const meta = HUB_META[kind];

  return (
    <Modal
      title="New connection"
      subtitle="Choose how the ports should be woven together."
      onClose={onClose}
      width={760}
      icon={
        <div className="grid h-9 w-9 place-items-center rounded-xl" style={{ background: `${meta.color}22`, color: meta.color }}>
          <Icon size={18} />
        </div>
      }
      footer={
        <>
          <span className="mr-auto self-center text-xs text-ink-400">
            {virtualCount > 0 &&
              `${virtualCount} virtual port${virtualCount > 1 ? "s" : ""} will be created. Windows asks for administrator approval once.`}
          </span>
          <button className="btn-ghost" onClick={onClose}>
            Cancel
          </button>
          <button className="btn-primary" onClick={submit} disabled={!!busy}>
            {busy ? "Working…" : `Create ${meta.label.toLowerCase()}`}
          </button>
        </>
      }
    >
      <div className="grid grid-cols-2 gap-2 md:grid-cols-4">
        {(Object.keys(HUB_META) as HubKind[]).map((k) => {
          const KIcon = HUB_ICON[k];
          const m = HUB_META[k];
          return (
            <button
              key={k}
              onClick={() => switchKind(k)}
              className={clsx(
                "rounded-xl border p-3 text-left transition-colors",
                k === kind ? "bg-ink-800" : "border-ink-700 hover:border-ink-500",
              )}
              style={k === kind ? { borderColor: m.color } : undefined}
            >
              <KIcon size={18} style={{ color: m.color }} />
              <div className="mt-1.5 text-sm font-semibold text-white">{m.label}</div>
              <div className="mt-0.5 text-[11px] leading-snug text-ink-300">{m.describe}</div>
            </button>
          );
        })}
      </div>

      <Preview kind={kind} master={master} members={members} />

      <label className="label mt-4">Name</label>
      <input className="input" value={name} onChange={(e) => setName(e.target.value)} placeholder={autoName} />

      <div className="mt-4 space-y-2">
        <EndpointEditor label={masterLabel} value={master} onChange={setMaster} />
        {members.map((m, i) => (
          <EndpointEditor
            key={i}
            label={kind === "bridge" ? memberLabel : `${memberLabel} ${kind === "bus" ? i + 2 : i + 1}`}
            value={m}
            onChange={(v) => setMembers(members.map((x, j) => (j === i ? v : x)))}
            onRemove={kind !== "bridge" && members.length > 1 ? () => setMembers(members.filter((_, j) => j !== i)) : undefined}
          />
        ))}
        {kind !== "bridge" && (
          <button
            className="btn-outline w-full border-dashed"
            onClick={() => setMembers([...members, kind === "join" ? defaultFor("serial") : defaultFor("newVirtual")])}
          >
            <Plus size={15} /> Add {memberLabel.toLowerCase()}
          </button>
        )}
      </div>

      <div className="mt-4 flex gap-5 text-sm text-ink-200">
        <label className="flex items-center gap-2">
          <input type="checkbox" checked={start} onChange={(e) => setStart(e.target.checked)} className="accent-loom-500" />
          Start now
        </label>
        <label className="flex items-center gap-2">
          <input
            type="checkbox"
            checked={autostart}
            onChange={(e) => setAutostart(e.target.checked)}
            className="accent-loom-500"
          />
          Start when SerialLoom opens
        </label>
      </div>
    </Modal>
  );
}

/** Small live diagram of what will be created. */
function Preview({ kind, master, members }: { kind: HubKind; master: EndpointRequest; members: EndpointRequest[] }) {
  const color = HUB_META[kind].color;
  const rowH = 30;
  const h = Math.max(members.length * rowH, rowH) + 20;
  const cy = h / 2;
  const W = 680;
  const hubX = W / 2;
  const label = (e: EndpointRequest) => specTitle(e) || "—";
  const isVirtual = (e: EndpointRequest) => e.kind === "newVirtual";
  const arrows = kind === "bus" ? "↔" : kind === "join" ? "←" : kind === "split" ? "→" : "↔";

  return (
    <div className="mt-4 rounded-xl border border-ink-700 bg-ink-950/60 px-2 py-1">
      <svg viewBox={`0 0 ${W} ${h}`} className="w-full" style={{ height: h }}>
        {members.map((_, i) => {
          const y = 10 + i * rowH + rowH / 2;
          return (
            <path
              key={i}
              d={`M ${hubX} ${cy} C ${hubX + 90} ${cy}, ${hubX + 90} ${y}, ${W - 170} ${y}`}
              fill="none"
              stroke={color}
              strokeOpacity={0.6}
              strokeWidth={2}
            />
          );
        })}
        <line x1={170} y1={cy} x2={hubX} y2={cy} stroke={color} strokeOpacity={0.6} strokeWidth={2} />
        <Pill x={10} y={cy} w={160} text={label(master)} virtual={isVirtual(master)} />
        <circle cx={hubX} cy={cy} r={13} fill={`${color}33`} stroke={color} strokeWidth={2} />
        <text x={hubX} y={cy + 4} textAnchor="middle" fontSize={12} fill={color}>
          {arrows}
        </text>
        {members.map((m, i) => (
          <Pill key={i} x={W - 170} y={10 + i * rowH + rowH / 2} w={160} text={label(m)} virtual={isVirtual(m)} />
        ))}
      </svg>
    </div>
  );
}

function Pill({ x, y, w, text, virtual }: { x: number; y: number; w: number; text: string; virtual: boolean }) {
  return (
    <g>
      <rect
        x={x}
        y={y - 11}
        width={w}
        height={22}
        rx={7}
        fill="#131a2e"
        stroke={virtual ? "#a78bfa" : "#3a4670"}
        strokeDasharray={virtual ? "4 3" : undefined}
      />
      <text x={x + w / 2} y={y + 4} textAnchor="middle" fontSize={11.5} fontFamily="Cascadia Code, Consolas, monospace" fill="#e6eaf5">
        {text.length > 22 ? `${text.slice(0, 21)}…` : text}
        {virtual ? "  (new)" : ""}
      </text>
    </g>
  );
}
