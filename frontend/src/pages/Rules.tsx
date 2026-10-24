import { useMemo, useState } from "react";
import { api, usePoll } from "../api";
import { formatValue, toMilli, unitOf } from "../format";
import type { Channel, NodeInfo, Rule, RuleInput } from "../types";

// "If <sensor> goes above/below <threshold>, turn <output> on (and off again when it clears)."

export default function Rules() {
  const rules = usePoll(api.rules, 5000);
  const nodes = usePoll(api.nodes, 10000);
  const [editing, setEditing] = useState<Rule | "new" | null>(null);

  const chan = (nodeId: number, ch: number) => nodes.data?.find((n) => n.id === nodeId)?.channels.find((c) => c.channel === ch);
  const nodeName = (id: number) => nodes.data?.find((n) => n.id === id)?.name ?? `Node ${id}`;

  return (
    <>
      <h1>Automation</h1>
      <div className="toolbar">
        <button className="primary" onClick={() => setEditing("new")} disabled={!nodes.data?.length}>
          + New rule
        </button>
      </div>
      {rules.error && <div className="error">{rules.error}</div>}
      {editing && nodes.data && (
        <RuleForm
          nodes={nodes.data}
          rule={editing === "new" ? null : editing}
          onDone={() => {
            setEditing(null);
            rules.refresh();
          }}
        />
      )}
      {rules.data?.length === 0 && <p className="muted">No rules yet. Example: "If greenhouse temp &gt; 30 °C, turn the siren on."</p>}
      <table className="table">
        <tbody>
          {(rules.data ?? []).map((r) => {
            const src = chan(r.node_id, r.channel);
            const dst = chan(r.target_node, r.target_channel);
            return (
              <tr key={r.id} className={r.enabled ? "" : "muted"}>
                <td>
                  <b>{r.name}</b>
                  {r.active && <span className="pill warn">Active</span>}
                </td>
                <td>
                  If {nodeName(r.node_id)} / {src?.name ?? `ch ${r.channel}`} {r.op === "gt" ? ">" : "<"}{" "}
                  {src ? formatValue(src.kind, r.threshold) : r.threshold} → set {nodeName(r.target_node)} / {dst?.name ?? `ch ${r.target_channel}`}{" "}
                  {r.on_value ? "ON" : "OFF"}
                  {r.off_value !== null && <> (back to {r.off_value ? "ON" : "OFF"} when it clears)</>}
                </td>
                <td className="right nowrap">
                  <button onClick={() => setEditing(r)}>Edit</button>{" "}
                  <button
                    className="danger"
                    onClick={async () => {
                      await api.deleteRule(r.id);
                      rules.refresh();
                    }}
                  >
                    Delete
                  </button>
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </>
  );
}

function RuleForm({ nodes, rule, onDone }: { nodes: NodeInfo[]; rule: Rule | null; onDone: () => void }) {
  const sensors = useMemo(
    () => nodes.flatMap((n) => n.channels.filter((c) => c.kind !== "digital_out").map((c) => ({ n, c }))),
    [nodes],
  );
  const outputs = useMemo(
    () => nodes.flatMap((n) => n.channels.filter((c) => c.kind === "digital_out").map((c) => ({ n, c }))),
    [nodes],
  );
  const key = (n: number, c: number) => `${n}:${c}`;
  const findKind = (k: string): Channel | undefined => sensors.find((s) => key(s.n.id, s.c.channel) === k)?.c;

  const [name, setName] = useState(rule?.name ?? "");
  const [src, setSrc] = useState(rule ? key(rule.node_id, rule.channel) : sensors[0] ? key(sensors[0].n.id, sensors[0].c.channel) : "");
  const [dst, setDst] = useState(
    rule ? key(rule.target_node, rule.target_channel) : outputs[0] ? key(outputs[0].n.id, outputs[0].c.channel) : "",
  );
  const [op, setOp] = useState<"gt" | "lt">(rule?.op ?? "gt");
  const srcCh = findKind(src);
  const [threshold, setThreshold] = useState(
    rule && srcCh ? String(srcCh.kind === "temperature" || srcCh.kind === "analog_in" ? rule.threshold / 1000 : rule.threshold) : "",
  );
  const [onValue, setOnValue] = useState(rule?.on_value ?? 1);
  const [autoOff, setAutoOff] = useState(rule ? rule.off_value !== null : true);
  const [enabled, setEnabled] = useState(rule?.enabled ?? true);
  const [err, setErr] = useState<string | null>(null);

  const submit = async () => {
    const [nodeId, ch] = src.split(":").map(Number);
    const [tn, tc] = dst.split(":").map(Number);
    const t = Number(threshold);
    if (!name.trim() || !src || !dst || threshold === "" || Number.isNaN(t)) {
      setErr("Fill in every field.");
      return;
    }
    const body: RuleInput = {
      name: name.trim(),
      enabled,
      node_id: nodeId,
      channel: ch,
      op,
      threshold: srcCh ? toMilli(srcCh.kind, t) : t,
      target_node: tn,
      target_channel: tc,
      on_value: onValue,
      off_value: autoOff ? (onValue ? 0 : 1) : null,
    };
    try {
      if (rule) await api.updateRule(rule.id, body);
      else await api.createRule(body);
      onDone();
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e));
    }
  };

  if (sensors.length === 0 || outputs.length === 0) {
    return <div className="banner">Rules need at least one sensor channel and one output channel.</div>;
  }

  return (
    <section className="card form">
      <h3>{rule ? "Edit rule" : "New rule"}</h3>
      <label>
        Name <input value={name} onChange={(e) => setName(e.target.value)} placeholder="Greenhouse too hot" />
      </label>
      <div className="row wrap">
        <label>
          If
          <select value={src} onChange={(e) => setSrc(e.target.value)}>
            {sensors.map(({ n, c }) => (
              <option key={key(n.id, c.channel)} value={key(n.id, c.channel)}>
                {n.name} / {c.name}
              </option>
            ))}
          </select>
        </label>
        <label>
          is
          <select value={op} onChange={(e) => setOp(e.target.value as "gt" | "lt")}>
            <option value="gt">above</option>
            <option value="lt">below</option>
          </select>
        </label>
        <label>
          <input type="number" step="any" value={threshold} onChange={(e) => setThreshold(e.target.value)} style={{ width: 100 }} />{" "}
          {srcCh ? unitOf(srcCh.kind) : ""}
        </label>
      </div>
      <div className="row wrap">
        <label>
          then turn
          <select value={dst} onChange={(e) => setDst(e.target.value)}>
            {outputs.map(({ n, c }) => (
              <option key={key(n.id, c.channel)} value={key(n.id, c.channel)}>
                {n.name} / {c.name}
              </option>
            ))}
          </select>
        </label>
        <label>
          <select value={onValue} onChange={(e) => setOnValue(Number(e.target.value))}>
            <option value={1}>ON</option>
            <option value={0}>OFF</option>
          </select>
        </label>
      </div>
      <label className="check">
        <input type="checkbox" checked={autoOff} onChange={(e) => setAutoOff(e.target.checked)} /> Switch it back when the condition clears
      </label>
      <label className="check">
        <input type="checkbox" checked={enabled} onChange={(e) => setEnabled(e.target.checked)} /> Enabled
      </label>
      {err && <div className="error">{err}</div>}
      <div className="row">
        <button className="primary" onClick={submit}>
          Save
        </button>
        <button onClick={onDone}>Cancel</button>
      </div>
    </section>
  );
}
