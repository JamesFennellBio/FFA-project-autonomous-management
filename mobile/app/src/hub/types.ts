// Shapes of the hub's replies (hub-node/src/hub/app.rs). Values are integers in
// "milli" units like everywhere else: milli-degrees C, millivolts, 0/1.

export type Kind = "digital_in" | "digital_out" | "analog_in" | "temperature" | "unknown";

export interface Info {
  hub_fingerprint: string;
  phone_id: number;
  nodes: number;
  recipes: number;
  max_recipes: number;
  time: number;
}

export interface NodeRow {
  id: number;
  name: string;
  last_seen: number | null;
}

export interface ChannelRow {
  node: number;
  channel: number;
  kind: Kind;
  output: boolean;
  name: string;
  value: number | null;
  ts: number | null;
}

export type HubEventBody =
  | { type: "alert"; node_id: number; channel: number; code: number; value: number; ts: number }
  | { type: "recipe_alert"; recipe_id: number; name: string; text: string; ts: number }
  | { type: "command_dropped"; node_id: number; channel: number; value: number; ts: number };

export interface HubEvent {
  id: number;
  event: HubEventBody;
}

export interface PairRequestRow {
  public_key: string;
  fingerprint: string;
  transport: "wifi" | "lora";
  seen_at: number;
}

export interface RecipeSummary {
  id: number;
  name: string;
  enabled: boolean;
}

// Recipes (hub-node/src/recipes.rs).
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
