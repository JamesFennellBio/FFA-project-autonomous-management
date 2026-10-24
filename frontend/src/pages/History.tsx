import { useMemo, useState } from "react";
import { CartesianGrid, Line, LineChart, ResponsiveContainer, Tooltip, XAxis, YAxis } from "recharts";
import { api, usePoll } from "../api";
import { chartValue, formatValue, unitOf } from "../format";
import type { Channel } from "../types";

const RANGES = [
  { label: "1 hour", secs: 3600 },
  { label: "24 hours", secs: 86400 },
  { label: "7 days", secs: 7 * 86400 },
  { label: "30 days", secs: 30 * 86400 },
];

export default function History() {
  const nodes = usePoll(api.nodes, 10000);
  const [sel, setSel] = useState<string>("");
  const [range, setRange] = useState(RANGES[1].secs);

  const options = useMemo(
    () =>
      (nodes.data ?? []).flatMap((n) =>
        n.channels.map((c) => ({ key: `${n.id}:${c.channel}`, label: `${n.name} — ${c.name}`, node: n.id, ch: c })),
      ),
    [nodes.data],
  );
  const current = options.find((o) => o.key === sel) ?? options[0];

  return (
    <>
      <h1>History</h1>
      <div className="toolbar">
        <select value={current?.key ?? ""} onChange={(e) => setSel(e.target.value)}>
          {options.map((o) => (
            <option key={o.key} value={o.key}>
              {o.label}
            </option>
          ))}
        </select>
        <div className="seg">
          {RANGES.map((r) => (
            <button key={r.secs} className={range === r.secs ? "seg-btn active" : "seg-btn"} onClick={() => setRange(r.secs)}>
              {r.label}
            </button>
          ))}
        </div>
      </div>
      {current ? <Chart node={current.node} ch={current.ch} range={range} /> : <p className="muted">No channels yet.</p>}
    </>
  );
}

function Chart({ node, ch, range }: { node: number; ch: Channel; range: number }) {
  const readings = usePoll(() => api.readings(node, ch.channel, Math.floor(Date.now() / 1000) - range), 5000, [node, ch.channel, range]);
  const data = (readings.data ?? []).map((r) => ({ t: r.ts * 1000, v: chartValue(ch.kind, r.value) }));
  const digital = ch.kind === "digital_in" || ch.kind === "digital_out";
  const fmtTime = (t: number) =>
    range > 86400
      ? new Date(t).toLocaleDateString([], { month: "short", day: "numeric" })
      : new Date(t).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });

  return (
    <section className="card chart-card">
      <div className="card-head">
        <h3>{ch.name}</h3>
        <span className="muted">Now: {formatValue(ch.kind, ch.last_value)}</span>
      </div>
      {readings.error && <div className="error">{readings.error}</div>}
      {data.length === 0 ? (
        <p className="muted">No readings in this time range.</p>
      ) : (
        <ResponsiveContainer width="100%" height={380}>
          <LineChart data={data} margin={{ top: 10, right: 20, bottom: 10, left: 0 }}>
            <CartesianGrid stroke="var(--grid)" strokeDasharray="3 3" />
            <XAxis dataKey="t" type="number" scale="time" domain={["dataMin", "dataMax"]} tickFormatter={fmtTime} stroke="var(--muted)" />
            <YAxis
              stroke="var(--muted)"
              domain={digital ? [0, 1] : ["auto", "auto"]}
              ticks={digital ? [0, 1] : undefined}
              unit={unitOf(ch.kind) ? ` ${unitOf(ch.kind)}` : ""}
              width={70}
            />
            <Tooltip
              labelFormatter={(t) => new Date(Number(t)).toLocaleString()}
              formatter={(v: number) => [digital ? (v ? "ON" : "OFF") : `${v.toFixed(2)} ${unitOf(ch.kind)}`, ch.name]}
              contentStyle={{ background: "var(--panel)", border: "1px solid var(--border)" }}
            />
            <Line type={digital ? "stepAfter" : "monotone"} dataKey="v" stroke="var(--accent)" dot={false} strokeWidth={2} isAnimationActive={false} />
          </LineChart>
        </ResponsiveContainer>
      )}
    </section>
  );
}
