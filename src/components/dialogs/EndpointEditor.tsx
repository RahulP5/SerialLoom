import { Trash } from "lucide-react";
import { useStore } from "../../store";
import { DEFAULT_SERIAL, type EndpointRequest, type SerialSettings } from "../../lib/types";
import { useT } from "../../i18n";

const BAUDS = [300, 1200, 2400, 4800, 9600, 19200, 38400, 57600, 115200, 230400, 460800, 921600];

type Kind = EndpointRequest["kind"];

const KINDS: Kind[] = ["newVirtual", "virtual", "serial", "tcpServer", "tcpClient"];

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
  const t = useT();
  const free = ports.filter((p) => !p.inUseBy);
  const realPorts = free.filter((p) => p.kind !== "virtual");
  const virtualPorts = free.filter((p) => p.kind === "virtual");

  const switchKind = (k: Kind) => onChange(defaultFor(k, (k === "virtual" ? virtualPorts : realPorts)[0]?.name));

  const portSelect = (choices: typeof free, current: string, set: (port: string) => void, empty: string) => (
    <select className="input min-w-0 flex-1 font-mono" value={current} onChange={(e) => set(e.target.value)}>
      <option value="">{choices.length ? t("editor.choosePort") : empty}</option>
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
        <select className="input w-56 shrink-0" value={value.kind} onChange={(e) => switchKind(e.target.value as Kind)}>
          {KINDS.map((k) => (
            <option key={k} value={k}>
              {t(`editor.${k}`)}
            </option>
          ))}
        </select>

        {value.kind === "serial" &&
          portSelect(realPorts, value.port, (port) => onChange({ ...value, port }), t("editor.noFreeSerial"))}
        {value.kind === "virtual" &&
          portSelect(virtualPorts, value.port, (port) => onChange({ ...value, port }), t("editor.noFreeVirtual"))}
        {value.kind === "newVirtual" && (
          <input
            className="input min-w-0 flex-1 font-mono"
            value={value.name}
            onChange={(e) => onChange({ ...value, name: e.target.value.toUpperCase() })}
            placeholder={t("editor.newVirtualPlaceholder")}
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
            placeholder={t("editor.hostPort")}
            spellCheck={false}
          />
        )}
        {onRemove && (
          <button className="btn-ghost shrink-0 px-2 text-ink-400 hover:text-rose-300" onClick={onRemove} title={t("common.remove")}>
            <Trash size={15} />
          </button>
        )}
      </div>
      {value.kind === "serial" && (
        <SerialSettingsRow value={value.settings} onChange={(settings) => onChange({ ...value, settings })} indent={!!label} />
      )}
      {(value.kind === "newVirtual" || value.kind === "virtual") && (
        <p className={label ? "mt-1.5 pl-22 text-[11px] text-ink-400" : "mt-1.5 text-[11px] text-ink-400"}>
          {t("editor.virtualNote")}
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
  const t = useT();
  const set = <K extends keyof SerialSettings>(k: K, v: SerialSettings[K]) => onChange({ ...value, [k]: v });
  return (
    <div className={indent ? "mt-2 pl-22" : "mt-2"}>
      <div className="flex flex-wrap gap-1.5">
        <select className="input w-auto" value={value.baudRate} onChange={(e) => set("baudRate", Number(e.target.value))}>
          {(BAUDS.includes(value.baudRate) ? BAUDS : [...BAUDS, value.baudRate]).map((b) => (
            <option key={b} value={b}>
              {t("editor.baud", { baud: b })}
            </option>
          ))}
        </select>
        <select className="input w-auto" value={value.dataBits} onChange={(e) => set("dataBits", Number(e.target.value))}>
          {[8, 7, 6, 5].map((b) => (
            <option key={b} value={b}>
              {t("editor.bits", { bits: b })}
            </option>
          ))}
        </select>
        <select
          className="input w-auto"
          value={value.parity}
          onChange={(e) => set("parity", e.target.value as SerialSettings["parity"])}
        >
          <option value="none">{t("editor.noParity")}</option>
          <option value="even">{t("editor.even")}</option>
          <option value="odd">{t("editor.odd")}</option>
        </select>
        <select
          className="input w-auto"
          value={value.stopBits}
          onChange={(e) => set("stopBits", e.target.value as SerialSettings["stopBits"])}
        >
          <option value="one">{t("editor.stop1")}</option>
          <option value="two">{t("editor.stop2")}</option>
        </select>
        <select
          className="input w-auto"
          value={value.flowControl}
          onChange={(e) => set("flowControl", e.target.value as SerialSettings["flowControl"])}
        >
          <option value="none">{t("editor.noFlow")}</option>
          <option value="hardware">RTS/CTS</option>
          <option value="software">XON/XOFF</option>
        </select>
      </div>
    </div>
  );
}
