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
  /** Goes in hub.toml as pc_key_id (SHA-256 of this PC's hybrid key). */
  pc_key_id: string;
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
  kind: "hub" | "node" | "phone";
  public_key: string;
  fingerprint: string;
  transport: "wifi" | "lora" | null;
  seen_at: number;
}

export interface Phone {
  id: number;
  fingerprint: string;
  name: string;
  paired_at: number;
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


// Recipes: stored and run on the hub (hub-node/src/recipes.rs). The phone app
// edits the same ones (mobile/app/src/hub/types.ts).
export type Op = "gt" | "lt" | "ge" | "le" | "eq" | "ne";

export type Cond =
  | { type: "compare"; node: number; channel: number; op: Op; value: number }
  | { type: "and"; all: Cond[] }
  | { type: "or"; any: Cond[] }
  | { type: "not"; cond: Cond };

export type Action =
  | { type: "set"; node: number; channel: number; value: number }
  | { type: "alert"; text: string };

export interface Recipe {
  id: number;
  name: string;
  enabled: boolean;
  when: Cond;
  then: Action[];
  otherwise: Action[];
}

export interface RecipeSummary {
  id: number;
  name: string;
  enabled: boolean;
}

export type RecipeInput = Omit<Recipe, "id">;
