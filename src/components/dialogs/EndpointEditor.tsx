import { Trash } from "lucide-react";
import { useStore } from "../../store";
import { DEFAULT_SERIAL, type EndpointRequest, type SerialSettings } from "../../lib/types";

const BAUDS = [300, 1200, 2400, 4800, 9600, 19200, 38400, 57600, 115200, 230400, 460800, 921600];

type Kind = EndpointRequest["kind"];

const KIND_OPTIONS: { value: Kind; label: string }[] = [
  { value: "newVirtual", label: "New virtual port" },
  { value: "virtual", label: "Existing virtual port" },
  { value: "serial", label: "Real serial port" },
  { value: "tcpServer", label: "TCP server" },
  { value: "tcpClient", label: "TCP client" },
];

export function defaultFor(kind: Kind, firstFree?: string): EndpointRequest {
  switch (kind) {
    case "newVirtual":
      return { kind, name: "COM#" };
    case "virtual":
      return { kind, port: firstFree ?? "" };
    case "serial":
      return { kind, port: firstFree ?? "", settings: DEFAULT_SERIAL };
    case "tcpServer":
      return { kind, bind: "0.0.0.0:7000" };
    case "tcpClient":
      return { kind, address: "192.168.1.20:4001" };
  }
}

/** Edits one endpoint request: picks its type and the fields for that type. */
export function EndpointEditor({
  value,
  onChange,
  onRemove,
  label,
}: {
  value: EndpointRequest;
  onChange: (v: EndpointRequest) => void;
  onRemove?: () => void;
  label?: string;
}) {
  const ports = useStore((s) => s.ports);
  const free = ports.filter((p) => !p.inUseBy);
  const realPorts = free.filter((p) => p.kind !== "virtual");
  const virtualPorts = free.filter((p) => p.kind === "virtual");

  const switchKind = (k: Kind) => onChange(defaultFor(k, (k === "virtual" ? virtualPorts : realPorts)[0]?.name));

  const portSelect = (choices: typeof free, current: string, set: (port: string) => void, empty: string) => (
    <select className="input min-w-0 flex-1 font-mono" value={current} onChange={(e) => set(e.target.value)}>
      <option value="">{choices.length ? "Choose a port…" : empty}</option>
      {choices.map((p) => (
        <option key={p.name} value={p.name}>
          {p.name} {p.description ? `— ${p.description}` : ""}
        </option>
      ))}
      {current && !choices.some((p) => p.name === current) && <option value={current}>{current}</option>}
    </select>
  );

  return (
    <div className="rounded-xl border border-ink-700 bg-ink-900/60 p-3">
      <div className="flex items-center gap-2">
        {label && <span className="w-20 shrink-0 text-xs font-semibold text-ink-300">{label}</span>}
        <select className="input w-48 shrink-0" value={value.kind} onChange={(e) => switchKind(e.target.value as Kind)}>
          {KIND_OPTIONS.map((o) => (
            <option key={o.value} value={o.value}>
              {o.label}
            </option>
          ))}
        </select>

        {value.kind === "serial" &&
          portSelect(realPorts, value.port, (port) => onChange({ ...value, port }), "No free serial ports")}
        {value.kind === "virtual" &&
          portSelect(virtualPorts, value.port, (port) => onChange({ ...value, port }), "No free virtual ports")}
        {value.kind === "newVirtual" && (
          <input
            className="input min-w-0 flex-1 font-mono"
            value={value.name}
            onChange={(e) => onChange({ ...value, name: e.target.value.toUpperCase() })}
            placeholder="COM20, or COM# for the next free number"
            spellCheck={false}
          />
        )}
        {value.kind === "tcpServer" && (
          <input
            className="input min-w-0 flex-1 font-mono"
            value={value.bind}
            onChange={(e) => onChange({ ...value, bind: e.target.value })}
            placeholder="0.0.0.0:7000"
            spellCheck={false}
          />
        )}
        {value.kind === "tcpClient" && (
          <input
            className="input min-w-0 flex-1 font-mono"
            value={value.address}
            onChange={(e) => onChange({ ...value, address: e.target.value })}
            placeholder="host:port"
            spellCheck={false}
          />
        )}
        {onRemove && (
          <button className="btn-ghost shrink-0 px-2 text-ink-400 hover:text-rose-300" onClick={onRemove} title="Remove">
            <Trash size={15} />
          </button>
        )}
      </div>
      {value.kind === "serial" && (
        <SerialSettingsRow value={value.settings} onChange={(settings) => onChange({ ...value, settings })} indent={!!label} />
      )}
      {(value.kind === "newVirtual" || value.kind === "virtual") && (
        <p className={label ? "mt-1.5 pl-22 text-[11px] text-ink-400" : "mt-1.5 text-[11px] text-ink-400"}>
          Other programs open this port by name and pick their own baud rate. Virtual ports accept any setting.
        </p>
      )}
    </div>
  );
}

function SerialSettingsRow({
  value,
  onChange,
  indent,
}: {
  value: SerialSettings;
  onChange: (v: SerialSettings) => void;
  indent: boolean;
}) {
  const set = <K extends keyof SerialSettings>(k: K, v: SerialSettings[K]) => onChange({ ...value, [k]: v });
  return (
    <div className={indent ? "mt-2 pl-22" : "mt-2"}>
      <div className="flex flex-wrap gap-1.5">
        <select className="input w-32" value={value.baudRate} onChange={(e) => set("baudRate", Number(e.target.value))}>
          {(BAUDS.includes(value.baudRate) ? BAUDS : [...BAUDS, value.baudRate]).map((b) => (
            <option key={b} value={b}>
              {b} baud
            </option>
          ))}
        </select>
        <select className="input w-20" value={value.dataBits} onChange={(e) => set("dataBits", Number(e.target.value))}>
          {[8, 7, 6, 5].map((b) => (
            <option key={b} value={b}>
              {b} bits
            </option>
          ))}
        </select>
        <select
          className="input w-28"
          value={value.parity}
          onChange={(e) => set("parity", e.target.value as SerialSettings["parity"])}
        >
          <option value="none">No parity</option>
          <option value="even">Even</option>
          <option value="odd">Odd</option>
        </select>
        <select
          className="input w-24"
          value={value.stopBits}
          onChange={(e) => set("stopBits", e.target.value as SerialSettings["stopBits"])}
        >
          <option value="one">1 stop</option>
          <option value="two">2 stop</option>
        </select>
        <select
          className="input w-32"
          value={value.flowControl}
          onChange={(e) => set("flowControl", e.target.value as SerialSettings["flowControl"])}
        >
          <option value="none">No flow ctl</option>
          <option value="hardware">RTS/CTS</option>
          <option value="software">XON/XOFF</option>
        </select>
      </div>
    </div>
  );
}
