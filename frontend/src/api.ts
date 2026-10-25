import { useCallback, useEffect, useRef, useState } from "react";
import type { Alert, NodeInfo, PendingPair, Phone, Reading, Rule, RuleInput, Status } from "./types";

type RawResponse = { ok: boolean; status: number; data: unknown };

declare global {
  interface Window {
    farmlab?: { api: (method: string, path: string, body?: unknown) => Promise<RawResponse> };
  }
}

export class ApiError extends Error {
  constructor(public status: number, message: string) {
    super(message);
  }
}

async function call<T>(method: string, path: string, body?: unknown): Promise<T> {
  if (!window.farmlab) throw new ApiError(0, "Run this inside the Electron app (npm run dev), not a plain browser.");
  const r = await window.farmlab.api(method, path, body);
  if (!r.ok) {
    const msg = (r.data as { error?: string } | null)?.error ?? `HTTP ${r.status}`;
    throw new ApiError(r.status, msg);
  }
  return r.data as T;
}

export const api = {
  status: () => call<Status>("GET", "/api/status"),
  nodes: () => call<NodeInfo[]>("GET", "/api/nodes"),
  renameNode: (id: number, name: string) => call("PATCH", `/api/nodes/${id}`, { name }),
  unpairNode: (id: number) => call("DELETE", `/api/nodes/${id}`),
  renameChannel: (id: number, ch: number, name: string) => call("PATCH", `/api/nodes/${id}/channels/${ch}`, { name }),
  command: (id: number, ch: number, value: number) => call("POST", `/api/nodes/${id}/channels/${ch}/command`, { value }),
  readings: (nodeId: number, channel: number, since: number) =>
    call<Reading[]>("GET", `/api/readings?node_id=${nodeId}&channel=${channel}&since=${since}`),
  pending: () => call<PendingPair[]>("GET", "/api/pairing/requests"),
  approve: (key: string, name?: string) => call("POST", `/api/pairing/requests/${key}/approve`, { name }),
  reject: (key: string) => call("POST", `/api/pairing/requests/${key}/reject`),
  phones: () => call<Phone[]>("GET", "/api/phones"),
  unpairPhone: (id: number) => call("DELETE", `/api/phones/${id}`),
  alerts: (unacked = false) => call<Alert[]>("GET", `/api/alerts?unacked=${unacked}`),
  ackAlert: (id: number) => call("POST", `/api/alerts/${id}/ack`),
  rules: () => call<Rule[]>("GET", "/api/rules"),
  createRule: (r: RuleInput) => call("POST", "/api/rules", r),
  updateRule: (id: number, r: RuleInput) => call("PUT", `/api/rules/${id}`, r),
  deleteRule: (id: number) => call("DELETE", `/api/rules/${id}`),
};

/** Polls `fn` every `ms`. Returns the latest data, the latest error and a manual refresh. */
export function usePoll<T>(fn: () => Promise<T>, ms: number, deps: unknown[] = []) {
  const [data, setData] = useState<T | null>(null);
  const [error, setError] = useState<string | null>(null);
  const fnRef = useRef(fn);
  fnRef.current = fn;

  const refresh = useCallback(async () => {
    try {
      setData(await fnRef.current());
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }, []);

  useEffect(() => {
    void refresh();
    const t = setInterval(() => void refresh(), ms);
    return () => clearInterval(t);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [ms, refresh, ...deps]);

  return { data, error, refresh };
}
