import { useState } from "react";
import { ArrowLeftRight, Cable, CircleAlert, Download, Hash, Plus, Split, Stethoscope, Trash } from "lucide-react";
import { useStore } from "../../store";
import { api } from "../../lib/api";
import { DEFAULT_SERIAL, type PortInfo } from "../../lib/types";
import { KindBadge } from "../PortIcon";

function Page({
  title,
  subtitle,
  actions,
  children,
}: {
  title: string;
  subtitle: string;
  actions?: React.ReactNode;
  children: React.ReactNode;
}) {
  return (
    <div className="h-full overflow-y-auto px-8 py-7">
      <div className="mx-auto max-w-5xl">
        <div className="mb-5 flex items-end gap-4">
          <div className="flex-1">
            <h1 className="text-xl font-semibold text-ink-0">{title}</h1>
            <p className="mt-1 text-sm text-ink-300">{subtitle}</p>
          </div>
          {actions}
        </div>
        {children}
      </div>
    </div>
  );
}

function endpointFor(p: PortInfo) {
  return p.kind === "virtual"
    ? ({ kind: "virtual", port: p.name } as const)
    : ({ kind: "serial", port: p.name, settings: DEFAULT_SERIAL } as const);
}

export function PortsView() {
  const { ports, hubs, openDialog, setView } = useStore();
  const hubName = (id: string | null) => hubs.find((h) => h.spec.id === id)?.spec.name;

  const start = (kind: "split" | "bridge", p: PortInfo) => {
    setView("topology");
    openDialog({ type: "hub", preset: { kind, master: endpointFor(p) } });
  };

  return (
    <Page title="Serial ports" subtitle="Every COM port Windows reports, including SerialLoom's virtual ports.">
      <div className="panel overflow-hidden">
        <table className="w-full text-sm">
          <thead className="bg-ink-800/60 text-left text-xs tracking-wide text-ink-300 uppercase">
            <tr>
              <th className="px-4 py-2.5 font-medium">Port</th>
              <th className="px-4 py-2.5 font-medium">Type</th>
              <th className="px-4 py-2.5 font-medium">Description</th>
              <th className="px-4 py-2.5 font-medium">Status</th>
              <th className="px-4 py-2.5" />
            </tr>
          </thead>
          <tbody className="divide-y divide-ink-700/60">
            {ports.length === 0 && (
              <tr>
                <td colSpan={5} className="px-4 py-10 text-center text-ink-400">
                  No serial ports found.
                </td>
              </tr>
            )}
            {ports.map((p) => (
              <tr key={p.name} className="hover:bg-ink-800/40">
                <td className="px-4 py-2.5 font-mono font-semibold text-ink-0">{p.name}</td>
                <td className="px-4 py-2.5">
                  <KindBadge kind={p.kind} />
                </td>
                <td className="px-4 py-2.5 text-ink-200">
                  {p.description ?? "—"}
                  {p.vid != null && (
                    <span className="ml-2 font-mono text-[11px] text-ink-400">
                      {p.vid.toString(16).padStart(4, "0")}:{p.pid?.toString(16).padStart(4, "0")}
                    </span>
                  )}
                </td>
                <td className="px-4 py-2.5 text-xs">
                  {p.inUseBy ? (
                    <span className="text-amber-300">In use · {hubName(p.inUseBy)}</span>
                  ) : !p.present ? (
                    <span className="text-ink-400">Not started</span>
                  ) : (
                    <span className="text-emerald-300">Available</span>
                  )}
                </td>
                <td className="px-4 py-2 text-right whitespace-nowrap">
                  {!p.inUseBy && (
                    <>
                      <button className="btn-ghost px-2 py-1 text-xs" onClick={() => start("split", p)}>
                        <Split size={13} /> Split
                      </button>
                      <button className="btn-ghost px-2 py-1 text-xs" onClick={() => start("bridge", p)}>
                        <ArrowLeftRight size={13} /> Bridge
                      </button>
                    </>
                  )}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </Page>
  );
}

export function VirtualPortsView() {
  const { virtualPorts, hubs, run, openDialog, driver } = useStore();
  const owner = (name: string) =>
    hubs.find((h) => [h.spec.master, ...h.spec.members].some((e) => e.spec.kind === "virtual" && e.spec.port === name))
      ?.spec.name;
  const ready = driver?.available ?? false;

  return (
    <Page
      title="Virtual ports"
      subtitle={`COM ports created by ${driver?.name ?? "the SerialLoom driver"}. Programs see them as ordinary serial ports.`}
      actions={
        <div className="flex gap-2">
          <button className="btn-ghost" onClick={() => openDialog({ type: "diagnostics" })} title="Collect a troubleshooting report">
            <Stethoscope size={15} /> Diagnostics
          </button>
          <button className="btn-outline" onClick={() => openDialog({ type: "pair" })} disabled={!ready}>
            <Cable size={15} /> New pair
          </button>
          <button className="btn-primary" onClick={() => openDialog({ type: "virtualPort" })} disabled={!ready}>
            <Plus size={15} /> New port
          </button>
        </div>
      }
    >
      {!ready && <DriverMissing />}
      {ready && virtualPorts.length === 0 ? (
        <div className="panel grid place-items-center px-6 py-14 text-center">
          <Cable size={28} className="text-ink-500" />
          <p className="mt-3 text-sm text-ink-300">No virtual ports yet.</p>
        </div>
      ) : (
        <div className="grid gap-3 sm:grid-cols-2 lg:grid-cols-3">
          {virtualPorts.map((p) => {
            const used = owner(p.name);
            return (
              <div key={p.id} className="panel p-4">
                <div className="flex items-center gap-2">
                  <span className="font-mono text-lg font-semibold text-ink-0">{p.name}</span>
                  <span
                    className={
                      p.state === "ready"
                        ? "chip bg-emerald-500/15 text-emerald-300"
                        : p.state === "problem"
                          ? "chip bg-rose-500/15 text-rose-300"
                          : "chip bg-ink-700 text-ink-300"
                    }
                  >
                    {p.state}
                  </span>
                  {!used && (
                    <button
                      className="ml-auto text-ink-500 hover:text-rose-300"
                      title="Remove port"
                      onClick={() =>
                        run(
                          "Removing port (Windows asks for administrator permission)",
                          () => api.removeVirtualPort(p.name),
                          `${p.name} removed`,
                        )
                      }
                    >
                      <Trash size={15} />
                    </button>
                  )}
                </div>
                <div className="mt-1 truncate font-mono text-[11px] text-ink-400">{p.id}</div>
                {p.problem && <p className="mt-2 text-xs text-rose-300">{p.problem}</p>}
                {p.driverReport && p.state !== "ready" && (
                  <p className="selectable mt-1.5 font-mono text-[10.5px] break-words text-ink-400">{p.driverReport}</p>
                )}
                <p className="mt-2 text-xs text-ink-300">{used ? `Used by "${used}"` : "Not connected to anything"}</p>
                <ChangeNumber port={p.name} />
              </div>
            );
          })}
        </div>
      )}
    </Page>
  );
}

export function DriverMissing() {
  const { driver, run } = useStore();
  if (!driver || driver.available) return null;
  return (
    <div className="panel mb-4 flex items-start gap-3 border-amber-500/40 p-4">
      <CircleAlert size={18} className="mt-0.5 shrink-0 text-amber-300" />
      <div className="flex-1 text-sm">
        <p className="font-medium text-amber-100">The SerialLoom driver is not installed</p>
        <p className="mt-1 text-ink-300">{driver.message}</p>
      </div>
      {driver.installable && (
        <button
          className="btn-primary shrink-0"
          onClick={() =>
            run("Installing driver (Windows asks for administrator permission)", () => api.installDriver(), "Driver installed")
          }
        >
          <Download size={15} /> Install driver
        </button>
      )}
    </div>
  );
}

/** Gives a virtual port another COM number, like Device Manager's Advanced port settings. */
function ChangeNumber({ port }: { port: string }) {
  const { run } = useStore();
  const [editing, setEditing] = useState(false);
  const [value, setValue] = useState(port);

  if (!editing) {
    return (
      <button
        className="btn-ghost mt-2 -ml-2 px-2 py-1 text-xs"
        onClick={() => {
          setValue(port);
          setEditing(true);
        }}
      >
        <Hash size={13} /> Change number
      </button>
    );
  }

  const save = async () => {
    const target = value.trim().toUpperCase();
    if (!target || target === port) return setEditing(false);
    const ok = await run(
      "Changing COM number (Windows asks for administrator permission)",
      () => api.renameVirtualPort(port, target),
      `${port} is now ${target}`,
    );
    if (ok) setEditing(false);
  };

  return (
    <div className="mt-2 flex gap-1.5">
      <input
        className="input py-1 font-mono text-xs"
        value={value}
        autoFocus
        spellCheck={false}
        onChange={(e) => setValue(e.target.value.toUpperCase())}
        onKeyDown={(e) => {
          if (e.key === "Enter") save();
          if (e.key === "Escape") setEditing(false);
        }}
        placeholder="COM20"
      />
      <button className="btn-primary px-2.5 py-1 text-xs" onClick={save}>
        Save
      </button>
      <button className="btn-ghost px-2 py-1 text-xs" onClick={() => setEditing(false)}>
        Cancel
      </button>
    </div>
  );
}
