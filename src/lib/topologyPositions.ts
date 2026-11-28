const POSITIONS_KEY = "serialloom.positions.v1";

const MIRROR_KEY = "serialloom.mirrored-hubs.v1";

type XY = { x: number; y: number };

export function loadMirroredHubs(): Set<string> {
  try {
    return new Set(JSON.parse(localStorage.getItem(MIRROR_KEY) ?? "[]"));
  } catch {
    return new Set();
  }
}

export function saveMirroredHubs(hubs: ReadonlySet<string>) {
  try {
    localStorage.setItem(MIRROR_KEY, JSON.stringify([...hubs]));
  } catch {
    /* storage unavailable */
  }
}

export function loadPositions(): Record<string, XY> {
  try {
    return JSON.parse(localStorage.getItem(POSITIONS_KEY) ?? "{}");
  } catch {
    return {};
  }
}

export function savePositions(p: Record<string, XY>) {
  try {
    localStorage.setItem(POSITIONS_KEY, JSON.stringify(p));
  } catch {
    /* storage unavailable: positions just won't persist */
  }
}
