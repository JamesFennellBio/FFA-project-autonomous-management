// Placeholder so TypeScript can resolve the package before the native build.
// `npm run ubrn:android` / `npm run ubrn:ios` overwrites this file with the
// generated entry point, which loads the Rust library and re-exports
// mobile/core's API (HubClient, newIdentitySeed, ...).
//
// The app only uses this module through mobile/app/src/hub/native.ts.
export type PhonePairing = { phoneId: number; hubPublicKey: string };

export declare class HubClient {
  constructor(seedHex: string, hubAddr: string, pairing: PhonePairing | undefined, expectedHubKey: string | undefined);
  fingerprint(): Promise<string>;
  pairing(): Promise<PhonePairing | undefined>;
  pair(timeoutSecs: number): Promise<PhonePairing>;
  request(requestJson: string): Promise<string>;
}

export declare function newIdentitySeed(): string;
export declare function fingerprintOfSeed(seedHex: string): string;
export declare function fingerprintOfKey(publicKeyHex: string): string;

throw new Error("ffa-hub-native isn't built yet: run `npm run ubrn:android` or `npm run ubrn:ios` in mobile/native");
