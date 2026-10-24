import { useState } from "react";
import { api, usePoll } from "./api";
import Dashboard from "./pages/Dashboard";
import History from "./pages/History";
import Pairing from "./pages/Pairing";
import Rules from "./pages/Rules";
import Alerts from "./pages/Alerts";
import Devices from "./pages/Devices";

type Page = "dashboard" | "history" | "rules" | "alerts" | "devices" | "pairing";

const PAGES: { id: Page; label: string }[] = [
  { id: "dashboard", label: "Dashboard" },
  { id: "history", label: "History" },
  { id: "rules", label: "Automation" },
  { id: "alerts", label: "Alerts" },
  { id: "devices", label: "Devices" },
  { id: "pairing", label: "Pairing" },
];

export default function App() {
  const [page, setPage] = useState<Page>("dashboard");
  const status = usePoll(api.status, 3000);
  const alerts = usePoll(() => api.alerts(true), 5000);
  const pending = usePoll(api.pending, 3000);

  const hub = status.data?.hub;
  const badge = (p: Page) => {
    if (p === "alerts" && alerts.data?.length) return <span className="badge">{alerts.data.length}</span>;
    if (p === "pairing" && pending.data?.length) return <span className="badge info">{pending.data.length}</span>;
    return null;
  };

  return (
    <div className="app">
      <aside className="sidebar">
        <div className="brand">
          <span className="logo">🌱</span>
          <div>
            <div className="brand-name">FFA Farm &amp; Lab</div>
            <div className="brand-sub">Autonomous management</div>
          </div>
        </div>
        <nav>
          {PAGES.map((p) => (
            <button key={p.id} className={page === p.id ? "nav active" : "nav"} onClick={() => setPage(p.id)}>
              {p.label}
              {badge(p.id)}
            </button>
          ))}
        </nav>
        <div className="hub-status">
          {status.error ? (
            <span className="dot bad" title={status.error}>Daemon offline</span>
          ) : !hub ? (
            <span className="dot">Connecting…</span>
          ) : !hub.paired ? (
            <span className="dot warn">Hub not paired</span>
          ) : hub.connected ? (
            <span className="dot good">Hub connected</span>
          ) : (
            <span className="dot bad">Hub offline</span>
          )}
          {hub?.fingerprint && <div className="fp small">Hub {hub.fingerprint}</div>}
          {status.data && <div className="fp small">PC {status.data.pc_fingerprint}</div>}
        </div>
      </aside>
      <main className="content">
        {status.error && <div className="banner error">{status.error}</div>}
        {page === "dashboard" && <Dashboard onGoPairing={() => setPage("pairing")} />}
        {page === "history" && <History />}
        {page === "rules" && <Rules />}
        {page === "alerts" && <Alerts onChange={alerts.refresh} />}
        {page === "devices" && <Devices />}
        {page === "pairing" && <Pairing status={status.data} pending={pending.data ?? []} refresh={pending.refresh} />}
      </main>
    </div>
  );
}
