# Farm & Lab app (v2): Tauri + Leptos

One Rust app for the desktop and the phone. It replaced the v1 Electron
desktop app and the Expo phone app.

## Layout
- `ui/`: the whole interface. Leptos, compiled to WASM with Trunk.
  - `src/api.rs`: the one data layer. Desktop → pc-daemon, phone → hub, or sample data in a plain browser
  - `src/pages/`: Overview / Live, Devices, History, Automation, Alerts, Pairing, Settings, first-run wizard
  - `src/recipe_edit.rs`: recipe ↔ Blockly JSON, and the simple "When ___ is above ___, turn ___ on" builder (tested)
  - `src/chart.rs`: SVG history chart (tested geometry)
  - `style/tokens.css`: design tokens from `docs/design/specs/farmlab-spec.md` (branch `v2/0-design`)
  - `vendor/`: Blockly 13.4.0 core (Apache-2.0) + `blockly-shim.js`, Geist fonts (OFL). Nothing is loaded from the network
- `src-tauri/`: the native shell
  - `daemon.rs`: desktop backend; reads the daemon's API token and calls `127.0.0.1:7401` (the page never sees the token)
  - `phone.rs`: phone backend; `farmlab-mobile-core` (v2 crypto, TCP to the hub)
  - `limits.rs`: Lite (< 3 GB) / Normal (< 6 GB) RAM caps and the CPU slider

## Run it
Needs: `rustup target add wasm32-unknown-unknown`, `cargo install trunk tauri-cli --locked`,
and on Linux the WebKitGTK dev packages (`libwebkit2gtk-4.1-dev`).

- Desktop, against a running pc-daemon: `cd app/src-tauri && cargo tauri dev`
  - another daemon: `FARMLAB_API_URL=http://127.0.0.1:17401 FARMLAB_TOKEN_FILE=/path/to/api-token cargo tauri dev`
- The phone app on a computer (same code, talks to the hub): `FARMLAB_PHONE=1 cargo tauri dev`
  - pair it in Settings with the link from the desktop's Pairing → Add phone → Copy link
- Just the UI with sample data, in any browser: `cd app/ui && trunk serve`, open http://127.0.0.1:1420
  - phone layout: `?phone`
  - any screen and look: `?page=history&mode=lab&theme=light&boomer&sun&lite`, recipe editor `?page=automation&edit=2`, wizard `?wizard`
- Release build: `cd app/src-tauri && cargo tauri build` (`.deb` / `.rpm` / AppImage on Linux, `.msi` / `.exe` on Windows, `.dmg` on macOS)

## Phones
The phone app is this same app; `is_phone()` is true on iOS and Android.
- Android (Linux, macOS or Windows): Android SDK + NDK, `JAVA_HOME` (JDK 17 or 21), then
  - `cargo tauri android init` once (writes `src-tauri/gen/android`, not committed)
  - `cargo tauri android dev` (phone on USB or an emulator) / `cargo tauri android build --apk`
- iOS on a Mac: Xcode, then `cargo tauri ios init` once, `cargo tauri ios dev` / `build`
- iOS from Linux, with [xtool](https://github.com/xtool-org/xtool) (`app/ios/`, a SwiftPM project instead of an Xcode one):
  - Swift 6 (`swiftly`), the xtool AppImage, `rustup target add aarch64-apple-ios`
  - `xtool setup` once: signs in with your Apple ID and builds the iOS SDK from an `Xcode.xip` (download it from developer.apple.com)
  - iPhone on USB (needs `usbmuxd` running), Developer Mode on
  - `./scripts/ios-xtool.sh` builds the UI, the Rust app as a static library, links it with Tauri's Swift runtime and the barcode plugin, then installs and opens it (`build` to only build)
  - a free Apple ID's apps expire after 7 days; run it again
- iOS asks for the camera (QR scanner) and Local Network (the hub); the texts are in `src-tauri/Info.ios.plist`
- The camera QR scanner (`tauri-plugin-barcode-scanner`) is only in these builds; elsewhere the pairing link can be pasted
- CI builds a debug APK and an iOS simulator build on every push (artifacts on the run page)

## Performance modes
Picked in the first-run wizard, changed in Settings.
- Linux: release builds restart themselves in a systemd user scope (`MemoryMax`, `CPUQuota`); the slider updates it live, and the `farmlab-daemon` user service gets the same limits
- Windows: a Job Object (memory limit + hard CPU cap) that WebView2's processes inherit
- macOS / phones: no hard cap for apps; the app polls less often, keeps shorter charts and drops animations instead
- Turn the Linux scope off: `FARMLAB_NO_SCOPE=1`; try it in a debug build: `FARMLAB_SCOPE_DEV=1`

## Design
- Modes: Lab (blue), Farm (green), Home (orange); light / dark / match system
- Sunlight: black on white, thick lines, big targets; on by default for Farm on the phone
- Boomer Mode: 1.35× type, 56 px targets, one column, no motion, recipes as a sentence
- Linear / Vercel / Chronicle patterns: inverted-L shell with a dotted panel, ⌘K command menu, `G` + key jumps, sliding tabs, Sonner-style toasts with Undo
- Motion only explains a change, ≤ 400 ms; off for Boomer Mode and reduced motion, fades only in Lite
