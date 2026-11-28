import { invoke } from "@tauri-apps/api/core";
import type { TraceLink } from "../components/graph";

export type FileFormat = "text" | "hex" | "both";
export type Direction = "rx" | "tx" | "both";
export type TraceOptions = TraceLink & { fileMode: boolean; usbProbe?: boolean };

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
  signals?: {
    levels: (boolean | null)[];
    changes: [{ secs_since_epoch: number; nanos_since_epoch: number }, number, boolean][];
  };
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


export function captureUsb(id: string, action: TraceAction) {
  return invoke<TraceStatus>("capture_usb_probe", { id, action });
}

export function openUsbProbe(port: string) {
  return invoke("open_trace_window", {
    id: port, index: 0, source: port, target: "USBPcap", forwardRx: true,
    fileMode: false, usbProbe: true,
  });
}
