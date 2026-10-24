import { useState } from "react";
import { api, usePoll } from "../api";
import { ago, formatValue, isOnline, kindLabel } from "../format";
import type { Channel, NodeInfo } from "../types";

export default function Dashboard({ onGoPairing }: { onGoPairing: () => void }) {
  const nodes = usePoll(api.nodes, 3000);

  if (nodes.error) return <div className="banner error">{nodes.error}</div>;
  if (!nodes.data) return <p className="muted">Loading…</p>;
  if (nodes.data.length === 0) {
    return (
      <div className="empty">
        <h2>No nodes yet</h2>
        <p>Hold the pairing button on an ESP32 node, then approve it on the Pairing page.</p>
        <button className="primary" onClick={onGoPairing}>
          Go to Pairing
        </button>
      </div>
    );
  }

  return (
    <>
      <h1>Dashboard</h1>
      <div className="grid">
        {nodes.data.map((n) => (
          <NodeCard key={n.id} node={n} onChange={nodes.refresh} />
        ))}
      </div>
    </>
  );
}

function NodeCard({ node, onChange }: { node: NodeInfo; onChange: () => void }) {
  const online = isOnline(node.last_seen);
  const sensors = node.channels.filter((c) => c.kind !== "digital_out");
  const outputs = node.channels.filter((c) => c.kind === "digital_out");
  return (
    <section className="card">
      <header className="card-head">
        <div>
          <h3>{node.name}</h3>
          <div className="muted small">
            #{node.id} · {node.transport ? node.transport.toUpperCase() : "—"} · seen {ago(node.last_seen)}
          </div>
        </div>
        <span className={online ? "pill good" : "pill bad"}>{online ? "Online" : "Offline"}</span>
      </header>
      {node.channels.length === 0 && <p className="muted small">Waiting for the node to report its channels…</p>}
      <div className="sensors">
        {sensors.map((c) => (
          <div key={c.channel} className="sensor">
            <div className="muted small">{c.name}</div>
            <div className="value">{formatValue(c.kind, c.last_value)}</div>
            <div className="muted tiny">{kindLabel(c.kind)}</div>
          </div>
        ))}
      </div>
      {outputs.length > 0 && (
        <div className="outputs">
          {outputs.map((c) => (
            <OutputToggle key={c.channel} node={node} ch={c} onChange={onChange} />
          ))}
        </div>
      )}
    </section>
  );
}

function OutputToggle({ node, ch, onChange }: { node: NodeInfo; ch: Channel; onChange: () => void }) {
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const on = !!ch.last_value;
  const set = async (value: number) => {
    setBusy(true);
    setErr(null);
    try {
      await api.command(node.id, ch.channel, value);
      onChange();
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <div className="output">
      <span>{ch.name}</span>
      <div className="seg">
        <button className={!on ? "seg-btn active" : "seg-btn"} disabled={busy} onClick={() => set(0)}>
          Off
        </button>
        <button className={on ? "seg-btn active on" : "seg-btn"} disabled={busy} onClick={() => set(1)}>
          On
        </button>
      </div>
      {err && <div className="error small">{err}</div>}
    </div>
  );
}
