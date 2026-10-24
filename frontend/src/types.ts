// Shapes returned by pc-daemon (see pc-daemon/src/api/mod.rs).

export type ChannelKind = "digital_in" | "digital_out" | "analog_in" | "temperature" | "unknown";

export interface Channel {
  channel: number;
  kind: ChannelKind;
  name: string;
  last_value: number | null;
  last_ts: number | null;
}

export interface NodeInfo {
  id: number;
  name: string;
  fingerprint: string;
  paired_at: number;
  last_seen: number | null;
  transport: "wifi" | "lora" | null;
  sessions: number;
  channels: Channel[];
}

export interface Status {
  pc_fingerprint: string;
  time: number;
  hub: {
    paired: boolean;
    fingerprint: string | null;
    connected: boolean;
    last_seen: number | null;
    backlog: number;
    sessions: number;
  };
}

export interface PendingPair {
  kind: "hub" | "node";
  public_key: string;
  fingerprint: string;
  transport: "wifi" | "lora" | null;
  seen_at: number;
}

export interface Reading {
  ts: number;
  value: number;
}

export interface Alert {
  id: number;
  node_id: number | null;
  node_name: string | null;
  channel: number | null;
  code: number;
  value: number | null;
  message: string;
  ts: number;
  acked: boolean;
}

export interface Rule {
  id: number;
  name: string;
  enabled: boolean;
  node_id: number;
  channel: number;
  op: "gt" | "lt";
  threshold: number;
  target_node: number;
  target_channel: number;
  on_value: number;
  off_value: number | null;
  active: boolean;
}

export type RuleInput = Omit<Rule, "id" | "active">;
