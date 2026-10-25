# Crypto review guide

## How to use this
- Every crypto line tagged `// CRYPTO-REVIEW:`
- Index of all tags at the bottom; regenerate: `./scripts/crypto-review.sh`
- Start in `crypto-sharecrate/src/crypto/` (~600 lines, no_std)
- Security properties as tests: `crypto-sharecrate/tests/protocol.rs`
- Other crates only call into it (+ RNG, key storage)

## Primitives
- Identity / signatures: Ed25519 (`verify_strict`), `ed25519-dalek` 2
- Key agreement: X25519, ephemeral both sides, `x25519-dalek` 2
- Key derivation: HKDF-SHA256, `hkdf` 0.12, `sha2` 0.10
- Encryption: ChaCha20-Poly1305 (IETF, 96-bit nonce), `chacha20poly1305` 0.10
- Hub recovery: BIP39 24 words → PBKDF2-SHA512 → HKDF → Ed25519 seed, `bip39` 2
- Node RNG: SHA-256 hash-DRBG seeded from the C3 TRNG, `data-node/src/rng.rs`
- All pure-Rust RustCrypto / dalek crates
- Hand-rolled: only protocol framing, DRBG, HKDF labels

## Who trusts whom
- Node (key A) pins hub key H at pairing; hub pins A after Approve
- Hub pins PC key P; PC pins H (hub plays the "node" role toward the PC)
- Phone (key F) pins H; hub pins F after Approve on the desktop; phone never talks to the PC
- Desktop app → daemon: loopback HTTP, 256-bit bearer token
- Token stays in the Electron main process, page never sees it

## Pairing (`crypto/pairing.rs`)
- Node holds BOOT 3 s → `PAIR_REQUEST = 0x08 | pubA | Sig_A("FFA-PAIR-REQ-v1" ‖ 0x08 ‖ pubA)` (proves key ownership)
- Hub forwards to PC; app shows fingerprint = first 64 bits of SHA-256(pubA)
- Person compares with node serial log → Approve
- Next request: `PAIR_ACCEPT = 0x09 | node_id | pubH | Sig_H("FFA-PAIR-ACC-v1" ‖ pubA ‖ 0x09 ‖ node_id ‖ pubH)`
- Signature covers pubA → accept for another device fails
- Pinned keys (findings 03/17/12):
  - `FFA_HUB_KEY` on nodes → only that hub (`NodeCore::with_expected_peer`, `WrongPeer`)
  - closes the trust-on-first-use race
  - hub requires `pc_public_key` in hub.toml
  - saved pairing for a different key → dropped, re-pair with the pinned peer

## Session handshake (`crypto/handshake.rs`)
- `HELLO I→R: 0x03 | id | eI(32) | nI(16) | Sig_I("FFA-HELLO-v1" ‖ bytes 0..51)`
- `HELLO-ACK R→I: 0x04 | id | epoch(4) | eR(32) | nR(16) | Sig_R("FFA-HELLO-ACK-v1" ‖ HELLO[0..51] ‖ ACK[0..55])`
- `ss = X25519(eI, eR)`, rejected if not contributory
- `salt = SHA-256("FFA-TRANSCRIPT-v1" ‖ HELLO[0..51] ‖ ACK[0..55])`
- `k_IR = HKDF(ss, salt, "FFA-v1 key init->resp")`
- `k_RI = HKDF(ss, salt, "FFA-v1 key resp->init")`
- SIGMA-like signed DH (close to TLS 1.3 signature-authenticated ECDHE)
- Mutual auth: both sign with pinned identity keys
- Forward secrecy: ephemeral `StaticSecret`s zeroized on drop
- Binding: Sig_R covers the whole HELLO → ACK can't be moved to another HELLO
- Key confirmation: responder keeps session pending until a frame decrypts (`crypto/responder.rs`)
  - replayed / forged HELLO can't knock out the live session

## Rekeying: 10 messages or 1 hour (`crypto/session.rs`)
- `MAX_MSGS = 10`: counters 0..=9 per direction, 10th refused on send and receive
- `MAX_AGE_SECS = 3600`: senders stop at 1 h; receivers allow 60 s grace
- `needs_rekey()` counts both directions → initiator rekeys when the responder runs out
- Monotonic clocks (embassy `Instant` on node, `Instant` on hub / PC) → NTP can't make a session look younger

## Frames (`frame.rs`)
- `type(1) | node_id(2) | epoch(4) | counter(4) | dir(1) | ciphertext | tag(16)`
- First 12 bytes = AAD (authenticated, not encrypted)
- `nonce = dir(1) | epoch(4) | counter(4) | 000000`
- Radio frames always 64 bytes (plaintext padded to 36) → all look the same on air
- Replay: counter must be > last verified; gaps allowed (lost LoRa packets)
- Replay state updates only after the tag verifies

## Phone app (`mobile/core`, `hub-node/src/hub/app.rs`)
- Phone side in Rust (uniffi from React Native), uses `NodeCore` from crypto-sharecrate
- Same signed PAIR_REQUEST, signed X25519 HELLO, HKDF, 10 msg / 1 h rekey
- JavaScript only sees JSON
- Frames: one `FrameType::Uplink` per request / reply (same sealing as hub→PC)
  - max 1200 bytes JSON → no IP fragmentation
  - one request → exactly one reply
  - phone counts the hub's replies → rekeys before the hub runs out
- Separate trust lists: `phones` / `approved_phones` in `paired.json`, phone port only
  - node key can't open a session on the phone port, phone key can't on the node port
- Paired phone can:
  - read values, history, alerts
  - set outputs (hub checks the channel was announced as an output)
  - manage recipes
  - approve nodes currently asking to pair (PC assigns id; `approve_key(.., nodes_only = true)` refuses hubs / phones)
- Paired phone can't: add phones, unpair anything, change hub settings
- Recipes = data, not code:
  - fixed condition / action types, `deny_unknown_fields`
  - limits: size, depth 4, 16 conditions, 8 actions per list
  - every target a paired node, every "set" an announced output
- Without a pinned hub key → trust-on-first-use (Settings has optional hub key → `WrongPeer` otherwise)
- Phone seed: OS RNG in Rust → JS once → `expo-secure-store`, `WHEN_UNLOCKED_THIS_DEVICE_ONLY` (Keychain / Keystore, not backed up)
  - sits in JS memory while the app runs
- Rate limits: shares hub's 50/s signature budget; 5 verified HELLOs / 10 s per phone; requests need a valid session

## Known weaknesses
- Node pairing without `FFA_HUB_KEY` = trust-on-first-use (60 s window); hub side always needs approval
- Keys at rest not encrypted:
  - node seed in plain flash at 0x9000
  - hub `identity.key`, PC `identity.key` / `api-token` = 0600 files
  - fix for the node: ESP32 flash encryption + secure boot (irreversible eFuse)
- Custom DRBG on the node (`data-node/src/rng.rs`): simple SHA-256, not NIST HMAC-DRBG; TRNG (ADC noise) seed at boot
- Metadata in the clear (authenticated): frame type, node id, epoch, counter
- Radio jamming can't be prevented; limited now:
  - 50 Ed25519 checks/s on the hub, 20 on the PC hub port
  - 5 verified HELLOs / 10 s per node (forged ones don't count)
  - 1 forwarded pairing request/s; never stored in the hub DB
  - rate-limit tables capped
  - replayed HELLO rejected by nonce; 4 pending slots
  - many recorded HELLOs replayed fast can still delay sessions (≈ jamming)
- Hub→PC link is this custom protocol, not TLS:
  - 4 connections max on the PC port
  - unproven connection: 10 s per frame, 30 s total, evicted first
  - verified hub never evicted
  - batches over 116 messages close the connection
- Timestamps from the hub clock (no RTC on Pi 3B); backward clock step can make the PC skip readings as duplicates (finding 19)
- Wi-Fi password compiled into node firmware
- No node key rotation (factory reset: hold BOOT 10 s + re-pair)
- No revocation broadcast to nodes; node keeps hub key until BOOT held 10 s
- Lost phone works until unpaired on the desktop (Pairing → Paired phones)
  - "Forget this hub" on the phone only removes the phone's side

## Fixed after the Oct 24 2026 scan
- 01 failsafe only re-armed by new session → any authenticated frame re-arms (`data-node/src/main.rs::contact`)
- 02/08 pairing spam filled hub DB/RAM → live-only, never stored, 1/s, key table capped at 1024
- 03/17 node TOFU race → optional `FFA_HUB_KEY` pin
- 04 unlimited TCP connections → 4-connection cap, first-frame + idle timeouts
- 09 HELLO / PAIR CPU flood → per-node + global signature budgets
- 10/18 "delivered" without proof → node sends encrypted `Ack`; queued→sent→acked
- 11/15 init said nothing stored → output + docs corrected
- 12 hub re-pairs with any PC → `pc_public_key` required
- 14 any TCP disconnect cleared hub link → only the registered connection can
- 16/22/25/31 node id reuse / collisions → ids never reused, rules deleted with node, hub won't re-point an id
- 24 deleted active rule left output on → off command on delete
- 27 `restore` overwrote identity → needs `--force`
- 29 replayed HELLO blocked sessions → nonce cache + 4 pending slots
- 30 silent command drop → `CommandDropped` alert + status `dropped`

## Fixed after the second scan (f1122480, 0 critical / 0 high)
- Forged HELLO-ACK cancels handshake (medium) → `Initiator::finish(&self)`; bad reply leaves it pending, timeout not reset
- Forged HELLOs use up node allowance (medium) → only verified HELLOs count
- Junk TCP connections hold all hub-port slots (medium) → only pinned / approved hub is proven; unproven max 30 s, evicted oldest-first
- Editing an active rule leaves output latched (medium) → off command sent before edit
- Unpair leaves waiting approval (low) → cleared on unpair
- No batch size cap (low) → `MAX_BATCH_MSGS` both ends; PC splits big queues
- Unbounded signature checks on PC hub port (low) → 20/s

## Tests for these properties
- `cargo test -p crypto-sharecrate`:
  - `rekey_after_ten_messages`, `rekey_after_one_hour`, `hub_rejects_expired_session`
  - `tampered_ciphertext_header_and_tag_rejected`, `replay_rejected`
  - `hello_signed_by_wrong_key_rejected`, `ack_from_wrong_hub_rejected`
  - `replayed_hello_rejected_and_live_session_survives`, `old_replay_cannot_evict_genuine_pending_session`
  - `pinned_hub_key_rejects_other_hubs`, `saved_pairing_with_wrong_key_is_dropped_when_pinned`
  - `pair_accept_for_other_device_rejected`, `forged_pair_request_rejected`
  - `forged_hello_ack_does_not_cancel_handshake`
- `cargo test -p hub-node`:
  - `hello_flood_is_rate_limited`, `forged_hellos_do_not_use_up_a_nodes_allowance`
  - `unpair_clears_waiting_approval`, `pair_request_flood_is_rate_limited_and_never_stored`, `approval_for_taken_id_is_refused`
  - phone port: `node_frames_on_the_phone_port_and_unknown_phones_are_dropped`, `phone_approves_only_nodes_that_are_asking`, `many_requests_rekey_and_keep_working`, `recipes_with_bad_targets_are_refused`
  - `recipes::tests::rejects_bad_shapes`
- `cargo test -p ffa-mobile-core`: `a_pinned_hub_key_refuses_any_other_hub`, `pairs_then_keeps_working_across_rekeys`, `a_lost_reply_is_retried`
- `cargo test -p pc-daemon`: `phones_pair_with_their_own_ids_and_can_only_approve_nodes`

## Every CRYPTO-REVIEW location

<!-- BEGIN CRYPTO-REVIEW INDEX (generated by scripts/crypto-review.sh, do not edit) -->

| File | Line | Note |
|------|------|------|
| `crypto-sharecrate/Cargo.toml` | 15 | these are the only crypto dependencies in the whole system. |
| `crypto-sharecrate/src/crypto/handshake.rs` | 1 | the ECDH handshake. Runs once per session (every 10 messages |
| `crypto-sharecrate/src/crypto/handshake.rs` | 48 | these MUST come from a CSPRNG. On the ESP32-C3, esp-hal's |
| `crypto-sharecrate/src/crypto/handshake.rs` | 55 | StaticSecret zeroizes on drop ("zeroize" feature). It is |
| `crypto-sharecrate/src/crypto/handshake.rs` | 108 | X25519, then reject a non-contributory (all-zero) result |
| `crypto-sharecrate/src/crypto/handshake.rs` | 134 | verify the initiator's signature *before* any ECDH work. |
| `crypto-sharecrate/src/crypto/handshake.rs` | 196 | the signature covers OUR hello, so this ACK answers |
| `crypto-sharecrate/src/crypto/identity.rs` | 1 | long-term Ed25519 identities for nodes, the hub and the PC daemon. |
| `crypto-sharecrate/src/crypto/identity.rs` | 14 | the 32-byte seed is the whole secret. Nodes keep it in flash, |
| `crypto-sharecrate/src/crypto/identity.rs` | 22 | `seed` must come from a CSPRNG (or the BIP39 phrase on the hub). |
| `crypto-sharecrate/src/crypto/identity.rs` | 38 | plain Ed25519 (RFC 8032) over the message. Every signed |
| `crypto-sharecrate/src/crypto/identity.rs` | 46 | `verify_strict` rejects weak/small-order public keys and |
| `crypto-sharecrate/src/crypto/identity.rs` | 58 | 64 bits of SHA-256(public key). Fine for a person comparing it |
| `crypto-sharecrate/src/crypto/kdf.rs` | 1 | turns the X25519 shared secret into session keys. |
| `crypto-sharecrate/src/crypto/kdf.rs` | 17 | SHA-256 of the signed bytes of HELLO and HELLO-ACK. It is the |
| `crypto-sharecrate/src/crypto/kdf.rs` | 28 | HKDF-SHA256 (RFC 5869). |
| `crypto-sharecrate/src/crypto/mod.rs` | 1 | everything in this module is cryptography. |
| `crypto-sharecrate/src/crypto/pairing.rs` | 1 | pairing (exchanging long-term public keys once). |
| `crypto-sharecrate/src/crypto/pairing.rs` | 90 | the signature includes own_pub, so an accept that was |
| `crypto-sharecrate/src/crypto/responder.rs` | 1 | responder-side session bookkeeping for one peer (the hub |
| `crypto-sharecrate/src/crypto/responder.rs` | 86 | a HELLO nonce is 16 random bytes from the node, so a |
| `crypto-sharecrate/src/crypto/responder.rs` | 133 | key confirmation passed. This session becomes |
| `crypto-sharecrate/src/crypto/session.rs` | 1 | an established session. Seals and opens frames with |
| `crypto-sharecrate/src/crypto/session.rs` | 13 | rekey after this many frames in either direction. |
| `crypto-sharecrate/src/crypto/session.rs` | 16 | rekey after this many seconds, even if few frames were sent. |
| `crypto-sharecrate/src/crypto/session.rs` | 48 | wipe the keys when the session is replaced or dropped. |
| `crypto-sharecrate/src/crypto/session.rs` | 54 | the 96-bit ChaCha20 nonce is |
| `crypto-sharecrate/src/crypto/session.rs` | 113 | the "1 hour or 10 transmissions" rule. The initiator checks |
| `crypto-sharecrate/src/crypto/session.rs` | 129 | the 12-byte header is the AEAD associated data, so the |
| `crypto-sharecrate/src/crypto/session.rs` | 166 | bump the counter only after a successful seal, so a |
| `crypto-sharecrate/src/crypto/session.rs` | 175 | the order matters. Cheap header checks come first, then |
| `crypto-sharecrate/src/crypto/session.rs` | 197 | refuse anything from an expired session or past the |
| `crypto-sharecrate/src/crypto/session.rs` | 202 | replay protection. The counter must be strictly newer |
| `crypto-sharecrate/src/crypto/session.rs` | 226 | don't leave unauthenticated plaintext in the caller's buffer. |
| `crypto-sharecrate/src/lib.rs` | 13 | ` (see docs/CRYPTO_REVIEW.md). |
| `crypto-sharecrate/src/node.rs` | 27 | the pinned responder (hub/PC) Ed25519 key. |
| `crypto-sharecrate/src/node.rs` | 47 | if set, pairing only accepts this responder key. |
| `crypto-sharecrate/src/node.rs` | 56 | `seed` is this device's long-term Ed25519 secret, loaded |
| `crypto-sharecrate/src/node.rs` | 69 | pin the responder's public key ahead of time (built into |
| `crypto-sharecrate/src/node.rs` | 127 | true when there's no session, or the current one hit |
| `crypto-sharecrate/src/node.rs` | 179 | with a pinned key, an accept from anyone else |
| `crypto-sharecrate/src/node.rs` | 196 | a reply that fails verification (forged, or for |
| `crypto-sharecrate/src/node.rs` | 202 | the old session (and its keys) is dropped here. |
| `crypto-sharecrate/tests/protocol.rs` | 2 | these tests are the spec for the security properties. |
| `data-node/src/config.rs` | 38 | the hub's 64-hex-char public key (`hub-node fingerprint`). |
| `data-node/src/main.rs` | 259 | new ECDH session every 10 messages / 1 hour ---- |
| `data-node/src/main.rs` | 261 | ephemeral key + nonce |
| `data-node/src/main.rs` | 355 | true-random 256-bit seed from the RNG with ADC noise as |
| `data-node/src/main.rs` | 371 | first boot, so make this node's long-term identity. |
| `data-node/src/main.rs` | 382 | with FFA_HUB_KEY set at build time, the node only pairs |
| `data-node/src/rng.rs` | 1 | random numbers for keys and handshakes on the ESP32-C3. |
| `data-node/src/storage.rs` | 10 | the Ed25519 seed is stored *unencrypted* in flash. Anyone |
| `data-node/src/storage.rs` | 25 | long-term Ed25519 secret seed. |
| `database/schema.sql` | 10 | pinned Ed25519 key (hex) |
| `database/schema.sql` | 20 | pinned Ed25519 key (hex) |
| `database/schema.sql` | 32 | Ed25519 key (hex) the hub pins |
| `hub-node/Cargo.toml` | 26 | seed phrase (BIP39) and key derivation for the hub identity. |
| `hub-node/src/config.rs` | 21 | the PC daemon's public key (64 hex chars), shown in the |
| `hub-node/src/hub.rs` | 64 | pinned Ed25519 key from pairing. |
| `hub-node/src/hub.rs` | 160 | keys a person approved in the app, waiting for the |
| `hub-node/src/hub.rs` | 186 | randomness for one handshake: 48 bytes (ephemeral key + |
| `hub-node/src/hub.rs` | 393 | HELLO from a paired node or phone. The signature is |
| `hub-node/src/hub.rs` | 447 | hub side of pairing for nodes and phones; see |
| `hub-node/src/hub.rs` | 661 | never silently re-point an id at a new |
| `hub-node/src/hub/app.rs` | 164 | same AEAD + replay checks as the PC uplink; |
| `hub-node/src/hub/app.rs` | 314 | a phone can only approve a node that is |
| `hub-node/src/main.rs` | 78 | the phrase is printed once and never saved. The |
| `hub-node/src/pc_link.rs` | 29 | pinned PC daemon Ed25519 key. |
| `hub-node/src/pc_link.rs` | 53 | `pc_key` is the PC daemon's public key from hub.toml. The |
| `hub-node/src/pc_link.rs` | 82 | randomness for a handshake, from the OS RNG. |
| `hub-node/src/seed.rs` | 1 | hub identity from a 24-word BIP39 seed phrase. |
| `mobile/app/src/hub/context.tsx` | 9 | "seed" is this phone's long-term Ed25519 secret. |
| `mobile/app/src/hub/native.ts` | 4 | all pairing, handshakes and encryption happen inside the |
| `mobile/core/src/lib.rs` | 64 | the hub's Ed25519 key (hex), pinned from now on. |
| `mobile/core/src/lib.rs` | 76 | 32 bytes from the phone's OS CSPRNG. The seed passes |
| `mobile/core/src/lib.rs` | 125 | `expected_hub_key` (hex), if given, is the only hub |
| `mobile/core/src/lib.rs` | 238 | sealed under the current session; this counts |
| `mobile/core/src/lib.rs` | 273 | runs the signed X25519 handshake when there's no |
| `mobile/core/src/lib.rs` | 332 | AEAD + replay check. Anything that fails is dropped. |
| `node-sim/src/main.rs` | 46 | the node's Ed25519 seed (on the ESP this lives in flash). |
| `node-sim/src/main.rs` | 84 | new identity from the OS RNG, like the ESP's first boot. |
| `node-sim/src/main.rs` | 128 | pinned hub key, same as FFA_HUB_KEY on the ESP32. |
| `node-sim/src/main.rs` | 162 | handshake randomness |
| `pc-daemon/src/api/mod.rs` | 117 | the human approval step of pairing. A device's key is only |
| `pc-daemon/src/api/mod.rs` | 127 | only keys that are in the pending list (they sent a signed |
| `pc-daemon/src/auth.rs` | 18 | 256-bit random token from the OS RNG, stored with mode 0600. |
| `pc-daemon/src/auth.rs` | 33 | constant-time compare, so response timing doesn't leak how |
| `pc-daemon/src/db.rs` | 47 | the pinned hub key, if a hub is paired. |
| `pc-daemon/src/hub_link.rs` | 145 | 48 random bytes for one handshake (ephemeral key + nonce) |
| `pc-daemon/src/hub_link.rs` | 209 | verifies the hub's signature with the pinned key. |
| `pc-daemon/src/hub_link.rs` | 289 | hub pairing. A hub is only pinned after a person approved |
| `pc-daemon/src/main.rs` | 62 | the PC's long-term Ed25519 identity (the hub pins it at pairing). |
| `pc-daemon/src/state.rs` | 45 | the PC daemon's long-term signing key. |
| `pc-daemon/src/state.rs` | 50 | hub key a person approved in the app; it's pinned once |
| `uplink/src/lib.rs` | 226 | seal a JSON value as an Uplink frame under the session. |
| `uplink/src/lib.rs` | 237 | open an Uplink frame. `open` is either `Session::open` or |
| `uplink/src/lib.rs` | 284 | used for the PC daemon's identity seed. The OS RNG fills |
| `uplink/src/lib.rs` | 317 | OS CSPRNG (getrandom / /dev/urandom / BCryptGenRandom). |

_98 tagged locations._

<!-- END CRYPTO-REVIEW INDEX -->
