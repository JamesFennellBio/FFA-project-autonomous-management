import * as SecureStore from "expo-secure-store";
import { createContext, useCallback, useContext, useEffect, useMemo, useState, type ReactNode } from "react";

import { Hub } from "./client";
import { keyFingerprint, makeClient, newSeed, seedFingerprint, type SavedPairing } from "./native";

// Everything is kept in the phone's secure storage (iOS Keychain / Android
// Keystore), readable only while the phone is unlocked and never backed up.
// CRYPTO-REVIEW: "seed" is this phone's long-term Ed25519 secret.
const KEYS = {
  seed: "ffa.seed",
  addr: "ffa.hub_addr",
  pin: "ffa.hub_key",
  pairing: "ffa.pairing",
} as const;
const STORE_OPTS: SecureStore.SecureStoreOptions = {
  keychainAccessible: SecureStore.WHEN_UNLOCKED_THIS_DEVICE_ONLY,
};

export const DEFAULT_PORT = 7301;

interface Settings {
  seed: string;
  hubAddr: string | null;
  /** Optional hub public key to pin before pairing (hex). */
  hubKey: string | null;
  pairing: SavedPairing | null;
}

interface HubState {
  ready: boolean;
  /** Shown in the desktop app when this phone asks to pair. */
  phoneFingerprint: string | null;
  hubAddr: string | null;
  hubKey: string | null;
  pairing: SavedPairing | null;
  hubFingerprint: string | null;
  /** Set once the phone is paired and knows the hub's address. */
  hub: Hub | null;
  /** Changes the saved hub address (and optional pinned key). */
  setHub(addr: string, hubKey: string | null): Promise<void>;
  /** Saves the address, then waits for approval in the desktop app (up to 2 minutes). */
  pair(addr: string, hubKey: string | null): Promise<void>;
  forget(): Promise<void>;
}

const Ctx = createContext<HubState | null>(null);

/** "192.168.1.20" -> "192.168.1.20:7301" */
export function withPort(addr: string): string {
  const a = addr.trim();
  return /:\d+$/.test(a) ? a : `${a}:${DEFAULT_PORT}`;
}

export function HubProvider({ children }: { children: ReactNode }) {
  const [s, setS] = useState<Settings | null>(null);

  useEffect(() => {
    void (async () => {
      let seed = await SecureStore.getItemAsync(KEYS.seed);
      if (!seed) {
        seed = newSeed();
        await SecureStore.setItemAsync(KEYS.seed, seed, STORE_OPTS);
      }
      const pairingJson = await SecureStore.getItemAsync(KEYS.pairing);
      setS({
        seed,
        hubAddr: await SecureStore.getItemAsync(KEYS.addr),
        hubKey: await SecureStore.getItemAsync(KEYS.pin),
        pairing: pairingJson ? (JSON.parse(pairingJson) as SavedPairing) : null,
      });
    })();
  }, []);

  // One native client per (address, pairing). It holds the session keys.
  const native = useMemo(
    () => (s?.hubAddr ? makeClient(s.seed, s.hubAddr, s.pairing, s.hubKey) : null),
    [s?.seed, s?.hubAddr, s?.pairing, s?.hubKey],
  );
  const hub = useMemo(() => (native && s?.pairing ? new Hub(native) : null), [native, s?.pairing]);

  const save = useCallback(async (addr: string, hubKey: string | null) => {
    const a = withPort(addr);
    const k = hubKey?.trim() || null;
    await SecureStore.setItemAsync(KEYS.addr, a, STORE_OPTS);
    if (k) await SecureStore.setItemAsync(KEYS.pin, k, STORE_OPTS);
    else await SecureStore.deleteItemAsync(KEYS.pin);
    return { a, k };
  }, []);

  const setHub = useCallback(
    async (addr: string, hubKey: string | null) => {
      const { a, k } = await save(addr, hubKey);
      setS((old) => (old ? { ...old, hubAddr: a, hubKey: k } : old));
    },
    [save],
  );

  const pair = useCallback(
    async (addr: string, hubKey: string | null) => {
      if (!s) throw new Error("Still starting up.");
      const { a, k } = await save(addr, hubKey);
      // A client of its own: the memoized one may still have the old address.
      const p = await makeClient(s.seed, a, null, k).pair(120);
      await SecureStore.setItemAsync(KEYS.pairing, JSON.stringify(p), STORE_OPTS);
      setS((old) => (old ? { ...old, hubAddr: a, hubKey: k, pairing: p } : old));
    },
    [s, save],
  );

  const forget = useCallback(async () => {
    await SecureStore.deleteItemAsync(KEYS.pairing);
    setS((old) => (old ? { ...old, pairing: null } : old));
  }, []);

  const value: HubState = {
    ready: s != null,
    phoneFingerprint: s ? seedFingerprint(s.seed) : null,
    hubAddr: s?.hubAddr ?? null,
    hubKey: s?.hubKey ?? null,
    pairing: s?.pairing ?? null,
    hubFingerprint: s?.pairing ? keyFingerprint(s.pairing.hubPublicKey) : null,
    hub,
    setHub,
    pair,
    forget,
  };
  return <Ctx.Provider value={value}>{children}</Ctx.Provider>;
}

export function useHub(): HubState {
  const v = useContext(Ctx);
  if (!v) throw new Error("useHub outside HubProvider");
  return v;
}
