import { useEffect, useState } from "react";
import { Cable, Copy, Plus, Stethoscope } from "lucide-react";
import { Modal } from "./Modal";
import { EndpointEditor, defaultFor } from "./EndpointEditor";
import { useStore } from "../../store";
import { api, errorText } from "../../lib/api";
import type { EndpointRequest } from "../../lib/types";

const ADMIN_NOTE = "Creating ports (Windows asks for administrator permission)";

/** Real (non-virtual) port currently using this COM name, if any. */
function useTakenBy(name: string) {
  const ports = useStore((s) => s.ports);
  const n = name.trim().toUpperCase();
  if (!n || n === "COM#") return undefined;
  return ports.find((p) => p.name.toUpperCase() === n && p.kind !== "virtual");
}

function NameInput({ label, value, onChange }: { label: string; value: string; onChange: (v: string) => void }) {
  const taken = useTakenBy(value);
  return (
    <div className="flex-1">
      <label className="label">{label}</label>
      <input
        className={taken ? "input border-rose-500 font-mono" : "input font-mono"}
        value={value}
        onChange={(e) => onChange(e.target.value.toUpperCase())}
        spellCheck={false}
      />
      {taken && (
        <p className="mt-1 text-[11px] text-rose-300">
          {taken.name} belongs to real hardware{taken.description ? ` (${taken.description})` : ""}. Pick another number.
        </p>
      )}
    </div>
  );
}

function Icon() {
  return (
    <div className="grid h-9 w-9 place-items-center rounded-xl bg-violet-500/15 text-violet-300">
      <Cable size={18} />
    </div>
  );
}

/** Two new virtual ports bridged together: a virtual null-modem cable. */
export function PairDialog({ onClose }: { onClose: () => void }) {
  const { run, busy, select } = useStore();
  const [a, setA] = useState("COM#");
  const [b, setB] = useState("COM#");

  const submit = async () => {
    const hub = await run(ADMIN_NOTE, () =>
      api.createHub({
        name: "Virtual null-modem",
        kind: "bridge",
        master: { kind: "newVirtual", name: a.trim() },
        members: [{ kind: "newVirtual", name: b.trim() }],
        autostart: true,
        start: true,
      }),
    );
    if (hub) {
      const [pa, pb] = [hub.spec.master, hub.spec.members[0]].map((e) => ("port" in e.spec ? e.spec.port : "?"));
      await api.updateHub(hub.spec.id, `${pa} ⇄ ${pb}`);
      useStore.getState().toast("success", `Created ${pa} ⇄ ${pb}`);
      const wanted = [a.trim(), b.trim()];
      const got = [pa, pb];
      const differs = wanted.filter((w, i) => w !== "COM#" && w !== got[i]);
      if (differs.length) {
        useStore
          .getState()
          .toast(
            "info",
            `Windows kept ${got.join(" / ")} instead of ${wanted.join(" / ")}. Use "Change number" under Virtual ports to try again.`,
          );
      }
      await useStore.getState().refresh();
      select({ type: "hub", id: hub.spec.id });
      onClose();
    }
  };

  return (
    <Modal
      title="New virtual port pair"
      subtitle="Two COM ports joined like a null-modem cable. What one program writes to the first, another reads from the second."
      onClose={onClose}
      icon={<Icon />}
      footer={
        <>
          <button className="btn-ghost" onClick={onClose}>
            Cancel
          </button>
          <button className="btn-primary" onClick={submit} disabled={!!busy}>
            {busy ? "Working…" : "Create pair"}
          </button>
        </>
      }
    >
      <div className="flex items-end gap-3">
        <NameInput label="Port A" value={a} onChange={setA} />
        <div className="pb-2 text-ink-400">⇄</div>
        <NameInput label="Port B" value={b} onChange={setB} />
      </div>
      <p className="mt-3 text-xs text-ink-400">
        Type a name such as <code className="font-mono text-ink-200">COM20</code>, or use{" "}
        <code className="font-mono text-ink-200">COM#</code> for the next free number. SerialLoom carries the data
        between the two ports while it runs.
      </p>
    </Modal>
  );
}

/** One standalone virtual port, to wire up later. */
export function VirtualPortDialog({ onClose }: { onClose: () => void }) {
  const { run, busy } = useStore();
  const [name, setName] = useState("COM#");

  const submit = async () => {
    const created = await run(ADMIN_NOTE, () => api.createVirtualPorts([name.trim()]));
    if (created) {
      useStore.getState().toast("success", `Created ${created.map((p) => p.name).join(", ")}`);
      onClose();
    }
  };

  return (
    <Modal
      title="New virtual COM port"
      subtitle="The port appears in Windows right away. Connect it to something on the canvas to make it carry data."
      onClose={onClose}
      icon={<Icon />}
      footer={
        <>
          <button className="btn-ghost" onClick={onClose}>
            Cancel
          </button>
          <button className="btn-primary" onClick={submit} disabled={!!busy}>
            {busy ? "Working…" : "Create port"}
          </button>
        </>
      }
    >
      <NameInput label="Port name" value={name} onChange={setName} />
    </Modal>
  );
}

export function AddMemberDialog({
  hubId,
  preset,
  onClose,
}: {
  hubId: string;
  preset?: EndpointRequest;
  onClose: () => void;
}) {
  const { run, busy, hubs } = useStore();
  const hub = hubs.find((h) => h.spec.id === hubId);
  const [value, setValue] = useState<EndpointRequest>(preset ?? defaultFor("newVirtual"));
  if (!hub) return null;

  const submit = async () => {
    const label = value.kind === "newVirtual" ? ADMIN_NOTE : "Adding endpoint";
    const ok = await run(label, () => api.addMember(hubId, value), `Added to "${hub.spec.name}"`);
    if (ok) onClose();
  };

  return (
    <Modal
      title={`Add to "${hub.spec.name}"`}
      subtitle={hub.status.running ? "The hub restarts briefly to pick up the new endpoint." : undefined}
      onClose={onClose}
      width={660}
      icon={
        <div className="grid h-9 w-9 place-items-center rounded-xl bg-cyan-500/15 text-cyan-300">
          <Plus size={18} />
        </div>
      }
      footer={
        <>
          <button className="btn-ghost" onClick={onClose}>
            Cancel
          </button>
          <button className="btn-primary" onClick={submit} disabled={!!busy}>
            {busy ? "Working…" : "Add endpoint"}
          </button>
        </>
      }
    >
      <EndpointEditor value={value} onChange={setValue} />
    </Modal>
  );
}

/** Read-only troubleshooting report the user can paste into an issue. */
export function DiagnosticsDialog({ onClose }: { onClose: () => void }) {
  const [text, setText] = useState("Collecting…");
  const [copied, setCopied] = useState(false);

  useEffect(() => {
    api.diagnostics().then(setText, (e) => setText(`Could not collect diagnostics: ${errorText(e)}`));
  }, []);

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(text);
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    } catch {
      /* clipboard unavailable: the text box can still be selected and copied */
    }
  };

  return (
    <Modal
      title="Diagnostics"
      subtitle="Versions, port states and Windows logs. Nothing is changed or sent anywhere."
      onClose={onClose}
      width={860}
      icon={
        <div className="grid h-9 w-9 place-items-center rounded-xl bg-cyan-500/15 text-cyan-300">
          <Stethoscope size={18} />
        </div>
      }
      footer={
        <>
          <button className="btn-ghost" onClick={onClose}>
            Close
          </button>
          <button className="btn-primary" onClick={copy}>
            <Copy size={15} /> {copied ? "Copied" : "Copy report"}
          </button>
        </>
      }
    >
      <textarea
        readOnly
        value={text}
        onFocus={(e) => e.currentTarget.select()}
        className="input h-[420px] resize-none font-mono text-[11px] leading-relaxed"
        spellCheck={false}
      />
    </Modal>
  );
}
