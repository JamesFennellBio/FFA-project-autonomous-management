// The only file that touches the generated Rust bindings (mobile/native, built
// from mobile/core). If the generated API ever changes shape, fix it here.
//
// CRYPTO-REVIEW: all pairing, handshakes and encryption happen inside the
// Rust core. JavaScript sees the identity seed once (to store it) and JSON.
import {
  HubClient,
  fingerprintOfKey,
  fingerprintOfSeed,
  newIdentitySeed,
  type PhonePairing,
} from "farmlab-hub-native";

export interface SavedPairing {
  phoneId: number;
  hubPublicKey: string;
}

/** What the rest of the app uses. */
export interface NativeClient {
  request(json: string): Promise<string>;
  pair(timeoutSecs: number): Promise<SavedPairing>;
}

export function makeClient(
  seedHex: string,
  hubAddr: string,
  pairing: SavedPairing | null,
  expectedHubKey: string | null,
): NativeClient {
  const p: PhonePairing | undefined = pairing
    ? { phoneId: pairing.phoneId, hubPublicKey: pairing.hubPublicKey }
    : undefined;
  const c = new HubClient(seedHex, hubAddr, p, expectedHubKey ?? undefined);
  return {
    request: (json) => c.request(json),
    pair: async (secs) => {
      const r = await c.pair(secs);
      return { phoneId: r.phoneId, hubPublicKey: r.hubPublicKey };
    },
  };
}

export const newSeed = (): string => newIdentitySeed();
export const seedFingerprint = (seedHex: string): string => fingerprintOfSeed(seedHex);
export const keyFingerprint = (keyHex: string): string => fingerprintOfKey(keyHex);

/** Error text from a rejected native call (uniffi errors are Error subclasses). */
export function errorText(e: unknown): string {
  if (e instanceof Error) return e.message;
  return String(e);
}
