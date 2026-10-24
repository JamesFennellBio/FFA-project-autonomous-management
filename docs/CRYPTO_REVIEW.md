# Crypto review guide

Everything in this system that touches cryptography is tagged with a
`// CRYPTO-REVIEW:` comment. This page explains the design so you can check
it, lists the weaknesses I already know about, and ends with an index of every
tagged line (regenerate it with `./scripts/crypto-review.sh`).

**Where to start:** almost all the crypto is in one small no_std crate,
`crypto-sharecrate/src/crypto/` (about 600 lines), and the tests in
`crypto-sharecrate/tests/protocol.rs` spell out the security properties it's
supposed to have. The other crates only call into it, plus the RNG and key storage.

## Primitives

| Purpose | Algorithm | Crate |
|---|---|---|
| Long-term identity, signatures | Ed25519 (`verify_strict`) | `ed25519-dalek` 2 |
| Key agreement | X25519 ECDH, ephemeral on both sides | `x25519-dalek` 2 |
| Key derivation | HKDF-SHA256 | `hkdf` 0.12, `sha2` 0.10 |
| Encryption | ChaCha20-Poly1305 (IETF, 96-bit nonce) | `chacha20poly1305` 0.10 |
| Hub recovery | BIP39 24 words → PBKDF2-SHA512 → HKDF → Ed25519 seed | `bip39` 2 |
| Node RNG | SHA-256 hash-DRBG seeded from the C3's TRNG | `data-node/src/rng.rs` |

All of these are RustCrypto / dalek crates in pure Rust. Nothing is hand-rolled
except the protocol framing, the DRBG and the HKDF labels.

## Who trusts whom

```
          pairing (TOFU + a person approves in the app)
 ESP node ───────────────────────────────────────────────▶ Pi hub ──────────▶ PC daemon ──▶ Electron app
 Ed25519 key A        pins hub key H          pins A      Ed25519 key H  pins P  key P      127.0.0.1 + bearer token
```

- A **node** pins the hub's Ed25519 key during pairing. The **hub** pins the
  node's key after you click Approve.
- The **hub** pins the PC's key and the **PC** pins the hub's key the same way
  (the hub plays the "node" role toward the PC).
- The **app** talks to the daemon over loopback HTTP with a 256-bit bearer
  token. The token stays in the Electron main process and the page never sees it.

## Pairing (`crypto/pairing.rs`)

1. The node holds BOOT for 3 s and sends `PAIR_REQUEST = 0x08 | pubA | Sig_A("FFA-PAIR-REQ-v1" ‖ 0x08 ‖ pubA)`
   (proof that it holds the key).
2. The hub forwards the request to the PC, and the app shows its **fingerprint** (first 64 bits of SHA-256(pubA)).
3. You compare the fingerprint with the node's serial log and click Approve.
4. On the node's next request, the hub sends `PAIR_ACCEPT = 0x09 | node_id | pubH | Sig_H("FFA-PAIR-ACC-v1" ‖ pubA ‖ 0x09 ‖ node_id ‖ pubH)`.
   Because the signature covers pubA, an accept made for another device fails.
5. **Pinned peer keys (scan findings 03/17/12).** Nodes built with
   `FFA_HUB_KEY=<hub public key>` refuse any PAIR_ACCEPT not signed by that
   key (`NodeCore::with_expected_peer`, error `WrongPeer`), which closes the
   trust-on-first-use race. The hub *requires* `pc_public_key` in hub.toml
   and only pairs with that PC. A saved pairing for a different key is
   dropped, and the device pairs again with the pinned peer.

## Session handshake (`crypto/handshake.rs`)

```
HELLO      I→R  0x03 | id | eI(32) | nI(16) | Sig_I("FFA-HELLO-v1" ‖ bytes 0..51)
HELLO-ACK  R→I  0x04 | id | epoch(4) | eR(32) | nR(16) | Sig_R("FFA-HELLO-ACK-v1" ‖ HELLO[0..51] ‖ ACK[0..55])
ss   = X25519(eI, eR)          (rejected if not contributory)
salt = SHA-256("FFA-TRANSCRIPT-v1" ‖ HELLO[0..51] ‖ ACK[0..55])
k_IR = HKDF(ss, salt, "FFA-v1 key init->resp")
k_RI = HKDF(ss, salt, "FFA-v1 key resp->init")
```

This is a SIGMA-like signed Diffie-Hellman exchange (close to TLS 1.3's
signature-authenticated ECDHE):

- **Mutual authentication:** each side signs with its pinned identity key.
- **Forward secrecy:** ephemeral secrets are `StaticSecret`s that are zeroized
  on drop right after use.
- **Binding:** Sig_R covers the whole HELLO, so an ACK can't be replayed onto
  a different HELLO.
- **Key confirmation:** the responder keeps the new session *pending* until
  a frame under it decrypts (`crypto/responder.rs`), so a replayed or forged
  HELLO can't knock out the live session.

## Rekeying: every 10 transmissions or 1 hour (`crypto/session.rs`)

- `MAX_MSGS = 10`: counters run 0..=9 in each direction, and frame 10 is refused on both send and receive.
- `MAX_AGE_SECS = 3600`: senders stop at 1 h. Receivers allow 60 s of grace for frames still in flight.
- `needs_rekey()` counts **both** directions, so the initiator starts a fresh
  handshake once the responder has used up its sends.
- Ages use a monotonic clock (`embassy_time::Instant` on the node,
  `Instant` on the hub and PC), so an NTP change can't make a session look younger.

## Frames (`frame.rs`)

```
type(1) | node_id(2) | epoch(4) | counter(4) | dir(1) | ciphertext | tag(16)
\______________ AAD (authenticated, not encrypted) ________/
nonce = dir(1) | epoch(4) | counter(4) | 000000
```

Radio frames are always 64 bytes (the plaintext is padded to 36), so every
frame looks the same size on air. Replay protection: the counter must be
strictly greater than the last frame that verified, and gaps are allowed for
lost LoRa packets. Replay state only changes **after** the tag verifies.

## Known weaknesses / things to look at

1. **Node pairing without `FFA_HUB_KEY` is still trust-on-first-use.** If you
   build a node without the hub key, whoever answers first during the 60 s
   pairing window gets pinned. With the key set, this is closed. The hub side
   always needs a person to approve.
2. **Keys at rest aren't encrypted.** The node seed sits in plain flash at
   0x9000 (anyone holding the board can read it). The hub's `identity.key` and
   the PC's `identity.key` / `api-token` are 0600 files. `hub-node init` now
   says plainly that the derived key *is* saved. Fixing the node needs ESP32
   flash encryption plus secure boot (an eFuse change you can't undo).
3. **Custom DRBG on the node** (`data-node/src/rng.rs`). It's a simple
   SHA-256 construction, not NIST HMAC-DRBG. Its seed comes from the TRNG
   (ADC noise) at boot.
4. **Metadata is in the clear:** frame type, node id, epoch and counter (all
   authenticated).
5. **Radio denial of service can't be prevented** (jamming). What is limited
   now: at most 50 Ed25519 checks per second on the hub; per node, at most 5
   HELLOs per 10 s; at most 1 forwarded pairing request per second; pairing
   requests are never stored in the hub DB; and the rate-limit tables are
   capped. A replayed HELLO is rejected by nonce, and a replay can't push out
   the genuine pending session (4 slots). An attacker with many recorded
   HELLOs replaying fast can still delay new sessions, which is no worse than
   jamming.
6. **The hub→PC link is this custom protocol, not TLS.** The PC port allows
   4 connections, each with a 10 s first-frame timeout and a 90 s idle timeout.
7. **Timestamps come from the hub's clock** (no RTC on the Pi 3B). A clock
   that steps backwards after a power cut can make the PC skip a few readings
   as duplicates (scan finding 19).
8. **The Wi-Fi password is compiled into the node firmware.**
9. **There's no node key rotation.** Rotating means a factory reset (hold
   BOOT 10 s) plus pairing again.
10. **There's no revocation broadcast to the node.** Unpairing removes the key on the hub and
    PC (and now requires the hub to be online, so they can't get out of
    sync), but the node keeps the hub's key until someone holds its button for 10 s.

### Fixed after the Oct 24 2026 security scan

| Finding | Fix |
|---|---|
| 01 failsafe only re-armed by a new session | any authenticated frame re-arms it (`data-node/src/main.rs::contact`) |
| 02/08 pairing-request spam filled hub DB/RAM | live-only, never stored; global 1/s; key table capped at 1024 |
| 03/17 node TOFU race | optional `FFA_HUB_KEY` pin |
| 04 unlimited TCP connections | 4-connection cap, first-frame and idle timeouts |
| 09 HELLO/PAIR CPU flood | per-node and global signature-check budgets |
| 10/18 "delivered" without proof | node sends an encrypted `Ack`; status queued→sent→acked |
| 11/15 init said nothing is stored | output and docs corrected |
| 12 hub re-pairs with any PC | `pc_public_key` required in hub.toml |
| 14 any TCP disconnect cleared hub link | only the registered connection can clear it |
| 16/22/25/31 node id reuse / collisions | ids never reused; rules deleted with the node; hub refuses to re-point an id |
| 24 deleted active rule left output on | off command sent on delete |
| 27 `restore` overwrote identity | needs `--force` |
| 29 replayed HELLO blocked new sessions | nonce cache + 4 pending slots |
| 30 silent command drop | `CommandDropped` → alert + status `dropped` |

## Tests that pin down these properties

`cargo test -p crypto-sharecrate` runs these:
`rekey_after_ten_messages`, `rekey_after_one_hour`, `hub_rejects_expired_session`,
`tampered_ciphertext_header_and_tag_rejected`, `replay_rejected`,
`hello_signed_by_wrong_key_rejected`, `ack_from_wrong_hub_rejected`,
`replayed_hello_rejected_and_live_session_survives`, `old_replay_cannot_evict_genuine_pending_session`,
`pinned_hub_key_rejects_other_hubs`, `saved_pairing_with_wrong_key_is_dropped_when_pinned`,
`pair_accept_for_other_device_rejected`, `forged_pair_request_rejected`.
`cargo test -p hub-node` covers the hub-side limits (`hello_flood_is_rate_limited`,
`pair_request_flood_is_rate_limited_and_never_stored`, `approval_for_taken_id_is_refused`).

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
| `crypto-sharecrate/src/crypto/handshake.rs` | 194 | the signature covers OUR hello, so this ACK answers |
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
| `crypto-sharecrate/src/node.rs` | 198 | the old session (and its keys) is dropped here. |
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
| `hub-node/Cargo.toml` | 26 | seed phrase (BIP39) and key derivation for the hub identity. |
| `hub-node/src/config.rs` | 18 | the PC daemon's public key (64 hex chars), shown in the |
| `hub-node/src/hub.rs` | 51 | pinned Ed25519 key from pairing. |
| `hub-node/src/hub.rs` | 71 | keys a person approved in the app, waiting for the node's next PAIR_REQUEST. |
| `hub-node/src/hub.rs` | 91 | randomness for one handshake: 48 bytes (ephemeral key + |
| `hub-node/src/hub.rs` | 230 | verifies the node's signature with its pinned key; |
| `hub-node/src/hub.rs` | 271 | hub side of node pairing; see crypto::pairing for the trust model. |
| `hub-node/src/hub.rs` | 332 | never silently re-point a node id at a new |
| `hub-node/src/main.rs` | 75 | the phrase is printed once and never saved. The |
| `hub-node/src/pc_link.rs` | 29 | pinned PC daemon Ed25519 key. |
| `hub-node/src/pc_link.rs` | 53 | `pc_key` is the PC daemon's public key from hub.toml. The |
| `hub-node/src/pc_link.rs` | 82 | randomness for a handshake, from the OS RNG. |
| `hub-node/src/seed.rs` | 1 | hub identity from a 24-word BIP39 seed phrase. |
| `node-sim/src/main.rs` | 46 | the node's Ed25519 seed (on the ESP this lives in flash). |
| `node-sim/src/main.rs` | 84 | new identity from the OS RNG, like the ESP's first boot. |
| `node-sim/src/main.rs` | 128 | pinned hub key, same as FFA_HUB_KEY on the ESP32. |
| `node-sim/src/main.rs` | 162 | handshake randomness |
| `pc-daemon/src/api/mod.rs` | 106 | the human approval step of pairing. A device's key is only |
| `pc-daemon/src/auth.rs` | 18 | 256-bit random token from the OS RNG, stored with mode 0600. |
| `pc-daemon/src/auth.rs` | 33 | constant-time compare, so response timing doesn't leak how |
| `pc-daemon/src/db.rs` | 47 | the pinned hub key, if a hub is paired. |
| `pc-daemon/src/hub_link.rs` | 72 | 48 random bytes for one handshake (ephemeral key + nonce) |
| `pc-daemon/src/hub_link.rs` | 117 | verifies the hub's signature with the pinned key. |
| `pc-daemon/src/hub_link.rs` | 183 | hub pairing. A hub is only pinned after a person approved |
| `pc-daemon/src/main.rs` | 62 | the PC's long-term Ed25519 identity (the hub pins it at pairing). |
| `pc-daemon/src/state.rs` | 44 | the PC daemon's long-term signing key. |
| `pc-daemon/src/state.rs` | 49 | hub key a person approved in the app; it's pinned once |
| `uplink/src/lib.rs` | 162 | seal a JSON value as an Uplink frame under the session. |
| `uplink/src/lib.rs` | 173 | open an Uplink frame. `open` is either `Session::open` or |
| `uplink/src/lib.rs` | 220 | used for the PC daemon's identity seed. The OS RNG fills |
| `uplink/src/lib.rs` | 253 | OS CSPRNG (getrandom / /dev/urandom / BCryptGenRandom). |

_85 tagged locations._

<!-- END CRYPTO-REVIEW INDEX -->
