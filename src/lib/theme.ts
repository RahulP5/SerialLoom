import { create } from "zustand";
import { isTauri } from "@tauri-apps/api/core";

export type ThemePref = "system" | "light" | "dark";

const KEY = "serialloom.theme";
const darkQuery = window.matchMedia("(prefers-color-scheme: dark)");

function load(): ThemePref {
  try {
    const v = localStorage.getItem(KEY);
    if (v === "light" || v === "dark" || v === "system") return v;
  } catch {
    // Storage unavailable: use the default.
  }
  return "system";
}

function resolve(pref: ThemePref): "light" | "dark" {
  if (pref === "system") return darkQuery.matches ? "dark" : "light";
  return pref;
}

function paint(pref: ThemePref) {
  const theme = resolve(pref);
  const root = document.documentElement;
  root.dataset.theme = theme;
  root.style.colorScheme = theme;
  useTheme.setState({ resolved: theme });
}

/** Matches the window's title bar to the theme (Windows draws it, not the page). */
async function paintWindow(pref: ThemePref) {
  if (!isTauri()) return;
  try {
    const { getCurrentWindow } = await import("@tauri-apps/api/window");
    await getCurrentWindow().setTheme(pref === "system" ? null : pref);
  } catch {
    // Older WebView or missing permission: the page theme still applies.
  }
}

interface ThemeState {
  pref: ThemePref;
  /** The theme in effect: `pref`, or what Windows uses when `pref` is "system". */
  resolved: "light" | "dark";
  setPref(p: ThemePref): void;
}

const initial = load();

export const useTheme = create<ThemeState>((set) => ({
  pref: initial,
  resolved: resolve(initial),
  setPref(pref) {
    try {
      localStorage.setItem(KEY, pref);
    } catch {
      // Not remembered, but still applied for this session.
    }
    paint(pref);
    void paintWindow(pref);
    set({ pref });
  },
}));

/** Applies the saved theme before the first render, and follows Windows when set to "system". */
export function initTheme() {
  const pref = useTheme.getState().pref;
  paint(pref);
  void paintWindow(pref);
  darkQuery.addEventListener("change", () => {
    const current = useTheme.getState().pref;
    if (current === "system") paint(current);
  });
}
