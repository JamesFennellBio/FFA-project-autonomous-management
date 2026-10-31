import { useEffect, useState } from "react";
import { api, usePoll } from "../api";
import { ago } from "../format";
import type { PendingPair, PhoneTicket, Status } from "../types";

// Pairing is the one step where a person decides which keys to trust:
// - the hub proves it's the right Pi with the one-time code `hub-node pair` prints
// - phones scan a one-time QR code made here
// - nodes are approved by comparing the fingerprint with their serial log

export default function Pairing({ status, pending, refresh }: { status: Status | null; pending: PendingPair[]; refresh: () => void }) {
  return (
    <>
      <h1>Pairing</h1>
      <section className="card">
        <h3>How to pair</h3>
        <ol className="steps">
          <li>
            <b>Hub (once):</b> on the Pi run <code>sudo -u farmlab hub-node pair</code>. Enter the address and the
            12-digit code it prints under <i>Add hub</i> below. Only the Pi that printed the code can complete it.
          </li>
          <li>
            <b>Nodes:</b> once the node has started, press and hold its BOOT button for 3 seconds (not while plugging it
            in; that puts the ESP32 into flashing mode). Its fingerprint shows on the serial monitor and below.
          </li>
          <li>Approve it. The node then saves the hub's key and starts sending encrypted data.</li>
          <li>
            <b>Phones:</b> click <i>Add phone</i> below and scan the QR code with the phone app. No approval needed;
            the code works once, for 5 minutes. (Pairing by address still works: approve the phone below.)
          </li>
        </ol>
        {status && (
          <div className="pubkey">
            <div className="muted small">This PC's key ID (optional extra pin, <code>pc_key_id</code> in hub.toml):</div>
            <code className="fp">{status.pc_key_id}</code>
            <button onClick={() => void navigator.clipboard.writeText(status.pc_key_id)}>Copy</button>
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

      <div className="grid">
        <AddHub paired={!!status?.hub.paired} refresh={refresh} />
        <AddPhone hubOnline={!!status?.hub.connected} />
      </div>

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

      <Phones />
    </>
  );
}

const KIND_TITLE = { node: "Data node (ESP32)", phone: "Phone app" } as const;

const errText = (e: unknown) => (e instanceof Error ? e.message : String(e));

function AddHub({ paired, refresh }: { paired: boolean; refresh: () => void }) {
  const [address, setAddress] = useState("");
  const [code, setCode] = useState("");
  const [busy, setBusy] = useState(false);
  const [msg, setMsg] = useState<{ ok: boolean; text: string } | null>(null);
  const submit = async () => {
    if (paired && !confirm("Replace the paired hub? The current one stops sending data here.")) return;
    setBusy(true);
    setMsg(null);
    try {
      const r = await api.pairHub(address.trim(), code);
      setMsg({ ok: true, text: `Paired with hub ${r.fingerprint}. It connects within a few seconds.` });
      setCode("");
      refresh();
    } catch (e) {
      setMsg({ ok: false, text: errText(e) });
    } finally {
      setBusy(false);
    }
  };
  return (
    <section className="card">
      <h3>{paired ? "Replace hub" : "Add hub"}</h3>
      <p className="muted small">
        Run <code>hub-node pair</code> on the Pi (add <code>--replace</code> if it was paired before).
      </p>
      <input placeholder="Pi address, e.g. 192.168.1.20" value={address} onChange={(e) => setAddress(e.target.value)} />
      <input
        className="fp"
        placeholder="Code, e.g. 4821-7730-1956"
        inputMode="numeric"
        autoComplete="off"
        value={code}
        onChange={(e) => setCode(e.target.value)}
      />
      <div className="row">
        <button className="primary" disabled={busy || !address.trim() || code.replace(/\D/g, "").length !== 12} onClick={() => void submit()}>
          {busy ? "Pairing…" : "Pair hub"}
        </button>
      </div>
      {msg && <div className={msg.ok ? "small" : "error small"}>{msg.text}</div>}
    </section>
  );
}

function AddPhone({ hubOnline }: { hubOnline: boolean }) {
  const [ticket, setTicket] = useState<PhoneTicket | null>(null);
  const [paired, setPaired] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const [now, setNow] = useState(() => Math.floor(Date.now() / 1000));

  // While a code is open: tick the countdown and watch for the phone to show up.
  useEffect(() => {
    if (!ticket || paired) return;
    const t = setInterval(() => {
      setNow(Math.floor(Date.now() / 1000));
      api
        .phones()
        .then((list) => list.some((p) => p.id === ticket.phone_id) && setPaired(true))
        .catch(() => {});
    }, 1000);
    return () => clearInterval(t);
  }, [ticket, paired]);

  const make = async () => {
    setErr(null);
    setPaired(false);
    try {
      setTicket(await api.phoneTicket());
      setNow(Math.floor(Date.now() / 1000));
    } catch (e) {
      setErr(errText(e));
    }
  };
  const left = ticket ? ticket.expires - now : 0;
  return (
    <section className="card">
      <h3>Add phone</h3>
      {!ticket && (
        <p className="muted small">Shows a one-time QR code for the phone app to scan.</p>
      )}
      {ticket && paired && <p>Phone #{ticket.phone_id} is paired.</p>}
      {ticket && !paired && left > 0 && (
        <>
          {/* SVG comes from our own daemon (Rust qrcode crate), not from the network. */}
          <div className="qr" dangerouslySetInnerHTML={{ __html: ticket.svg }} />
          <p className="muted small">
            Works once, for {Math.floor(left / 60)}:{String(left % 60).padStart(2, "0")} more. The phone should show hub{" "}
            <span className="fp">{ticket.hub_fingerprint}</span>.
          </p>
        </>
      )}
      {ticket && !paired && left <= 0 && <p className="muted small">That code expired.</p>}
      <div className="row">
        <button className="primary" disabled={!hubOnline} onClick={() => void make()}>
          {ticket ? "New code" : "Show QR code"}
        </button>
        {ticket && !paired && left > 0 && (
          <button onClick={() => void navigator.clipboard.writeText(ticket.uri)}>Copy link</button>
        )}
        {!hubOnline && <span className="muted small">The hub must be online.</span>}
      </div>
      {err && <div className="error small">{err}</div>}
    </section>
  );
}

function Phones() {
  const phones = usePoll(api.phones, 10_000);
  const [err, setErr] = useState<string | null>(null);
  const remove = async (id: number, name: string) => {
    if (!confirm(`Unpair ${name}? It stops working until it is paired again.`)) return;
    setErr(null);
    try {
      await api.unpairPhone(id);
      phones.refresh();
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e));
    }
  };
  const list = phones.data ?? [];
  return (
    <>
      <h2>Paired phones</h2>
      {list.length === 0 ? (
        <p className="muted">No phones yet.</p>
      ) : (
        <div className="grid">
          {list.map((p) => (
            <section className="card" key={p.id}>
              <div className="card-head">
                <h3>{p.name}</h3>
                <span className="pill">#{p.id}</span>
              </div>
              <div className="fingerprint">{p.fingerprint}</div>
              <p className="muted small">Paired {ago(p.paired_at)}.</p>
              <button onClick={() => void remove(p.id, p.name)}>Unpair</button>
            </section>
          ))}
        </div>
      )}
      {err && <div className="error small">{err}</div>}
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
        <h3>{KIND_TITLE[p.kind]}</h3>
        <span className="pill">{p.transport ? p.transport.toUpperCase() : "LAN"}</span>
      </div>
      <div className="fingerprint">{p.fingerprint}</div>
      <p className="muted small">Seen {ago(p.seen_at)}. Only approve if this matches what the device shows.</p>
      <input
        placeholder={p.kind === "phone" ? "Name (e.g. Sam's phone)" : "Name (e.g. Greenhouse 1)"}
        value={name}
        onChange={(e) => setName(e.target.value)}
      />
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
