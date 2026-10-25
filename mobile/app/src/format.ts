import type { ChannelRow, HubEventBody, Kind } from "./hub/types";

/** A reading in human units. Temperatures arrive in milli-degrees C. */
export function formatValue(kind: Kind, value: number | null): string {
  if (value == null) return "–";
  switch (kind) {
    case "temperature":
      return `${(value / 1000).toFixed(1)} °C`;
    case "analog_in":
      return `${value} mV`;
    case "digital_in":
      return value ? "HIGH" : "LOW";
    case "digital_out":
      return value ? "ON" : "OFF";
    default:
      return String(value);
  }
}

/** What the recipe editor's number field means for each kind. */
export function unitFor(kind: Kind): { label: string; scale: number } {
  switch (kind) {
    case "temperature":
      return { label: "°C", scale: 1000 };
    case "analog_in":
      return { label: "mV", scale: 1 };
    default:
      return { label: "0/1", scale: 1 };
  }
}

export function ago(ts: number | null, now = Date.now() / 1000): string {
  if (ts == null) return "never";
  const s = Math.max(0, Math.round(now - ts));
  if (s < 60) return `${s} s ago`;
  if (s < 3600) return `${Math.round(s / 60)} min ago`;
  if (s < 86400) return `${Math.round(s / 3600)} h ago`;
  return `${Math.round(s / 86400)} d ago`;
}

/** Nodes report every 10 s; a minute of silence means it's offline. */
export function isOnline(lastSeen: number | null, hubTime: number): boolean {
  return lastSeen != null && hubTime - lastSeen < 60;
}

const ALERT_CODES: Record<number, string> = {
  1: "Sensor fault",
  2: "Threshold crossed",
  3: "Node restarted",
};

export function describeEvent(e: HubEventBody, channelName: (node: number, ch: number) => string): string {
  switch (e.type) {
    case "alert":
      return `${ALERT_CODES[e.code] ?? `Alert ${e.code}`}: ${channelName(e.node_id, e.channel)} (${e.value})`;
    case "recipe_alert":
      return `Recipe "${e.name}": ${e.text}`;
    case "command_dropped":
      return `Command dropped (queue full): ${channelName(e.node_id, e.channel)} → ${e.value}`;
  }
}

export function channelLabel(c: Pick<ChannelRow, "name">, nodeName: string): string {
  return `${nodeName} · ${c.name}`;
}
