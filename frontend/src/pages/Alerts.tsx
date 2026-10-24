import { useState } from "react";
import { api, usePoll } from "../api";
import { ago } from "../format";

export default function Alerts({ onChange }: { onChange: () => void }) {
  const [all, setAll] = useState(false);
  const alerts = usePoll(() => api.alerts(!all), 5000, [all]);

  const ack = async (id: number) => {
    await api.ackAlert(id);
    alerts.refresh();
    onChange();
  };

  return (
    <>
      <h1>Alerts</h1>
      <label className="check">
        <input type="checkbox" checked={all} onChange={(e) => setAll(e.target.checked)} /> Show acknowledged
      </label>
      {alerts.error && <div className="error">{alerts.error}</div>}
      {alerts.data && alerts.data.length === 0 && <p className="muted">No alerts. 🎉</p>}
      <table className="table">
        <tbody>
          {(alerts.data ?? []).map((a) => (
            <tr key={a.id} className={a.acked ? "muted" : ""}>
              <td className="nowrap">{ago(a.ts)}</td>
              <td>{a.node_name ?? (a.node_id !== null ? `Node ${a.node_id}` : "System")}</td>
              <td>{a.message}</td>
              <td className="right">{!a.acked && <button onClick={() => ack(a.id)}>Acknowledge</button>}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </>
  );
}
