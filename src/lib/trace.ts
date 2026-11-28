import { invoke } from "@tauri-apps/api/core";
import type { TraceLink } from "../components/graph";

export type FileFormat = "text" | "hex" | "both";
export type Direction = "rx" | "tx" | "both";
export type TraceOptions = TraceLink & { fileMode: boolean };

export interface TraceRecord {
  sequence: number;
  timestamp: string;
  elapsedMs: number;
  direction: Exclude<Direction, "both">;
  bytes: number[];
}

export interface TraceStatus {
  running: boolean;
  pending: boolean;
  isFile: boolean;
  remainingSeconds: number;
  dropped: number;
  reason?: "stopped" | "expired" | "space" | "size" | "write";
  detail?: string;
  records: TraceRecord[];
}

type TraceAction = { type: "status" | "summary" | "stop" } |
  { type: "start"; direction: Direction; seconds: number; path: string | null; format: FileFormat };

export const traceChanged = "serialloom://trace-changed";

export function capture(link: Pick<TraceLink, "id" | "index">, action: TraceAction) {
  return invoke<TraceStatus>("capture_link", { id: link.id, index: link.index, action });
}

export function openTrace(link: TraceLink, fileMode: boolean) {
  return invoke("open_trace_window", { ...link, fileMode });
}

export function traceTime(seconds: number) {
  return `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, "0")}`;
}

