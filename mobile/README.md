# Phone side (iOS + Android)

The phone app is the same Tauri + Leptos app as the desktop (`app/`), started
in phone mode. This folder holds its Rust link to the hub.

## What it does
- Talks straight to the Pi hub over farm Wi-Fi (TCP 7302)
- Same encryption as the PC link: v2 hybrid handshake (Ed25519 + ML-DSA-65, X25519 + ML-KEM-768), ChaCha20-Poly1305
- Live readings, history, switch outputs, alerts, approve new nodes
- Recipes as a sentence or as Blockly blocks; the hub stores and runs them, so they work with the PC and phone off

## Layers
- `app/ui`: the screens (Leptos → WASM), the same code as the desktop
- `app/src-tauri/src/phone.rs`: phone backend; keeps the identity and pairing, sends requests
- `mobile/core`: Rust; pairing (QR ticket or manual approval), handshake, encryption, TCP, retries
- `hub-node/src/hub/app.rs`: hub answers the phone (sockets in `hub-node/src/app_tcp.rs`)
- `hub-node/src/recipes.rs`: hub runs recipes
- All crypto in Rust (`crypto-sharecrate`); the web view only sees JSON
- Crypto notes: "Phone app" and "v2 app shell" in `docs/CRYPTO_REVIEW.md`

## Test the hub side without a phone
- Hub + PC daemon running (main README)
- By QR: desktop Pairing → Add phone → Copy link, then `cargo run -p farmlab-mobile-core --example phone -- --state /tmp/phone.json pair-qr '<link>'`
- Or manually: `cargo run -p farmlab-mobile-core --example phone -- --hub 127.0.0.1:7302 --state /tmp/phone.json pair`, then approve "Phone app" on the desktop Pairing page
- `cargo run -p farmlab-mobile-core --example phone -- --hub 127.0.0.1:7302 --state /tmp/phone.json ask '{"type":"channels"}'`
- Request types: top of `hub-node/src/hub/app.rs`
- The whole phone UI on a computer: `cd app/src-tauri && FARMLAB_PHONE=1 cargo tauri dev`

## Build
See `app/README.md` → Phones (Android, iOS on a Mac, iOS from Linux with xtool).

## Using it
- Same Wi-Fi as the hub
- Allow "Local Network" on iOS, or it can't reach the hub
- Desktop: Pairing → Add phone shows a QR code; phone: Settings → Scan QR (or paste the link)
- Match the hub fingerprint the phone shows with the one on the desktop
- Live: tap a sensor → history; switch → set output
- Lost phone: unpair it under Paired phones on the desktop
