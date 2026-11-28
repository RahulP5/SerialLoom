import { open } from "@tauri-apps/plugin-dialog";
import { inTauri } from "../../lib/api";
import { Trash } from "lucide-react";
import { useStore } from "../../store";
import {
  DEFAULT_SERIAL, DEFAULT_GPS, FILE_FORMATS, GPS_FIXES, NMEA_SENTENCES, SOURCE_FORMATS, isGps, withGpsDefaults,
  type GpsFix, type GpsSettings, type SourceFormat, type EndpointRequest, type ReplayFormat, type ReplaySettings,
  type SerialSettings,
} from "../../lib/types";
import { useT, type MessageKey } from "../../i18n";

const BAUDS = [300, 1200, 2400, 4800, 9600, 19200, 38400, 57600, 115200, 230400, 460800, 921600];

type Kind = Exclude<EndpointRequest["kind"], "replay"> | SourceFormat;

export function replaySource(format: ReplayFormat): EndpointRequest {
  return { kind: "replay", settings: {
    path: "", format, ...(isGps(format) ? { gps: DEFAULT_GPS } : {}), repeat: true, timestamped: format === "nmea2000",
    bytesPerSecond: format === "nmea2000" ? 11520 : 480, cycleMs: 1000, linesPerCycle: 1,
  } };
}

const KINDS: Kind[] = ["newVirtual", "virtual", "serial", "tcpServer", "tcpClient", "udp", ...SOURCE_FORMATS];

export function defaultFor(kind: Kind, firstFree?: string): EndpointRequest {
  switch (kind) {
    case "file":
    case "gps0183":
    case "gps2000":
      return replaySource(kind);
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
    case "udp":
      return { kind, settings: { bind: "127.0.0.1:10110", destination: "", receive: true, framing: "raw" } };
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
        <select className="input w-56 shrink-0" value={value.kind === "replay" ? (isGps(value.settings.format) ? value.settings.format : "file") : value.kind} onChange={(e) => switchKind(e.target.value as Kind)}>
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
      {value.kind === "udp" && (
        <div className="mt-2 flex flex-wrap gap-3">
          <label className="flex items-center gap-2 text-xs">
            {t("udp.local")}
            <input className="input w-44 font-mono" value={value.settings.bind} placeholder="127.0.0.1:10110"
              spellCheck={false} onChange={(e) => onChange({ ...value, settings: { ...value.settings, bind: e.target.value } })} />
          </label>
          <label className="flex items-center gap-2 text-xs">
            <input type="checkbox" checked={value.settings.receive}
              onChange={(e) => onChange({ ...value, settings: { ...value.settings, receive: e.target.checked } })} />
            {t("udp.receive")}
          </label>
          <label className="flex items-center gap-2 text-xs">
            {t("udp.destination")}
            <input className="input w-44 font-mono" value={value.settings.destination} placeholder="127.0.0.1:10110"
              spellCheck={false} onChange={(e) => onChange({ ...value, settings: { ...value.settings, destination: e.target.value } })} />
          </label>
          {!!value.settings.destination.trim() && (
            <label className="flex items-center gap-2 text-xs">
              {t("udp.framing")}
              <select className="input w-auto" value={value.settings.framing}
                onChange={(e) => onChange({ ...value, settings: { ...value.settings,
                  framing: e.target.value === "nmea0183" ? "nmea0183" : "raw" } })}>
                <option value="raw">{t("replay.raw")}</option>
                <option value="nmea0183">{t("editor.nmea0183")}</option>
              </select>
            </label>
          )}
        </div>
      )}
      {value.kind === "replay" && (
        <ReplaySettingsRow value={value.settings} onChange={(settings) => onChange({ kind: "replay", settings })} />
      )}
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

type GpsNumber = "latitude" | "longitude" | "course" | "speed" | "altitude" | "satellites" | "hdop" | "vdop" |
  "pdop" | "variation" | "noise";

/** A GPS simulator's settings, as used when creating one and to change a running one. */
export function GpsFields({ gps, onChange, nmea0183 }: {
  gps: GpsSettings;
  onChange: (gps: GpsSettings) => void;
  /** NMEA 0183 (talker and sentences); otherwise NMEA 2000. */
  nmea0183: boolean;
}) {
  const t = useT();
  const set = <K extends keyof GpsSettings>(key: K, value: GpsSettings[K]) => onChange({ ...gps, [key]: value });
  const number = (key: GpsNumber, width = "w-24") => (
    <label className="flex items-center gap-2 text-xs">
      {t(`gps.${key}`)}
      <input className={`input ${width}`} type="number" step="any" value={gps[key]}
        onChange={(event) => set(key, event.target.value)} />
    </label>
  );
  const group = (title: MessageKey, children: React.ReactNode) => (
    <fieldset>
      <legend className="text-[11px] font-semibold tracking-wider text-ink-400 uppercase">{t(title)}</legend>
      <div className="mt-1.5 flex flex-wrap items-center gap-3">{children}</div>
    </fieldset>
  );
  return (
    <div className="space-y-3">
      {group("gps.position", <>
        {number("latitude", "w-28")}
        {number("longitude", "w-28")}
        <label className="flex items-center gap-2 text-xs">
          {t("gps.motion")}
          <select className="input w-auto" value={gps.motion}
            onChange={(event) => set("motion", event.target.value as GpsSettings["motion"])}>
            <option value="moving">{t("gps.motionMoving")}</option>
            <option value="fixed">{t("gps.motionFixed")}</option>
          </select>
        </label>
        {number("course")}
        {gps.motion === "moving" && number("speed")}
        {number("noise")}
      </>)}
      {group("gps.quality", <>
        <label className="flex items-center gap-2 text-xs">
          {t("gps.fix")}
          <select className="input w-auto" value={gps.fix} onChange={(event) => set("fix", event.target.value as GpsFix)}>
            {GPS_FIXES.map((fix) => <option key={fix} value={fix}>{t(`gps.fix.${fix}`)}</option>)}
          </select>
        </label>
        {number("satellites", "w-16")}
        {number("hdop", "w-20")}
        {number("vdop", "w-20")}
        {number("pdop", "w-20")}
        {number("altitude")}
        {number("variation")}
      </>)}
      {nmea0183
        ? group("gps.output", <>
          <label className="flex items-center gap-2 text-xs">
            {t("gps.talker")}
            <select className="input w-auto" value={gps.talker}
              onChange={(event) => set("talker", event.target.value as GpsSettings["talker"])}>
              <option value="GP">GP</option>
              <option value="GN">GN</option>
            </select>
          </label>
          {NMEA_SENTENCES.map((sentence) => (
            <label key={sentence} className="flex items-center gap-1.5 font-mono text-xs" title={t(`gps.sentence.${sentence}`)}>
              <input type="checkbox" checked={gps.sentences.includes(sentence)}
                onChange={(event) => set("sentences", NMEA_SENTENCES.filter((s) =>
                  s === sentence ? event.target.checked : gps.sentences.includes(s)))} />
              {sentence}
            </label>
          ))}
        </>)
        : <p className="text-xs text-ink-400">{t("gps.pgns")}</p>}
    </div>
  );
}

function ReplaySettingsRow({ value, onChange }: {
  value: ReplaySettings;
  onChange: (settings: ReplaySettings) => void;
}) {
  const t = useT();
  const { toast } = useStore();
  const raw = value.format === "file" || (value.format === "nmea2000" && !value.timestamped);
  const numeric = (key: "bytesPerSecond" | "cycleMs" | "linesPerCycle", label: string, max: number) => (
    <label className="flex items-center gap-2 text-xs">
      {label}
      <input type="number" className="input w-28" min={1} max={max} step={1}
        value={value[key]} onChange={(event) => onChange({ ...value, [key]: Number(event.target.value) })} />
    </label>
  );

  if (isGps(value.format)) {
    return (
      <div className="mt-2 space-y-3">
        <GpsFields gps={withGpsDefaults(value.gps)} nmea0183={value.format === "gps0183"}
          onChange={(gps) => onChange({ ...value, gps })} />
        <div className="flex flex-wrap gap-3">{numeric("cycleMs", t("gps.interval"), 60_000)}</div>
      </div>
    );
  }

  return (
    <div className="mt-2 space-y-2">
      <label className="flex items-center gap-2 text-xs">
        {t("replay.format")}
        <select className="input w-auto" value={value.format}
          onChange={(event) => {
            const format = event.target.value as typeof FILE_FORMATS[number];
            onChange({ ...value, format, timestamped: format === "nmea2000",
              bytesPerSecond: format === "nmea2000" ? 11520 : 480 });
          }}>
          {FILE_FORMATS.map((format) => (
            <option key={format} value={format}>{t(format === "file" ? "replay.raw" : `editor.${format}`)}</option>
          ))}
        </select>
      </label>
      <div className="flex gap-2">
        <input className="input min-w-0 flex-1 font-mono" aria-label={t("replay.path")}
          placeholder={t("replay.path")} value={value.path}
          onChange={(event) => onChange({ ...value, path: event.target.value })} />
        {inTauri && <button type="button" className="btn-outline" onClick={async () => {
          try {
            const path = await open({ multiple: false, directory: false });
            if (typeof path === "string") onChange({ ...value, path });
          } catch (error) {
            toast("error", String(error));
          }
        }}>{t("replay.browse")}</button>}
      </div>
      <div className="flex flex-wrap gap-3">
        <label className="flex items-center gap-2 text-xs">
          <input type="checkbox" checked={value.repeat}
            onChange={(event) => onChange({ ...value, repeat: event.target.checked })} />
          {t("replay.repeat")}
        </label>
        {value.format !== "file" && (
          <label className="flex items-center gap-2 text-xs">
            <input type="checkbox" checked={value.timestamped}
              onChange={(event) => onChange({ ...value, timestamped: event.target.checked })} />
            {t("replay.timestamped")}
          </label>
        )}
        {raw && numeric("bytesPerSecond", t("replay.rate"), 1_000_000)}
        {!raw && (!value.timestamped || value.repeat) &&
          numeric("cycleMs", t(value.timestamped ? "replay.loopGap" : "replay.cycle"), 60_000)}
        {value.format === "nmea0183" && !value.timestamped && numeric("linesPerCycle", t("replay.lines"), 256)}
      </div>
      {value.timestamped && <p className="text-xs text-ink-400">
        {t(value.format === "nmea2000" ? "replay.hexFormat" : "replay.textFormat")}
      </p>}
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
