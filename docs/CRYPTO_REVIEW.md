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
- Labels (`FFA-HELLO-v1`, `FFA-HUB`, `FFA-DRBG`…) and flash magic `FFA1` keep the old name on purpose: renaming them changes every derived key, old seed phrases and pairings stop working

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
  - `FARMLAB_HUB_KEY` on nodes → only that hub (`NodeCore::with_expected_peer`, `WrongPeer`)
  - closes the trust-on-first-use race
  - hub requires `pc_key_id` (the PC's v2 KeyId) in hub.toml
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
- Phone side in Rust (uniffi from React Native), uses `LinkCore` (v2) from crypto-sharecrate
- v2 hybrid: PAIR_REQUEST2, HELLO2 (Ed25519 + ML-DSA-65, X25519 + ML-KEM-768), HKDF, 1000 msg / 1 h rekey
- Transport: TCP 7302, `u16 length | frame` like hub→PC (handshake frames are ~4.6 KB, too big for one UDP datagram)
  - `hub-node/src/app_tcp.rs`: max 16 connections (oldest closed when full), 120 s idle timeout, `MAX_TCP_FRAME` cap
  - only v2 frames accepted on the phone port; v1 PAIR_REQUEST / HELLO → dropped (no downgrade)
- JavaScript only sees JSON
- Frames: one `FrameType::Uplink` per request / reply (same sealing as hub→PC)
  - max 4096 bytes JSON per request / reply
  - one request → exactly one reply
  - phone counts the hub's replies → rekeys before the hub runs out
- Separate trust lists: `phones` (full hybrid keys) / `approved_phones` (KeyIds) in `paired.json`, phone port only
  - node key can't open a session on the phone port, phone key can't on the node port
  - phones paired before v2 are dropped on load and must pair again
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
- Without a pinned hub key → trust-on-first-use (Settings has optional hub key ID → `WrongPeer` otherwise; Phase 3's QR code always pins it)
- Phone seed: OS RNG in Rust → JS once → `expo-secure-store`, `WHEN_UNLOCKED_THIS_DEVICE_ONLY` (Keychain / Keystore, not backed up)
  - sits in JS memory while the app runs
- Rate limits: shares hub's 50/s signature budget; 5 verified HELLOs / 10 s per phone; requests need a valid session

## v2 hybrid post-quantum protocol (`crypto/v2/`, feature `pq`)
This is for the hub <-> PC and hub <-> phone links only. The ESP32 nodes stay on v1, because their 64/128-byte frames can't carry post-quantum keys. Both links use it (hub `pc_link.rs` / `hub/app.rs`, `pc-daemon/src/hub_link.rs`, `mobile/core`); everything except the node firmware builds crypto-sharecrate with `pq`.

- **Identifiers:** pending pairing lists, approvals (`PcMsg::ApprovePair`), the daemon's `phones` table and `hub.toml` (`pc_key_id`) use the 32-byte KeyId. Full hybrid keys are kept only where signatures are verified: the hub's `paired.json` phones, the daemon's `hubs` row, `pc_pairing.json`, and the phone's saved pairing. A v1 entry in any of these doesn't load, so that peer pairs again.
- **Hub keys:** one seed, two identities. v1 Ed25519 (`Identity::from_seed`) for nodes, v2 hybrid (HKDF-derived, so independent) for the PC and phones. `hub-node fingerprint` prints both.

- **Identity** (`v2/identity.rs`):
  - One 32-byte seed. HKDF-SHA256 (salt `FFA-v2-identity`) derives an Ed25519 seed (`FFA-v2 ed25519`) and an ML-DSA-65 seed ξ (`FFA-v2 ml-dsa-65`). The hub's BIP39 phrase therefore restores both keys.
  - Public key = Ed25519 (32) ‖ ML-DSA-65 (1952) = 1984 bytes.
  - `KeyId` = SHA-256(`FFA-v2-KEYID` ‖ public key). The fingerprint is the first 8 bytes of the KeyId.
- **Signatures:**
  - Both schemes sign d = SHA-512(`FFA-v2-SIG` ‖ len ‖ label ‖ (len32 ‖ part)…).
  - Ed25519(d) ‖ ML-DSA-65 deterministic (d, ctx = label) = 3373 bytes.
  - `verify` checks **both** (`verify_strict` for Ed25519) and fails if either fails. Both are always evaluated.
  - Signing the 64-byte digest keeps the crate heap-free; SHA-512's collision resistance (256-bit) is above ML-DSA-65's target.
- **Handshake** (`v2/handshake.rs`):
  - HELLO2 (4608 B) carries an X25519 ephemeral key and an ML-KEM-768 encapsulation key. HELLO-ACK2 (4516 B) carries an X25519 key and the ML-KEM ciphertext. Both are hybrid-signed, and the ACK signature covers the whole HELLO2.
  - Keys = HKDF-SHA256(ikm = X25519 secret ‖ ML-KEM secret, salt = SHA-256(`FFA-TRANSCRIPT-v2` ‖ KeyId_I ‖ KeyId_R ‖ HELLO2 ‖ ACK2)).
  - Confidentiality holds while either X25519 or ML-KEM holds. Both parties' KeyIds in the salt bind the keys to the identities.
  - The responder verifies the signature before any X25519 or ML-KEM work. The peer's encapsulation key goes through FIPS 203's modulus check (`EncapsulationKey::new`).
  - The all-zero X25519 result is rejected, as in v1.
- **Randomness:**
  - The crate stays sans-IO: the caller passes 112 random bytes per handshake side.
  - ML-KEM uses the deterministic keygen and encapsulation entry points, which are safe **only** if those bytes are fresh from a CSPRNG every time.
- **Responder** (`PeerSessions::on_hello_v2`): the same nonce replay list and pending-session key confirmation as v1. The bookkeeping is shared code (`admit`).
- **Sessions:** the same ChaCha20-Poly1305 `Session`, but v2 sessions rekey after `V2_MAX_MSGS` = 1000 frames per direction or 1 hour. v1 stays at 10.
- **Pairing** (`v2/pairing.rs`):
  - PAIR_REQUEST2 carries the hybrid key and an optional QR-ticket proof: HMAC-SHA256(token, `FFA-v2-TICKET` ‖ device key ‖ hub KeyId). The proof is inside the signed bytes, so it can't be lifted onto another device's request. It also names the hub, so it can't be relayed to another hub.
  - `verify_ticket` is constant-time. Burning the token after one use and expiring it are the caller's job (Phase 3).
  - PAIR_ACCEPT2's signature covers the device's KeyId.
- **Initiator** (`v2/link.rs`, `LinkCore`):
  - The v2 counterpart of `NodeCore`. A pinned responder KeyId (from the QR code or the SPAKE2 pairing) refuses any other responder with `WrongPeer`.
  - A forged ACK leaves the handshake pending.
  - v1 handshake and pairing frames on a v2 link are rejected with `BadFrame`.
- **No downgrade:** v2 frames use type bytes 0x10–0x13. The v1 parsers reject them and the v2 parsers reject v1 frames.
- **Libraries:** RustCrypto `ml-dsa` 0.1.1 and `ml-kem` 0.3.3, pinned exactly, `no_std`, with the `zeroize` feature on. `hmac` 0.12. All are optional, behind `pq`, so the firmware never builds them.

## Known weaknesses
- Phone port (TCP 7302) closes the oldest connection when 16 are open, so a device on the LAN can keep evicting real phones (denial of service only: no keys or data exposed; the phone reconnects on its next request)
- Node pairing without `FARMLAB_HUB_KEY` = trust-on-first-use (60 s window); hub side always needs approval
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
- 03/17 node TOFU race → optional `FARMLAB_HUB_KEY` pin
- 04 unlimited TCP connections → 4-connection cap, first-frame + idle timeouts
- 09 HELLO / PAIR CPU flood → per-node + global signature budgets
- 10/18 "delivered" without proof → node sends encrypted `Ack`; queued→sent→acked
- 11/15 init said nothing stored → output + docs corrected
- 12 hub re-pairs with any PC → `pc_public_key` required
- 14 any TCP disconnect cleared hub link → only the registered connection can
- 16/22/25/31 node id reuse / collisions → ids never reused, hub won't re-point an id (PC rules were removed; automation is hub recipes)
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
- `cargo test -p crypto-sharecrate --features pq` (v2): `either_signature_half_failing_rejects`, `hybrid_sign_verify`, `tampered_hello_rejected`, `tampered_ack_rejected_and_handshake_stays_pending`, `ack_from_wrong_responder_rejected`, `v1_and_v2_frames_do_not_mix`, `replayed_hello_rejected`, `pending_session_needs_key_confirmation`, `pinned_key_id_mismatch_is_wrong_peer`, `accept_for_another_device_rejected`, `ticket_checks`, `saved_pairing_for_other_key_dropped_when_pinned`, `v2_sessions_use_v2_message_limit`, `full_flow_pair_handshake_traffic`, `large_uplink_payload`, `small_order_x25519_rejected`
- `cargo test -p hub-node`:
  - `hello_flood_is_rate_limited`, `forged_hellos_do_not_use_up_a_nodes_allowance`
  - `unpair_clears_waiting_approval`, `pair_request_flood_is_rate_limited_and_never_stored`, `approval_for_taken_id_is_refused`
  - phone port: `node_frames_on_the_phone_port_and_unknown_phones_are_dropped`, `phone_approves_only_nodes_that_are_asking`, `many_requests_rekey_and_keep_working`, `recipes_with_bad_targets_are_refused`
  - `recipes::tests::rejects_bad_shapes`
- `cargo test -p farmlab-mobile-core`: `a_pinned_hub_key_refuses_any_other_hub`, `pairs_then_keeps_working_across_rekeys`, `a_lost_reply_is_retried`
- `cargo test -p pc-daemon`: `phones_pair_with_their_own_ids_and_can_only_approve_nodes`

## Every CRYPTO-REVIEW location

<!-- BEGIN CRYPTO-REVIEW INDEX (generated by scripts/crypto-review.sh, do not edit) -->

| File | Line | Note |
|------|------|------|
| `crypto-sharecrate/Cargo.toml` | 18 | these are the only crypto dependencies in the whole system. |
| `crypto-sharecrate/Cargo.toml` | 26 | v2 hybrid (feature "pq") only. |
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
| `crypto-sharecrate/src/crypto/kdf.rs` | 46 | (v2): HKDF salt for the hybrid handshake. It covers both |
| `crypto-sharecrate/src/crypto/kdf.rs` | 67 | (v2): HKDF-SHA256 with |
| `crypto-sharecrate/src/crypto/mod.rs` | 1 | everything in this module is cryptography. |
| `crypto-sharecrate/src/crypto/pairing.rs` | 1 | pairing (exchanging long-term public keys once). |
| `crypto-sharecrate/src/crypto/pairing.rs` | 90 | the signature includes own_pub, so an accept that was |
| `crypto-sharecrate/src/crypto/responder.rs` | 1 | responder-side session bookkeeping for one peer (the hub |
| `crypto-sharecrate/src/crypto/responder.rs` | 116 | a HELLO nonce is 16 random bytes from the node, so a |
| `crypto-sharecrate/src/crypto/responder.rs` | 164 | key confirmation passed. This session becomes |
| `crypto-sharecrate/src/crypto/session.rs` | 1 | an established session. Seals and opens frames with |
| `crypto-sharecrate/src/crypto/session.rs` | 13 | rekey after this many frames in either direction (v1 radio |
| `crypto-sharecrate/src/crypto/session.rs` | 16 | the same limit for v2 hybrid links (hub <-> PC, hub <-> phone). |
| `crypto-sharecrate/src/crypto/session.rs` | 20 | rekey after this many seconds, even if few frames were sent. |
| `crypto-sharecrate/src/crypto/session.rs` | 54 | wipe the keys when the session is replaced or dropped. |
| `crypto-sharecrate/src/crypto/session.rs` | 60 | the 96-bit ChaCha20 nonce is |
| `crypto-sharecrate/src/crypto/session.rs` | 127 | the "1 hour or N transmissions" rule (N = 10 on radio links). The initiator checks |
| `crypto-sharecrate/src/crypto/session.rs` | 145 | the 12-byte header is the AEAD associated data, so the |
| `crypto-sharecrate/src/crypto/session.rs` | 182 | bump the counter only after a successful seal, so a |
| `crypto-sharecrate/src/crypto/session.rs` | 191 | the order matters. Cheap header checks come first, then |
| `crypto-sharecrate/src/crypto/session.rs` | 213 | refuse anything from an expired session or past the |
| `crypto-sharecrate/src/crypto/session.rs` | 218 | replay protection. The counter must be strictly newer |
| `crypto-sharecrate/src/crypto/session.rs` | 242 | don't leave unauthenticated plaintext in the caller's buffer. |
| `crypto-sharecrate/src/crypto/v2/handshake.rs` | 1 | the v2 hybrid handshake (X25519 + ML-KEM-768, hybrid-signed). |
| `crypto-sharecrate/src/crypto/v2/handshake.rs` | 71 | MUST come from a CSPRNG and must never be reused. ML-KEM's |
| `crypto-sharecrate/src/crypto/v2/handshake.rs` | 80 | both ephemeral secrets zeroize on drop. |
| `crypto-sharecrate/src/crypto/v2/handshake.rs` | 161 | verify the hybrid signature before any X25519 or ML-KEM work. |
| `crypto-sharecrate/src/crypto/v2/handshake.rs` | 169 | EncapsulationKey::new runs FIPS 203's modulus check on |
| `crypto-sharecrate/src/crypto/v2/handshake.rs` | 241 | the signature covers OUR hello2, the ML-KEM |
| `crypto-sharecrate/src/crypto/v2/identity.rs` | 1 | hybrid long-term identity: Ed25519 + ML-DSA-65 (FIPS 204). |
| `crypto-sharecrate/src/crypto/v2/identity.rs` | 54 | both inner keys zeroize on drop (ed25519-dalek and ml-dsa |
| `crypto-sharecrate/src/crypto/v2/identity.rs` | 63 | `seed` must come from a CSPRNG, or from the BIP39 phrase on the hub. |
| `crypto-sharecrate/src/crypto/v2/identity.rs` | 96 | hybrid signature over the length-prefixed parts (see module docs). |
| `crypto-sharecrate/src/crypto/v2/identity.rs` | 112 | the message both schemes sign. Each part is length-prefixed. |
| `crypto-sharecrate/src/crypto/v2/identity.rs` | 125 | succeeds only if **both** the Ed25519 signature |
| `crypto-sharecrate/src/crypto/v2/identity.rs` | 158 | the identifier QR codes, config files and pairing pin. |
| `crypto-sharecrate/src/crypto/v2/link.rs` | 31 | the pinned responder hybrid key. |
| `crypto-sharecrate/src/crypto/v2/link.rs` | 60 | if set, pairing only accepts a responder with this KeyId |
| `crypto-sharecrate/src/crypto/v2/link.rs` | 70 | `seed` is this device's long-term secret (Keychain or |
| `crypto-sharecrate/src/crypto/v2/link.rs` | 83 | pin the responder ahead of time. A saved pairing for a |
| `crypto-sharecrate/src/crypto/v2/link.rs` | 144 | true when there's no session, or the current one hit |
| `crypto-sharecrate/src/crypto/v2/link.rs` | 184 | with a pinned KeyId, an accept from anyone |
| `crypto-sharecrate/src/crypto/v2/link.rs` | 204 | a reply that fails verification leaves the |
| `crypto-sharecrate/src/crypto/v2/link.rs` | 213 | v1 handshake and pairing frames are never accepted |
| `crypto-sharecrate/src/crypto/v2/mod.rs` | 1 | the v2 hybrid post-quantum protocol (feature `pq`). |
| `crypto-sharecrate/src/crypto/v2/pairing.rs` | 1 | v2 pairing (exchanging long-term hybrid public keys once). |
| `crypto-sharecrate/src/crypto/v2/pairing.rs` | 68 | HMAC-SHA256 keyed by the one-time token. |
| `crypto-sharecrate/src/crypto/v2/pairing.rs` | 127 | constant-time check that `req` proves knowledge of `token` |
| `crypto-sharecrate/src/crypto/v2/pairing.rs` | 168 | the signature covers our KeyId, so an accept made for |
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
| `crypto-sharecrate/tests/protocol_v2.rs` | 4 | these tests are the spec for the v2 security properties. |
| `crypto-sharecrate/tests/protocol_v2.rs` | 123 | AND-composition. Breaking either half breaks the signature. |
| `crypto-sharecrate/tests/protocol_v2.rs` | 291 | no downgrade in either direction. |
| `crypto-sharecrate/tests/protocol_v2.rs` | 345 | a phone that pinned the hub's KeyId from the QR code |
| `data-node/src/config.rs` | 38 | the hub's 64-hex-char public key (`hub-node fingerprint`). |
| `data-node/src/main.rs` | 259 | new ECDH session every 10 messages / 1 hour ---- |
| `data-node/src/main.rs` | 261 | ephemeral key + nonce |
| `data-node/src/main.rs` | 355 | true-random 256-bit seed from the RNG with ADC noise as |
| `data-node/src/main.rs` | 371 | first boot, so make this node's long-term identity. |
| `data-node/src/main.rs` | 382 | with FARMLAB_HUB_KEY set at build time, the node only pairs |
| `data-node/src/rng.rs` | 1 | random numbers for keys and handshakes on the ESP32-C3. |
| `data-node/src/storage.rs` | 10 | the Ed25519 seed is stored *unencrypted* in flash. Anyone |
| `data-node/src/storage.rs` | 25 | long-term Ed25519 secret seed. |
| `database/schema.sql` | 10 | pinned Ed25519 key (hex) |
| `database/schema.sql` | 20 | pinned Ed25519 key (hex) |
| `database/schema.sql` | 32 | Ed25519 key (hex) the hub pins |
| `hub-node/Cargo.toml` | 26 | seed phrase (BIP39) and key derivation for the hub identity. |
| `hub-node/src/config.rs` | 21 | the PC daemon's key ID (64 hex chars: SHA-256 of its |
| `hub-node/src/hub.rs` | 65 | pinned Ed25519 key from pairing. |
| `hub-node/src/hub.rs` | 88 | pinned Ed25519 + ML-DSA-65 key from pairing. |
| `hub-node/src/hub.rs` | 181 | v1 Ed25519 identity, for the nodes' radio links. |
| `hub-node/src/hub.rs` | 183 | v2 hybrid identity from the same seed, for phones and |
| `hub-node/src/hub.rs` | 188 | keys a person approved in the app, waiting for the |
| `hub-node/src/hub.rs` | 215 | randomness for one handshake: 48 bytes (ephemeral key + |
| `hub-node/src/hub.rs` | 439 | HELLO from a paired node. The signature is checked with |
| `hub-node/src/hub.rs` | 479 | hub side of pairing for nodes; see crypto::pairing for |
| `hub-node/src/hub.rs` | 709 | never silently re-point an id at a new |
| `hub-node/src/hub/app.rs` | 156 | only v2 frames are accepted on the phone port. v1 |
| `hub-node/src/hub/app.rs` | 175 | same AEAD + replay checks as the PC uplink; |
| `hub-node/src/hub/app.rs` | 207 | v2 pairing for phones. The phone's hybrid key is only |
| `hub-node/src/hub/app.rs` | 255 | HELLO2 from a paired phone, checked against its pinned |
| `hub-node/src/hub/app.rs` | 423 | a phone can only approve a node that is |
| `hub-node/src/main.rs` | 79 | the phrase is printed once and never saved. The |
| `hub-node/src/pc_link.rs` | 31 | the pinned PC daemon hybrid key (Ed25519 + ML-DSA-65, hex). |
| `hub-node/src/pc_link.rs` | 62 | `pc_key` is the PC daemon's KeyId from hub.toml. The hub only |
| `hub-node/src/seed.rs` | 1 | hub identity from a 24-word BIP39 seed phrase. |
| `mobile/app/src/hub/context.tsx` | 9 | "seed" is this phone's long-term Ed25519 secret. |
| `mobile/app/src/hub/native.ts` | 4 | all pairing, handshakes and encryption happen inside the |
| `mobile/core/src/lib.rs` | 70 | the hub's hybrid key (Ed25519 + ML-DSA-65, 3968 hex |
| `mobile/core/src/lib.rs` | 100 | 32 bytes from the phone's OS CSPRNG. The seed passes |
| `mobile/core/src/lib.rs` | 150 | `expected_hub_key` (the hub's key ID or full key, hex), |
| `mobile/core/src/lib.rs` | 259 | sealed under the current session; this counts |
| `mobile/core/src/lib.rs` | 329 | runs the v2 hybrid handshake when there's no session or |
| `mobile/core/src/lib.rs` | 385 | AEAD + replay check. Anything that fails is dropped. |
| `node-sim/src/main.rs` | 46 | the node's Ed25519 seed (on the ESP this lives in flash). |
| `node-sim/src/main.rs` | 84 | new identity from the OS RNG, like the ESP's first boot. |
| `node-sim/src/main.rs` | 128 | pinned hub key, same as FARMLAB_HUB_KEY on the ESP32. |
| `node-sim/src/main.rs` | 162 | handshake randomness |
| `pc-daemon/src/api/mod.rs` | 123 | the human approval step of pairing. A device's key is only |
| `pc-daemon/src/api/mod.rs` | 133 | only keys that are in the pending list (they sent a signed |
| `pc-daemon/src/auth.rs` | 18 | 256-bit random token from the OS RNG, stored with mode 0600. |
| `pc-daemon/src/auth.rs` | 33 | constant-time compare, so response timing doesn't leak how |
| `pc-daemon/src/db.rs` | 47 | the pinned hub hybrid key (Ed25519 + ML-DSA-65), if a hub is |
| `pc-daemon/src/hub_link.rs` | 148 | random bytes for one v2 handshake plus 4 for the epoch id, |
| `pc-daemon/src/hub_link.rs` | 187 | only v2 frames on the hub port. A v1 |
| `pc-daemon/src/hub_link.rs` | 212 | verifies the hub's hybrid signature with the pinned key. |
| `pc-daemon/src/hub_link.rs` | 291 | hub pairing (v2). A hub is only pinned after a person |
| `pc-daemon/src/main.rs` | 61 | the PC's long-term identity seed. It derives the hybrid |
| `pc-daemon/src/state.rs` | 49 | the PC daemon's long-term hybrid signing key |
| `pc-daemon/src/state.rs` | 55 | KeyId of the hub a person approved in the app; its key |
| `uplink/src/lib.rs` | 271 | seal a JSON value as an Uplink frame under the session. |
| `uplink/src/lib.rs` | 282 | open an Uplink frame. `open` is either `Session::open` or |
| `uplink/src/lib.rs` | 347 | randomness for one side of a v2 handshake, from the OS RNG. |
| `uplink/src/lib.rs` | 375 | used for the PC daemon's identity seed. The OS RNG fills |
| `uplink/src/lib.rs` | 408 | OS CSPRNG (getrandom / /dev/urandom / BCryptGenRandom). |

_139 tagged locations._

<!-- END CRYPTO-REVIEW INDEX -->
