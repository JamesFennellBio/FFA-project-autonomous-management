import type { ChannelKind } from "./types";

// Readings are integers in "milli" units (see crypto-sharecrate/src/packet.rs).

export function formatValue(kind: ChannelKind, v: number | null): string {
  if (v === null || v === undefined) return "—";
  switch (kind) {
    case "temperature":
      return `${(v / 1000).toFixed(1)} °C`;
    case "analog_in":
      return `${(v / 1000).toFixed(2)} V`;
    case "digital_in":
    case "digital_out":
      return v ? "ON" : "OFF";
    default:
      return String(v);
  }
}

/** Number for charts, in display units. */
export function chartValue(kind: ChannelKind, v: number): number {
  return kind === "temperature" || kind === "analog_in" ? v / 1000 : v;
}

export function unitOf(kind: ChannelKind): string {
  return kind === "temperature" ? "°C" : kind === "analog_in" ? "V" : "";
}

/** User input (°C or V) back to milli units, for rule thresholds. */
export function toMilli(kind: ChannelKind, display: number): number {
  return kind === "temperature" || kind === "analog_in" ? Math.round(display * 1000) : Math.round(display);
}

export function ago(ts: number | null, now = Date.now() / 1000): string {
  if (!ts) return "never";
  const s = Math.max(0, Math.round(now - ts));
  if (s < 60) return `${s}s ago`;
  if (s < 3600) return `${Math.floor(s / 60)}m ago`;
  if (s < 86400) return `${Math.floor(s / 3600)}h ago`;
  return `${Math.floor(s / 86400)}d ago`;
}

export function kindLabel(kind: ChannelKind): string {
  return {
    temperature: "Temperature",
    analog_in: "Analog in",
    digital_in: "Digital in",
    digital_out: "Output",
    unknown: "Unknown",
  }[kind];
}

/** Online = heard from in the last 2 minutes. */
export function isOnline(lastSeen: number | null): boolean {
  return !!lastSeen && Date.now() / 1000 - lastSeen < 120;
}
