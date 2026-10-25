# Autonomous Farm & Lab Management

## What it is
- IoT farm / lab management system
- ESP32-C3 data nodes → Raspberry Pi 3B hub → PC daemon → Electron desktop app
- Phone app (iOS + Android) talks to the hub directly
- Recipes (if-this-then-that) built with Blockly blocks on the phone, run on the hub
- Demo channels: siren, solenoid, photoresistor, DS18B20 thermometer
- Website: `website/` (Farm / Lab versions, editable in the browser)

## Radios / links
- Node → hub: LoRa SX1262 (915 MHz, SF7) or Wi-Fi UDP (port 7300)
- Hub → PC: TCP (port 7400)
- Phone → hub: Wi-Fi UDP (port 7301)
- Desktop app → PC daemon: REST on 127.0.0.1 (port 7401), bearer token

## Crypto
- Ed25519 identities
- X25519 ECDH, signed handshake
- HKDF-SHA256 → separate key per direction
- ChaCha20-Poly1305
- New keys every 10 messages or 1 hour
- Pairing: button press + approval in the desktop app
- Hub recovery: 24-word BIP39 phrase
- All crypto lines tagged `CRYPTO-REVIEW`
- Index + design notes: `docs/CRYPTO_REVIEW.md`

## Storage
- Hub: SQLite in RAM, written to SD once a day, keeps 30 days
- PC: SQLite, full history

## Folders
- `crypto-sharecrate/`: protocol + crypto, no_std, shared by everything
- `data-node/`: ESP32-C3 firmware (no_std Rust)
- `hub-node/`: Pi hub (Rust)
- `pc-daemon/`: PC daemon, SQLite, REST API, rules (Rust)
- `uplink/`: hub ↔ PC message types
- `node-sim/`: fake node for testing without hardware
- `frontend/`: Electron + React + TypeScript
- `mobile/core/`: phone side of the hub link (Rust, uniffi)
- `mobile/native/`: React Native module built from `mobile/core`
- `mobile/app/`: Expo / React Native app
- `database/`: SQLite schemas
- `USER_MANUAL.md`: wiring, flashing, setup
- `mobile/README.md`: building the phone app

## Run on one PC (no hardware)
- `cargo build --workspace`
- `cargo run -p pc-daemon` (copy the PC public key from the log)
- `cp hub-node/hub.example.toml /tmp/hub.toml`, set `pc_addr = "127.0.0.1:7400"`, `pc_public_key`, `data_dir = "/tmp/farmlab-hub"`
- `cargo run -p hub-node -- --config /tmp/hub.toml init`
- `cargo run -p hub-node -- --config /tmp/hub.toml`
- `cd frontend && npm install && npm run dev`
- `cargo run -p node-sim -- --state /tmp/sim1.json --pair --hub-key <hub public key>`
- Approve the hub, then the node, on the Pairing page
- Phone without a phone: `cargo run -p farmlab-mobile-core --example phone -- --state /tmp/phone.json pair`, approve it, then `... ask '{"type":"channels"}'`

## Tests
- `cargo test --workspace`
- `cd data-node && cargo build --release`
