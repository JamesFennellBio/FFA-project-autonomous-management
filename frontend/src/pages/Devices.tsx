import { useState } from "react";
import { api, usePoll } from "../api";
import { ago, kindLabel } from "../format";
import type { NodeInfo } from "../types";

export default function Devices() {
  const nodes = usePoll(api.nodes, 10000);
  return (
    <>
      <h1>Devices</h1>
      {nodes.error && <div className="error">{nodes.error}</div>}
      {(nodes.data ?? []).map((n) => (
        <DeviceCard key={n.id} node={n} refresh={nodes.refresh} />
      ))}
    </>
  );
}

function DeviceCard({ node, refresh }: { node: NodeInfo; refresh: () => void }) {
  const [name, setName] = useState(node.name);
  const [chNames, setChNames] = useState<Record<number, string>>({});

  const save = async () => {
    if (name !== node.name) await api.renameNode(node.id, name);
    for (const [ch, n] of Object.entries(chNames)) await api.renameChannel(node.id, Number(ch), n);
    setChNames({});
    refresh();
  };
  const unpair = async () => {
    if (!confirm(`Unpair "${node.name}"? It will have to be paired again (history is kept).`)) return;
    await api.unpairNode(node.id);
    refresh();
  };

  return (
    <section className="card">
      <div className="card-head">
        <input className="title-input" value={name} onChange={(e) => setName(e.target.value)} />
        <div className="row">
          <button className="primary" onClick={save}>
            Save
          </button>
          <button className="danger" onClick={unpair}>
            Unpair
          </button>
        </div>
      </div>
      <p className="muted small">
        Node #{node.id} · fingerprint <span className="fp">{node.fingerprint}</span> · {node.sessions} key exchanges · paired{" "}
        {ago(node.paired_at)}
      </p>
      <table className="table">
        <thead>
          <tr>
            <th>Ch</th>
            <th>Type</th>
            <th>Name</th>
          </tr>
        </thead>
        <tbody>
          {node.channels.map((c) => (
            <tr key={c.channel}>
              <td>{c.channel}</td>
              <td>{kindLabel(c.kind)}</td>
              <td>
                <input value={chNames[c.channel] ?? c.name} onChange={(e) => setChNames({ ...chNames, [c.channel]: e.target.value })} />
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </section>
  );
}
