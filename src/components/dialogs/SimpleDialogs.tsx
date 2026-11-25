import { useEffect, useState } from "react";
import { Cable, Copy, Plus, Stethoscope } from "lucide-react";
import { Modal } from "./Modal";
import { EndpointEditor, defaultFor } from "./EndpointEditor";
import { useStore } from "../../store";
import { api, errorText } from "../../lib/api";
import type { EndpointRequest } from "../../lib/types";
import { t, tr, useT } from "../../i18n";

const adminNote = () => useStore.getState().adminNote(t("dialog.creatingPorts"));

/** Real (non-virtual) port currently using this COM name, if any. */
function useTakenBy(name: string) {
  const ports = useStore((s) => s.ports);
  const n = name.trim().toUpperCase();
  if (!n || n === "COM#") return undefined;
  return ports.find((p) => p.name.toUpperCase() === n && p.kind !== "virtual");
}

function NameInput({ label, value, onChange }: { label: string; value: string; onChange: (v: string) => void }) {
  const taken = useTakenBy(value);
  const t = useT();
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
          {taken.description
            ? t("dialog.takenDescribed", { port: taken.name, description: taken.description })
            : t("dialog.taken", { port: taken.name })}
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
  const t = useT();
  const [a, setA] = useState("COM#");
  const [b, setB] = useState("COM#");

  const submit = async () => {
    const hub = await run(adminNote(), () =>
      api.createHub({
        name: t("pair.defaultName"),
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
      useStore.getState().toast("success", t("pair.created", { a: pa, b: pb }));
      const wanted = [a.trim(), b.trim()];
      const got = [pa, pb];
      const differs = wanted.filter((w, i) => w !== "COM#" && w !== got[i]);
      if (differs.length) {
        useStore
          .getState()
          .toast(
            "info",
            t("pair.kept", {
              got: got.join(" / "),
              wanted: wanted.join(" / "),
              action: t("virtual.changeNumber"),
              page: t("nav.virtual"),
            }),
          );
      }
      await useStore.getState().refresh();
      select({ type: "hub", id: hub.spec.id });
      onClose();
    }
  };

  return (
    <Modal
      title={t("pair.title")}
      subtitle={t("pair.subtitle")}
      onClose={onClose}
      icon={<Icon />}
      footer={
        <>
          <button className="btn-ghost" onClick={onClose}>
            {t("common.cancel")}
          </button>
          <button className="btn-primary" onClick={submit} disabled={!!busy}>
            {busy ? t("common.working") : t("pair.create")}
          </button>
        </>
      }
    >
      <div className="flex items-end gap-3">
        <NameInput label={t("pair.portA")} value={a} onChange={setA} />
        <div className="pb-2 text-ink-400">⇄</div>
        <NameInput label={t("pair.portB")} value={b} onChange={setB} />
      </div>
      <p className="mt-3 text-xs text-ink-400">
        {tr("pair.hint", {
          example: <code className="font-mono text-ink-200">COM20</code>,
          auto: <code className="font-mono text-ink-200">COM#</code>,
        })}
      </p>
    </Modal>
  );
}

/** One standalone virtual port, to wire up later. */
export function VirtualPortDialog({ onClose }: { onClose: () => void }) {
  const { run, busy } = useStore();
  const t = useT();
  const [name, setName] = useState("COM#");

  const submit = async () => {
    const created = await run(adminNote(), () => api.createVirtualPorts([name.trim()]));
    if (created) {
      useStore.getState().toast("success", t("vport.created", { ports: created.map((p) => p.name).join(", ") }));
      onClose();
    }
  };

  return (
    <Modal
      title={t("vport.title")}
      subtitle={t("vport.subtitle")}
      onClose={onClose}
      icon={<Icon />}
      footer={
        <>
          <button className="btn-ghost" onClick={onClose}>
            {t("common.cancel")}
          </button>
          <button className="btn-primary" onClick={submit} disabled={!!busy}>
            {busy ? t("common.working") : t("vport.create")}
          </button>
        </>
      }
    >
      <NameInput label={t("vport.name")} value={name} onChange={setName} />
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
  const t = useT();
  const hub = hubs.find((h) => h.spec.id === hubId);
  const [value, setValue] = useState<EndpointRequest>(preset ?? defaultFor("newVirtual"));
  if (!hub) return null;

  const submit = async () => {
    const label = value.kind === "newVirtual" ? adminNote() : t("addMember.adding");
    const ok = await run(label, () => api.addMember(hubId, value), t("addMember.added", { hub: hub.spec.name }));
    if (ok) onClose();
  };

  return (
    <Modal
      title={t("addMember.title", { hub: hub.spec.name })}
      subtitle={hub.status.running ? t("addMember.restart") : undefined}
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
            {t("common.cancel")}
          </button>
          <button className="btn-primary" onClick={submit} disabled={!!busy}>
            {busy ? t("common.working") : t("inspector.addEndpoint")}
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
  const t = useT();
  const [text, setText] = useState(() => t("diag.collecting"));
  const [copied, setCopied] = useState(false);

  useEffect(() => {
    api.diagnostics().then(setText, (e) => setText(t("diag.failed", { error: errorText(e) })));
  }, [t]);

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
      title={t("diag.title")}
      subtitle={t("diag.subtitle")}
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
            {t("common.close")}
          </button>
          <button className="btn-primary" onClick={copy}>
            <Copy size={15} /> {copied ? t("diag.copied") : t("diag.copy")}
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
