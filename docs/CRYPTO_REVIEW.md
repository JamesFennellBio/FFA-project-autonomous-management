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
- Hub pins PC key P; PC pins H; both after the one-time code exchange (`uplink/src/pake.rs`); the hub plays the "node" role toward the PC
- Phone (key F) pins H from the QR code (or a typed key ID); hub pins F on a valid QR ticket, or after Approve on the desktop; phone never talks to the PC
- Desktop app → daemon: loopback HTTP, 256-bit bearer token
- Token stays in the app's Rust side (`app/src-tauri/src/daemon.rs`), the page never sees it

## Pairing (`crypto/pairing.rs`)
- Node holds BOOT 3 s → `PAIR_REQUEST = 0x08 | pubA | Sig_A("FFA-PAIR-REQ-v1" ‖ 0x08 ‖ pubA)` (proves key ownership)
- Hub forwards to PC; app shows fingerprint = first 64 bits of SHA-256(pubA)
- Person compares with node serial log → Approve
- Next request: `PAIR_ACCEPT = 0x09 | node_id | pubH | Sig_H("FFA-PAIR-ACC-v1" ‖ pubA ‖ 0x09 ‖ node_id ‖ pubH)`
- Signature covers pubA → accept for another device fails
- Pinned keys (findings 03/17/12):
  - `FARMLAB_HUB_KEY` on nodes → only that hub (`NodeCore::with_expected_peer`, `WrongPeer`)
  - closes the trust-on-first-use race
  - hub ↔ PC: pinned by the code exchange; `pc_key_id` in hub.toml is an optional extra pin
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
- Phone side in Rust (`mobile/core`, called from `app/src-tauri/src/phone.rs`), uses `LinkCore` (v2) from crypto-sharecrate
- v2 hybrid: PAIR_REQUEST2, HELLO2 (Ed25519 + ML-DSA-65, X25519 + ML-KEM-768), HKDF, 1000 msg / 1 h rekey
- Transport: TCP 7302, `u16 length | frame` like hub→PC (handshake frames are ~4.6 KB, too big for one UDP datagram)
  - `hub-node/src/app_tcp.rs`: max 16 connections (oldest closed when full), 120 s idle timeout, `MAX_TCP_FRAME` cap
  - only v2 frames accepted on the phone port; v1 PAIR_REQUEST / HELLO → dropped (no downgrade)
- The web view (WASM UI) only sees JSON
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
- Phone seed: OS RNG in Rust, saved by Rust in `phone.json` (0600, app data folder); never reaches the web view (see Known weaknesses)
  - sits in JS memory while the app runs
- Rate limits: shares hub's 50/s signature budget; 5 verified HELLOs / 10 s per phone; requests need a valid session

## Hub ↔ PC pairing code (`uplink/src/pake.rs`, `hub-node/src/pair_cmd.rs`)
- Proves the PC is talking to the right Pi (it's headless, so there's nothing to compare a fingerprint on except over SSH)
- `hub-node pair` opens TCP 7402 for 10 minutes, prints a 12-digit code (10^12 values, rejection-sampled from the OS RNG)
- SPAKE2 (`spake2` 0.4.0, Ed25519 group, pinned exactly), ids `farmlab-hub` / `farmlab-pc`, password = the 12 digits
- Key confirmation: HKDF-SHA256(K, salt = SHA-256(label ‖ msg_hub ‖ msg_pc)) → two MAC keys
  - PC proves first: HMAC over its hybrid public key, its hub-listen port, the hub id
  - hub answers only after that MAC checks out: its hybrid key + HMAC over both payloads
  - each side pins the other's key only after the other's MAC verifies (constant-time)
- Wrong code: one online guess per run; 3 wrong runs close the window; success closes it (single use); the code is never saved
- The hub learns the PC's address from the TCP peer IP + the PC's port in the MAC'd payload, so hub.toml needs no PC settings
- Optional pin: `pc_key_id` in hub.toml → a PC with another key is refused even with the right code
- The pinned keys are then proven by HELLO2 signatures, so a stolen public key is useless
- Not post-quantum: SPAKE2 is elliptic-curve. A future quantum attacker who recorded the run could recover K, but only the public keys travel under it, and those aren't secret; the session keys come from the hybrid HELLO2 later
- Daemon side: `POST /api/hubs/pair` (20 s overall timeout); the old "approve the hub's fingerprint" path is removed and the hub port only accepts HELLO2 / Uplink

## Phone QR tickets (`uplink/src/pairlink.rs`, `hub-node/src/hub.rs`, `hub/app.rs`)
- Desktop → `POST /api/phones/ticket`: daemon makes a 16-byte token (OS RNG), reserves a phone id (never reused), sends `PcMsg::PhoneTicket` over the encrypted uplink
- QR = `farmlab://pair?v=2&a=<hub ip>:7302&k=<hub KeyId>&t=<token>&e=<expiry>`; SVG drawn by the daemon
- Phone: `HubClient` is built with `k` as the expected peer, so the hub is pinned before anything is sent (`pair_with_qr` refuses a client made otherwise)
- PAIR_REQUEST2 carries HMAC(token, `FFA-v2-TICKET` ‖ phone key ‖ hub KeyId) inside the signed bytes
- Hub: tickets in memory only, at most 4 open, 5 min TTL (capped at 10), single use (removed on success), refused if the id is taken; checked with `verify_ticket` (constant time)
- Valid ticket → paired at once, `HubMsg::PhonePaired` tells the PC; wrong / used / expired ticket → falls back to the manual approval list
- Possession of the QR = permission to pair one phone for 5 minutes (a photo of the screen is enough). That's the point of it, but it's why it's short-lived and single use

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

## v2 app shell (`app/src-tauri`)
- Desktop: `daemon_api` reads the daemon's API token in Rust and adds it; the WASM page never sees it. Only `/api/...` paths and GET / POST / PATCH / PUT / DELETE go through; the URL is fixed to `127.0.0.1:7401` (env override for testing only)
- Phone: `hub_request` sends the page's JSON through `farmlab-mobile-core`; all pairing, handshakes and sealing happen in Rust
  - QR pairing: the client is made from the QR's hub address and KeyId before anything is sent (`HubClient::pair_with_qr` refuses any other client)
- CSP: `default-src 'self'`, scripts only from the app (plus `wasm-unsafe-eval`), no remote connections; Blockly and fonts are vendored, nothing loads from the network
- App commands take only data (JSON); none run code or open files the page names

- Anyone who sees an open phone QR code (over a shoulder, a screenshot) can pair one phone with it within 5 minutes; the desktop shows the new phone, and it can be unpaired there
- The hub pairing code is shown in the Pi's terminal; whoever reads it within its 10 minutes can pair a PC (`pc_key_id` pin prevents that)
- The QR token crosses the PC → hub uplink (encrypted) and is held in hub memory in the clear until used or expired
- Phone port (TCP 7302) closes the oldest connection when 16 are open, so a device on the LAN can keep evicting real phones (denial of service only: no keys or data exposed; the phone reconnects on its next request)
- Node pairing without `FARMLAB_HUB_KEY` = trust-on-first-use (60 s window); hub side always needs approval
- v2 phone app keeps its seed and pinned hub key in `phone.json` (0600) in the app's private data folder, not yet in the iOS Keychain / Android Keystore as planned; the OS app sandbox keeps other apps out, but a rooted / jailbroken phone or an unencrypted backup could read it
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
- `cargo test -p farmlab-mobile-core`: `a_pinned_hub_key_refuses_any_other_hub`, `pairs_then_keeps_working_across_rekeys`, `a_lost_reply_is_retried`, `a_qr_code_pairs_at_once_with_the_pinned_hub`, `a_wrong_qr_ticket_is_not_accepted`, `qr_pairing_needs_a_client_made_from_that_qr_code`
- `cargo test -p pc-daemon`: `phones_pair_with_their_own_ids_and_can_only_approve_nodes`, `hub_pairs_with_the_right_code_only`, `phone_qr_ticket_goes_to_the_hub_and_pins_its_key`
- `cargo test -p uplink`: `right_code_pairs_both_ways`, `wrong_code_fails_on_both_sides_and_reveals_no_hub_key`, `codes`, plus the QR payload tests in `pairlink`
- `cargo test -p hub-node`: `right_code_pairs_and_learns_the_pc_address`, `three_wrong_codes_close_the_window`, `a_pinned_pc_key_refuses_other_pcs`, `a_qr_ticket_pairs_a_phone_without_approval`, `bad_reused_expired_or_relayed_tickets_do_not_pair`, `a_ticket_for_a_used_phone_id_is_refused`

## Every CRYPTO-REVIEW location

<!-- BEGIN CRYPTO-REVIEW INDEX (generated by scripts/crypto-review.sh, do not edit) -->

| File | Line | Note |
|------|------|------|
| `app/src-tauri/src/phone.rs` | 4 | the phone's identity seed and its pinned hub key are kept |
| `app/src-tauri/src/phone.rs` | 23 | the phone's long-term secret (64 hex). |
| `app/src-tauri/src/phone.rs` | 29 | the hub's full hybrid key, pinned at pairing. |
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
| `hub-node/src/config.rs` | 27 | optional extra pin. The PC daemon's key ID (64 hex chars: |
| `hub-node/src/hub.rs` | 65 | pinned Ed25519 key from pairing. |
| `hub-node/src/hub.rs` | 88 | pinned Ed25519 + ML-DSA-65 key from pairing. |
| `hub-node/src/hub.rs` | 110 | HMAC key the phone must prove it knows (from the QR code). |
| `hub-node/src/hub.rs` | 194 | v1 Ed25519 identity, for the nodes' radio links. |
| `hub-node/src/hub.rs` | 196 | v2 hybrid identity from the same seed, for phones and |
| `hub-node/src/hub.rs` | 201 | keys a person approved in the app, waiting for the |
| `hub-node/src/hub.rs` | 207 | open QR tickets, oldest first. Single use. |
| `hub-node/src/hub.rs` | 230 | randomness for one handshake: 48 bytes (ephemeral key + |
| `hub-node/src/hub.rs` | 455 | HELLO from a paired node. The signature is checked with |
| `hub-node/src/hub.rs` | 495 | hub side of pairing for nodes; see crypto::pairing for |
| `hub-node/src/hub.rs` | 728 | capped list, oldest dropped; each |
| `hub-node/src/hub.rs` | 752 | never silently re-point an id at a new |
| `hub-node/src/hub/app.rs` | 156 | only v2 frames are accepted on the phone port. v1 |
| `hub-node/src/hub/app.rs` | 175 | same AEAD + replay checks as the PC uplink; |
| `hub-node/src/hub/app.rs` | 207 | v2 pairing for phones. The phone's hybrid key is pinned |
| `hub-node/src/hub/app.rs` | 234 | constant-time HMAC check against each open ticket. |
| `hub-node/src/hub/app.rs` | 290 | HELLO2 from a paired phone, checked against its pinned |
| `hub-node/src/hub/app.rs` | 458 | a phone can only approve a node that is |
| `hub-node/src/main.rs` | 87 | the phrase is printed once and never saved. The |
| `hub-node/src/pair_cmd.rs` | 33 | one success closes the window (the code is single use); |
| `hub-node/src/pc_link.rs` | 31 | the pinned PC daemon hybrid key (Ed25519 + ML-DSA-65, hex). |
| `hub-node/src/pc_link.rs` | 80 | the PC's key comes from pairing with the one-time code, and |
| `hub-node/src/seed.rs` | 1 | hub identity from a 24-word BIP39 seed phrase. |
| `mobile/core/src/lib.rs` | 67 | the hub's hybrid key (Ed25519 + ML-DSA-65, 3968 hex |
| `mobile/core/src/lib.rs` | 97 | 32 bytes from the phone's OS CSPRNG. It's the phone's |
| `mobile/core/src/lib.rs` | 123 | the hub's key ID (hex). Pass it to [`HubClient::new`] as |
| `mobile/core/src/lib.rs` | 166 | `expected_hub_key` (the hub's key ID or full key, hex), |
| `mobile/core/src/lib.rs` | 233 | make this client from the same QR code first |
| `mobile/core/src/lib.rs` | 338 | sealed under the current session; this counts |
| `mobile/core/src/lib.rs` | 408 | runs the v2 hybrid handshake when there's no session or |
| `mobile/core/src/lib.rs` | 464 | AEAD + replay check. Anything that fails is dropped. |
| `node-sim/src/main.rs` | 46 | the node's Ed25519 seed (on the ESP this lives in flash). |
| `node-sim/src/main.rs` | 84 | new identity from the OS RNG, like the ESP's first boot. |
| `node-sim/src/main.rs` | 128 | pinned hub key, same as FARMLAB_HUB_KEY on the ESP32. |
| `node-sim/src/main.rs` | 162 | handshake randomness |
| `pc-daemon/src/api/mod.rs` | 142 | first hub pairing. Runs the SPAKE2 exchange (uplink::pake) |
| `pc-daemon/src/api/mod.rs` | 190 | makes a one-time phone pairing QR code. The token goes to |
| `pc-daemon/src/api/mod.rs` | 240 | the human approval step of pairing. A device's key is only |
| `pc-daemon/src/api/mod.rs` | 250 | only keys that are in the pending list (they sent a signed |
| `pc-daemon/src/auth.rs` | 18 | 256-bit random token from the OS RNG, stored with mode 0600. |
| `pc-daemon/src/auth.rs` | 33 | constant-time compare, so response timing doesn't leak how |
| `pc-daemon/src/db.rs` | 47 | the pinned hub hybrid key (Ed25519 + ML-DSA-65), if a hub is |
| `pc-daemon/src/hub_link.rs` | 146 | random bytes for one v2 handshake plus 4 for the epoch id, |
| `pc-daemon/src/hub_link.rs` | 184 | only HELLO2 and Uplink on the hub port. The |
| `pc-daemon/src/hub_link.rs` | 199 | verifies the hub's hybrid signature with the pinned key. |
| `pc-daemon/src/hub_link.rs` | 323 | a phone that proved it scanned our QR ticket. |
| `pc-daemon/src/main.rs` | 61 | the PC's long-term identity seed. It derives the hybrid |
| `pc-daemon/src/state.rs` | 50 | the PC daemon's long-term hybrid signing key |
| `uplink/Cargo.toml` | 15 | SPAKE2 for the hub <-> PC pairing code (pake.rs). |
| `uplink/src/lib.rs` | 245 | `token` (hex, 16 bytes) is the HMAC key the phone proves |
| `uplink/src/lib.rs` | 290 | seal a JSON value as an Uplink frame under the session. |
| `uplink/src/lib.rs` | 301 | open an Uplink frame. `open` is either `Session::open` or |
| `uplink/src/lib.rs` | 366 | randomness for one side of a v2 handshake, from the OS RNG. |
| `uplink/src/lib.rs` | 394 | used for the PC daemon's identity seed. The OS RNG fills |
| `uplink/src/lib.rs` | 427 | OS CSPRNG (getrandom / /dev/urandom / BCryptGenRandom). |
| `uplink/src/pairlink.rs` | 7 | the phone pins `k` (the hub's KeyId), so it only ever pairs |
| `uplink/src/pake.rs` | 1 | first hub <-> PC pairing with a one-time code (SPAKE2). |
| `uplink/src/pake.rs` | 62 | uniform over 10^12 values (rejection sampling on OS randomness). |
| `uplink/src/pake.rs` | 108 | confirmation keys from the SPAKE2 key, bound to both messages. |
| `uplink/src/pake.rs` | 238 | constant-time check that the hub knew the code too. |
| `uplink/src/pake.rs` | 274 | constant-time check that the PC knew the code. A wrong |

_156 tagged locations._

<!-- END CRYPTO-REVIEW INDEX -->
