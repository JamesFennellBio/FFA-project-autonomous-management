import { useState } from "react";
import { api } from "../api";
import { ago } from "../format";
import type { PendingPair, Status } from "../types";

// Pairing is the one step where a person decides which keys to trust, so the
// fingerprint is front and center: compare it with the device's serial log
// (ESP node) or `hub-node fingerprint` (Pi) before you click Approve.

export default function Pairing({ status, pending, refresh }: { status: Status | null; pending: PendingPair[]; refresh: () => void }) {
  return (
    <>
      <h1>Pairing</h1>
      <section className="card">
        <h3>How to pair</h3>
        <ol className="steps">
          <li>
            <b>Hub (once):</b> put this PC's public key (below) in the Pi's <code>hub.toml</code> as{" "}
            <code>pc_public_key</code>, then start <code>hub-node</code>. It appears below; check that its fingerprint
            matches <code>hub-node fingerprint</code>.
          </li>
          <li>
            <b>Nodes:</b> once the node has started, press and hold its BOOT button for 3 seconds (not while plugging it
            in; that puts the ESP32 into flashing mode). Its fingerprint shows on the serial monitor and below.
          </li>
          <li>Approve it. The node then saves the hub's key and starts sending encrypted data.</li>
        </ol>
        {status && (
          <div className="pubkey">
            <div className="muted small">This PC's public key (for hub.toml):</div>
            <code className="fp">{status.pc_public_key}</code>
            <button onClick={() => void navigator.clipboard.writeText(status.pc_public_key)}>Copy</button>
          </div>
        )}
        {status && (
          <p className="muted small">
            This PC's fingerprint: <span className="fp">{status.pc_fingerprint}</span>
            {status.hub.fingerprint && (
              <>
                {" "}
                · Paired hub: <span className="fp">{status.hub.fingerprint}</span>
              </>
            )}
          </p>
        )}
      </section>

      <h2>Waiting for approval</h2>
      {pending.length === 0 ? (
        <p className="muted">Nothing is asking to pair right now. Requests show up here within a few seconds.</p>
      ) : (
        <div className="grid">
          {pending.map((p) => (
            <PendingCard key={p.public_key} p={p} refresh={refresh} />
          ))}
        </div>
      )}
    </>
  );
}

function PendingCard({ p, refresh }: { p: PendingPair; refresh: () => void }) {
  const [name, setName] = useState("");
  const [err, setErr] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const act = async (f: () => Promise<unknown>) => {
    setBusy(true);
    setErr(null);
    try {
      await f();
      refresh();
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <section className="card">
      <div className="card-head">
        <h3>{p.kind === "hub" ? "Hub (Raspberry Pi)" : "Data node (ESP32)"}</h3>
        <span className="pill">{p.transport ? p.transport.toUpperCase() : "LAN"}</span>
      </div>
      <div className="fingerprint">{p.fingerprint}</div>
      <p className="muted small">Seen {ago(p.seen_at)}. Only approve if this matches what the device shows.</p>
      {p.kind === "node" && (
        <input placeholder="Name (e.g. Greenhouse 1)" value={name} onChange={(e) => setName(e.target.value)} />
      )}
      <div className="row">
        <button className="primary" disabled={busy} onClick={() => act(() => api.approve(p.public_key, name || undefined))}>
          Approve
        </button>
        <button disabled={busy} onClick={() => act(() => api.reject(p.public_key))}>
          Dismiss
        </button>
      </div>
      {err && <div className="error small">{err}</div>}
    </section>
  );
}
