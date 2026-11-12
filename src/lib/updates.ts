import { useEffect } from "react";
import { create } from "zustand";
import { LATEST_RELEASE_API } from "./project";

/**
 * "New version available" notice. SerialLoom asks GitHub for its latest
 * release when it starts and once a day; the request carries no personal
 * data. Both choices are remembered like the theme, so changing them never
 * touches the port driver.
 */

const ENABLED_KEY = "serialloom.updates.check";
const DISMISSED_KEY = "serialloom.updates.dismissed";
const CHECK_EVERY_MS = 24 * 60 * 60 * 1000;
const REQUEST_TIMEOUT_MS = 15_000;

export interface UpdateInfo {
  version: string;
  /** Release notes on GitHub. */
  notesUrl: string;
}

function read(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

function write(key: string, value: string) {
  try {
    localStorage.setItem(key, value);
  } catch {
    // Not remembered, but still applied for this session.
  }
}

/** "v0.1.10" -> [0, 1, 10]; null for anything but a plain release version. */
function parseVersion(v: string): number[] | null {
  const m = /^v?(\d+)\.(\d+)\.(\d+)$/.exec(v.trim());
  return m ? m.slice(1).map(Number) : null;
}

export function isNewer(candidate: string, current: string): boolean {
  const a = parseVersion(candidate);
  const b = parseVersion(current);
  if (!a || !b) return false;
  for (let i = 0; i < 3; i++) {
    if (a[i] !== b[i]) return a[i] > b[i];
  }
  return false;
}

/** The newest release, or null when GitHub cannot be reached (offline, blocked, rate-limited). */
async function latestRelease(): Promise<UpdateInfo | null> {
  const abort = new AbortController();
  const timer = setTimeout(() => abort.abort(), REQUEST_TIMEOUT_MS);
  try {
    const res = await fetch(LATEST_RELEASE_API, {
      headers: { Accept: "application/vnd.github+json" },
      signal: abort.signal,
    });
    if (!res.ok) return null;
    const release = await res.json();
    if (typeof release.tag_name !== "string" || typeof release.html_url !== "string") return null;
    return { version: release.tag_name.replace(/^v/, ""), notesUrl: release.html_url };
  } catch {
    return null;
  } finally {
    clearTimeout(timer);
  }
}

interface UpdateState {
  enabled: boolean;
  /** A newer release the user has not dismissed. */
  available: UpdateInfo | null;
  setEnabled(on: boolean): void;
  /** Hides the notice until an even newer version comes out. */
  dismiss(): void;
  check(current: string): Promise<void>;
}

export const useUpdates = create<UpdateState>((set, get) => ({
  enabled: read(ENABLED_KEY) !== "off",
  available: null,
  setEnabled(on) {
    write(ENABLED_KEY, on ? "on" : "off");
    set(on ? { enabled: true } : { enabled: false, available: null });
  },
  dismiss() {
    const shown = get().available;
    if (shown) write(DISMISSED_KEY, shown.version);
    set({ available: null });
  },
  async check(current) {
    const latest = await latestRelease();
    // Keep what we know when GitHub is unreachable or checks were switched off meanwhile.
    if (!latest || !get().enabled) return;
    const show = isNewer(latest.version, current) && latest.version !== read(DISMISSED_KEY);
    set({ available: show ? latest : null });
  },
}));

/** Checks now and then once a day while SerialLoom runs, as long as checks are on. */
export function useUpdateChecks(current: string | undefined) {
  const enabled = useUpdates((s) => s.enabled);
  const check = useUpdates((s) => s.check);
  useEffect(() => {
    if (!current || !enabled) return;
    void check(current);
    const timer = setInterval(() => void check(current), CHECK_EVERY_MS);
    return () => clearInterval(timer);
  }, [current, enabled, check]);
}
