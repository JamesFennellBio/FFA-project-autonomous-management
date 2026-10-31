// Dev only: sample data for running the page in a plain browser (`npx vite`),
// so the look can be worked on without pc-daemon or Electron.
// main.tsx loads this only when window.farmlab is missing in a dev build.
import type { Alert, NodeInfo, PendingPair, Phone, Recipe, Status } from "./types";

const now = () => Math.floor(Date.now() / 1000);
const t0 = now();

const nodes: NodeInfo[] = [
  {
    id: 1, name: "North barn", fingerprint: "7F3A-91C2-0B4E", paired_at: t0 - 86400 * 12, last_seen: t0, transport: "wifi", sessions: 41,
    channels: [
      { channel: 0, kind: "temperature", name: "Barn temp", last_value: 24600, last_ts: t0 },
      { channel: 1, kind: "analog_in", name: "Soil moisture", last_value: 1820, last_ts: t0 },
      { channel: 2, kind: "digital_in", name: "Door", last_value: 0, last_ts: t0 },
      { channel: 3, kind: "digital_out", name: "Fans", last_value: 0, last_ts: t0 },
      { channel: 4, kind: "digital_out", name: "Water valve", last_value: 1, last_ts: t0 },
    ],
  },
  {
    id: 2, name: "Greenhouse", fingerprint: "C410-5D7E-A2F9", paired_at: t0 - 86400 * 5, last_seen: t0 - 20, transport: "lora", sessions: 12,
    channels: [
      { channel: 0, kind: "temperature", name: "Air temp", last_value: 29100, last_ts: t0 },
      { channel: 1, kind: "analog_in", name: "Light", last_value: 2640, last_ts: t0 },
      { channel: 2, kind: "digital_out", name: "Siren", last_value: 0, last_ts: t0 },
    ],
  },
  {
    id: 3, name: "Field pump", fingerprint: "08BD-E61F-33C7", paired_at: t0 - 86400 * 2, last_seen: t0 - 3600, transport: "lora", sessions: 3,
    channels: [{ channel: 0, kind: "temperature", name: "Water temp", last_value: 14300, last_ts: t0 - 3600 }],
  },
];

const alerts: Alert[] = [
  { id: 1, node_id: 2, node_name: "Greenhouse", channel: 0, code: 1, value: 31200, message: "Air temp above 30 °C", ts: t0 - 600, acked: false },
  { id: 2, node_id: 3, node_name: "Field pump", channel: null, code: 2, value: null, message: "Node offline", ts: t0 - 3000, acked: false },
  { id: 3, node_id: 1, node_name: "North barn", channel: 2, code: 3, value: 1, message: "Door opened", ts: t0 - 86400, acked: true },
];

const recipes: Recipe[] = [
  {
    id: 1, name: "Cool the barn", enabled: true,
    when: { type: "compare", node: 1, channel: 0, op: "gt", value: 30000 },
    then: [{ type: "set", node: 1, channel: 3, value: 1 }],
    otherwise: [{ type: "set", node: 1, channel: 3, value: 0 }],
  },
  {
    id: 2, name: "Greenhouse guard", enabled: true,
    when: { type: "and", all: [
      { type: "compare", node: 2, channel: 0, op: "gt", value: 32000 },
      { type: "not", cond: { type: "compare", node: 1, channel: 2, op: "eq", value: 1 } },
    ] },
    then: [{ type: "set", node: 2, channel: 2, value: 1 }, { type: "alert", text: "Greenhouse over 32 °C" }],
    otherwise: [{ type: "set", node: 2, channel: 2, value: 0 }],
  },
  {
    id: 3, name: "Dry soil", enabled: false,
    when: { type: "compare", node: 1, channel: 1, op: "lt", value: 1200 },
    then: [{ type: "set", node: 1, channel: 4, value: 1 }],
    otherwise: [{ type: "set", node: 1, channel: 4, value: 0 }],
  },
];

const pending: PendingPair[] = [
  { kind: "node", public_key: "a1b2c3", fingerprint: "5E2D-77A0-C9B1", transport: "wifi", seen_at: t0 - 15 },
];
const phones: Phone[] = [{ id: 1, fingerprint: "B7C1-0E98-4FA2", name: "James's phone", paired_at: t0 - 86400 * 3 }];
let nextRecipe = 4;

// Small random walk so the dashboard moves like the real thing.
setInterval(() => {
  for (const n of nodes) {
    if (n.id === 3) continue;
    n.last_seen = now();
    for (const c of n.channels) {
      if (c.kind === "temperature" || c.kind === "analog_in") c.last_value = Math.round((c.last_value ?? 0) + (Math.random() - 0.5) * 200);
    }
  }
}, 3000);

function readings(nodeId: number, channel: number, since: number) {
  const ch = nodes.find((n) => n.id === nodeId)?.channels.find((c) => c.channel === channel);
  const base = ch?.last_value ?? 0;
  const span = now() - since;
  const out = [];
  for (let i = 0; i <= 120; i++) {
    const ts = since + (span * i) / 120;
    const day = Math.sin(((ts % 86400) / 86400) * Math.PI * 2);
    const v = ch?.kind === "digital_in" || ch?.kind === "digital_out" ? (day > 0.3 ? 1 : 0) : base + day * base * 0.12 + (Math.random() - 0.5) * base * 0.02;
    out.push({ ts: Math.round(ts), value: Math.round(v) });
  }
  return out;
}

function handle(method: string, path: string, body: unknown): unknown {
  const url = new URL(path, "http://x");
  const p = url.pathname.split("/").filter(Boolean).slice(1); // drop "api"
  const q = url.searchParams;
  const b = (body ?? {}) as Record<string, unknown>;
  const node = (id: string) => nodes.find((n) => n.id === Number(id));

  if (p[0] === "status") {
    const s: Status = {
      pc_fingerprint: "3C9E-1A42-D07B",
      pc_key_id: "9f2c4e1ab37d0c5e8f61a2b49d7c03e5f1a8b26d4c9e07f3a51b8d2c6e94f017",
      time: now(),
      hub: { paired: true, fingerprint: "E5A0-6B13-F8D2", connected: true, last_seen: now(), backlog: 0, sessions: 56 },
    };
    return s;
  }
  if (p[0] === "nodes" && p.length === 1) return nodes;
  if (p[0] === "nodes" && p[2] === "channels") {
    const c = node(p[1])?.channels.find((x) => x.channel === Number(p[3]));
    if (c && p[4] === "command") c.last_value = Number(b.value);
    else if (c && method === "PATCH") c.name = String(b.name);
    return {};
  }
  if (p[0] === "nodes" && method === "PATCH") { const n = node(p[1]); if (n) n.name = String(b.name); return {}; }
  if (p[0] === "nodes" && method === "DELETE") { nodes.splice(nodes.findIndex((n) => n.id === Number(p[1])), 1); return {}; }
  if (p[0] === "readings") return readings(Number(q.get("node_id")), Number(q.get("channel")), Number(q.get("since")));
  if (p[0] === "pairing" && p.length === 2) return pending;
  if (p[0] === "pairing") { pending.splice(pending.findIndex((x) => x.public_key === p[2]), 1); return {}; }
  if (p[0] === "phones" && method === "GET") return phones;
  if (p[0] === "hubs") return { fingerprint: "E5A0-6B13-F8D2-4C71" };
  if (p[0] === "phones" && p[1] === "ticket") {
    const svg = '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 21 21"><rect width="21" height="21" fill="#fff"/><rect x="1" y="1" width="7" height="7"/><rect x="13" y="1" width="7" height="7"/><rect x="1" y="13" width="7" height="7"/></svg>';
    return { phone_id: phones.length + 1, uri: "farmlab://pair?v=2&a=192.168.1.20:7302&k=00&t=00&e=0", svg, expires: now() + 300, hub_fingerprint: "E5A0-6B13-F8D2-4C71" };
  }
  if (p[0] === "phones") { phones.splice(phones.findIndex((x) => x.id === Number(p[1])), 1); return {}; }
  if (p[0] === "alerts" && p[2] === "ack") { const a = alerts.find((x) => x.id === Number(p[1])); if (a) a.acked = true; return {}; }
  if (p[0] === "alerts") return q.get("unacked") === "true" ? alerts.filter((a) => !a.acked) : alerts;
  if (p[0] === "recipes" && p.length === 1 && method === "GET") {
    return { recipes: recipes.map(({ id, name, enabled }) => ({ id, name, enabled })), max_recipes: 32 };
  }
  const recipe = recipes.find((r) => r.id === Number(p[1]));
  if (p[0] === "recipes" && p[2] === "enabled") { if (recipe) recipe.enabled = Boolean(b.enabled); return {}; }
  if (p[0] === "recipes" && method === "GET") return recipe ? { recipe } : null;
  if (p[0] === "recipes" && method === "POST") { const id = nextRecipe++; recipes.push({ ...(b as unknown as Recipe), id }); return { id }; }
  if (p[0] === "recipes" && method === "PUT" && recipe) { Object.assign(recipe, b, { id: recipe.id }); return { id: recipe.id }; }
  if (p[0] === "recipes" && method === "DELETE") { recipes.splice(recipes.findIndex((r) => r.id === Number(p[1])), 1); return {}; }
  return null;
}

window.farmlab = {
  api: async (method, path, body) => {
    const data = handle(method, path, body);
    return data === null ? { ok: false, status: 404, data: { error: `sample data has no ${method} ${path}` } } : { ok: true, status: 200, data };
  },
};
