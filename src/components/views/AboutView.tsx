import { ExternalLink } from "lucide-react";
import { useStore } from "../../store";
import { openExternal } from "../../lib/api";
import { COPYRIGHT_YEAR, DEVELOPER } from "../../lib/project";

export function AboutView() {
  const info = useStore((s) => s.info);
  const repo = info?.repository ?? "";

  return (
    <div className="flex h-full justify-center overflow-y-auto px-8 py-7">
      <div className="my-auto w-full max-w-lg text-center">
        <img src="/logo.svg" alt="SerialLoom logo" className="mx-auto h-24 w-24 drop-shadow-[0_10px_30px_rgba(99,102,241,0.35)]" />
        <h1 className="mt-4 text-2xl font-semibold text-white">SerialLoom</h1>
        <p className="mt-1 text-sm text-ink-300">Version {info?.version ?? "…"}</p>
        <p className="mt-4 text-sm leading-relaxed text-ink-200">
          Weave serial ports together: create virtual COM port pairs, split one port between several applications, join
          ports, and bridge serial devices to the network.
        </p>
        <div className="mt-5 flex justify-center gap-2">
          {repo && (
            <button className="btn-outline" onClick={() => openExternal(repo)}>
              <GitHubMark /> Source code
            </button>
          )}
          {repo && (
            <button className="btn-outline" onClick={() => openExternal(`${repo}/issues`)}>
              <ExternalLink size={14} /> Report an issue
            </button>
          )}
        </div>
        <section className="panel mt-8 p-4 text-left">
          <h2 className="label">Developers</h2>
          <div className="mt-2 flex items-center gap-3">
            <div className="grid h-10 w-10 shrink-0 place-items-center rounded-full bg-linear-to-br from-loom-500 to-cyan-500 text-sm font-semibold text-white">
              {DEVELOPER.name
                .split(" ")
                .map((w) => w[0])
                .join("")}
            </div>
            <div className="min-w-0 flex-1">
              <div className="font-semibold text-white">{DEVELOPER.name}</div>
              <div className="text-xs text-ink-400">Creator and maintainer</div>
            </div>
            <button className="btn-outline px-2.5 py-1 text-xs" onClick={() => openExternal(DEVELOPER.github)}>
              <GitHubMark /> GitHub profile
            </button>
          </div>
        </section>

        <p className="mt-6 text-xs text-ink-300">
          © {COPYRIGHT_YEAR}{" "}
          <button className="text-cyan-300 hover:underline" onClick={() => openExternal(DEVELOPER.github)}>
            {DEVELOPER.name}
          </button>
          . Free and open-source software under the MIT License.
        </p>
        <p className="mt-3 text-xs leading-relaxed text-ink-400">
          Virtual COM ports use Windows' built-in USB serial driver through the bundled, Microsoft-signed{" "}
          <button className="text-cyan-300 hover:underline" onClick={() => openExternal("https://github.com/vadimgrn/usbip-win2")}>
            usbip-win2
          </button>{" "}
          virtual USB controller (BSD-2-Clause). See THIRD_PARTY_NOTICES.md.
        </p>
      </div>
    </div>
  );
}

function GitHubMark() {
  return (
    <svg viewBox="0 0 16 16" width="14" height="14" fill="currentColor" aria-hidden="true">
      <path d="M8 0C3.58 0 0 3.58 0 8c0 3.54 2.29 6.53 5.47 7.59.4.07.55-.17.55-.38 0-.19-.01-.82-.01-1.49-2.01.37-2.53-.49-2.69-.94-.09-.23-.48-.94-.82-1.13-.28-.15-.68-.52-.01-.53.63-.01 1.08.58 1.23.82.72 1.21 1.87.87 2.33.66.07-.52.28-.87.51-1.07-1.78-.2-3.64-.89-3.64-3.95 0-.87.31-1.59.82-2.15-.08-.2-.36-1.02.08-2.12 0 0 .67-.21 2.2.82.64-.18 1.32-.27 2-.27.68 0 1.36.09 2 .27 1.53-1.04 2.2-.82 2.2-.82.44 1.1.16 1.92.08 2.12.51.56.82 1.27.82 2.15 0 3.07-1.87 3.75-3.65 3.95.29.25.54.73.54 1.48 0 1.07-.01 1.93-.01 2.2 0 .21.15.46.55.38A8.013 8.013 0 0016 8c0-4.42-3.58-8-8-8z" />
    </svg>
  );
}
