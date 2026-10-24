# FFA Autonomous Farm & Lab Management

An IoT system for farms and labs: ESP32-C3 sensor/actuator nodes report to a
Raspberry Pi hub over **LoRa or Wi-Fi**. The hub forwards everything to a
daemon on your PC, and the PC shows it in a desktop app with charts, controls,
alerts and automation rules. Every link is end-to-end encrypted with
**ECDH (X25519) + ChaCha20-Poly1305**, with fresh keys every **10 messages or 1 hour**.

```
 ┌───────────────┐  LoRa 915 MHz (SX1262)  ┌──────────────────┐   TCP (encrypted)   ┌──────────────┐  localhost REST  ┌──────────────┐
 │ ESP32-C3 node │ ───────────────────────▶│ Raspberry Pi hub │ ──────────────────▶ │  PC daemon   │ ◀──────────────▶ │ Electron app │
 │ no_std Rust   │  Wi-Fi UDP (2.4 GHz)    │ RAM SQLite, 30 d │                     │ SQLite, rules│  (token via IPC) │ React + TS   │
 │ sensors/relays│ ───────────────────────▶│ daily SD flush   │                     │ REST API     │                  │              │
 └───────────────┘                         └──────────────────┘                     └──────────────┘                  └──────────────┘
```

| Folder | What | Runs on |
|---|---|---|
| `crypto-sharecrate/` | Protocol + crypto, `no_std`, shared by everything | all |
| `data-node/` | Node firmware (`no_std`, esp-hal + embassy) | ESP32-C3 |
| `hub-node/` | Hub: radios, pairing, sessions, 30-day buffer | Raspberry Pi 3B |
| `pc-daemon/` | Storage, automation, REST API | your PC |
| `uplink/` | Hub ↔ PC message types + TCP framing | hub + PC |
| `node-sim/` | Fake node for testing without hardware | your PC |
| `frontend/` | Electron + React + TypeScript app | your PC |
| `database/` | SQLite schemas (PC and hub) | |
| `docs/CRYPTO_REVIEW.md` | **Crypto design, known weaknesses, index of every `CRYPTO-REVIEW` tag** | |
| `USER_MANUAL.md` | Wiring, flashing, setup, pairing, troubleshooting | |

## Quick start on one PC (no hardware)

```bash
cargo build --workspace
```

Start the daemon first and copy the **PC public key** from its log:

```bash
cargo run -p pc-daemon
```

In another terminal, make a hub config with that key:

```bash
cp hub-node/hub.example.toml /tmp/hub.toml && sed -i 's|/var/lib/ffa-hub|/tmp/ffa-hub|; s|192.168.1.50|127.0.0.1|; s|paste-the-64-character-key-here|PASTE_PC_KEY_HERE|' /tmp/hub.toml
```

Then the hub, the app and a simulated node, each in its own terminal:

```bash
cargo run -p hub-node -- --config /tmp/hub.toml init && cargo run -p hub-node -- --config /tmp/hub.toml
```

```bash
cd frontend && npm install && npm run dev
```

```bash
cargo run -p node-sim -- --state /tmp/sim1.json --pair --hub-key HUB_PUBLIC_KEY_FROM_INIT
```

In the app, open **Pairing**, approve the hub, then approve the node. Readings
show up on the Dashboard within a few seconds. Toggle the siren/solenoid and
watch the simulator log `>>> SIREN (ch 2) set to ON`.

## Tests

```bash
cargo test --workspace
```

```bash
cd data-node && cargo build --release
```

See [USER_MANUAL.md](USER_MANUAL.md) for real hardware.
