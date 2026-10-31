# User manual

## Parts
- 3× ESP32-C3 (SuperMini or DevKitM-1)
- 1× Raspberry Pi 3B v1.2, 16 GB+ SD card, Raspberry Pi OS Lite 64-bit
- 4× SX1262 LoRa breakout, 915 MHz (US): one per node + one for the Pi
  - e.g. Waveshare Core1262-HF, Ebyte E22-900M22S
  - never transmit without an antenna
- DS18B20 waterproof probe + 4.7 kΩ resistor
- Photoresistor (LDR) + 10 kΩ resistor
- 2× logic-level N-MOSFET (e.g. IRLZ44N) or 3.3 V relay modules
- 12 V solenoid valve + 1N4007 diode (across the coil, stripe to +) + 12 V supply
- 5 V or 12 V siren / buzzer

## Wiring a data node
- DS18B20: VDD (red) → 3V3, DATA (yellow) → GPIO4, GND (black) → GND
- 4.7 kΩ between DATA and 3V3
- Photoresistor divider: 3V3 → LDR → GPIO3 → 10 kΩ → GND
- Siren: GPIO5 → MOSFET/relay IN (siren on its own supply)
- Solenoid: GPIO6 → MOSFET/relay IN, 1N4007 across the coil
- MOSFET/relay GND → ESP GND (common ground with 12 V supply)
- LoRa (only with `--features lora`): SCK GPIO10, MOSI GPIO7, MISO GPIO21, NSS GPIO2, RESET GPIO0, BUSY GPIO1, DIO1 GPIO20, 3V3, GND
- BOOT (GPIO9) = pairing button
- GPIO8 = onboard LED, flashes on transmit
- LoRa uses GPIO20/21 (UART pins) → read logs over USB
- Never drive the solenoid / siren straight from a GPIO (3.3 V, ~20 mA)

## Flashing a node
- Set up the Pi first (need the hub key)
- One-time: `rustup target add riscv32imc-unknown-none-elf`
- One-time: `cargo install espflash --locked`
- Give the Pi a DHCP reservation on the router
- Env vars:
  - `FARMLAB_WIFI_SSID`, `FARMLAB_WIFI_PASSWORD`
  - `FARMLAB_HUB_IP`: Pi's IP
  - `FARMLAB_HUB_KEY`: from `hub-node fingerprint`; node refuses any other hub
  - `FARMLAB_TRANSPORT`: `wifi`, `lora` or `auto` (Wi-Fi when connected, else LoRa)
- Without `FARMLAB_HUB_KEY`, someone nearby could answer the pairing request first
- Wi-Fi: `cd data-node && FARMLAB_WIFI_SSID="FarmNet" FARMLAB_WIFI_PASSWORD="..." FARMLAB_HUB_IP="192.168.1.10" FARMLAB_HUB_KEY="<hub key>" FARMLAB_TRANSPORT="wifi" cargo run --release`
- LoRa / auto: same with `FARMLAB_TRANSPORT="auto"` and `cargo run --release --features lora`
- First boot prints a fingerprint, e.g. `8FA7-0608-CEBB-45C1` → note it for pairing

## Setting up the Pi hub
- Flash Raspberry Pi OS Lite 64-bit, enable SSH, same network as the PC
- Enable SPI: `sudo raspi-config` → Interface Options → SPI → Yes
- SX1262 wiring (BCM):
  - SCK GPIO11 (pin 23)
  - MOSI GPIO10 (pin 19)
  - MISO GPIO9 (pin 21)
  - NSS GPIO25 (pin 22)
  - RESET GPIO22 (pin 15)
  - BUSY GPIO23 (pin 16)
  - DIO1 GPIO24 (pin 18)
  - 3V3 pin 17, GND pin 20
- Build on the PC (needs Docker):
  - `cargo install cross --locked`
  - `cross build --release --target aarch64-unknown-linux-gnu -p hub-node --features lora`
- Copy: `scp target/aarch64-unknown-linux-gnu/release/hub-node hub-node/hub.example.toml deploy/farmlab-hub.service pi@farmlab-hub.local:~`
- Install on the Pi: `sudo useradd -r -G spi,gpio farmlab; sudo install hub-node /usr/local/bin/; sudo mkdir -p /etc/farmlab-hub /var/lib/farmlab-hub; sudo chown farmlab /var/lib/farmlab-hub; sudo cp hub.example.toml /etc/farmlab-hub/hub.toml`
- Edit `/etc/farmlab-hub/hub.toml`:
  - `lora.enabled = true`
  - `app_listen`: phone port (TCP), default `0.0.0.0:7302`
  - no PC settings needed: the hub learns the PC's address and key when you pair (see Pairing)
  - optional: `pc_addr` if the PC's IP changes later; `pc_key_id` to only ever pair with that PC
- Create identity: `sudo -u farmlab hub-node --config /etc/farmlab-hub/hub.toml init`
  - 24 words shown once, never saved → write on paper
  - derived key IS saved in `/var/lib/farmlab-hub/identity.key` → keep SD card safe
  - prints hub public key = `FARMLAB_HUB_KEY` (again: `hub-node fingerprint`)
- Start: `sudo cp farmlab-hub.service /etc/systemd/system/ && sudo systemctl enable --now farmlab-hub`
- Readings in RAM, written to SD once a day + on stop, 30 days kept
- Pairings and recipes saved right away

## Setting up the PC
- `cargo build --release -p pc-daemon`
- `./target/release/pc-daemon`
- Firewall: allow TCP 7400 from the Pi (API on 7401 stays localhost-only)
- Pi firewall (if any): allow TCP 7302 (phones) and, while pairing, TCP 7402
- Run at login (Linux): `deploy/farmlab-daemon.service`
- App (v2): `cd app/src-tauri && cargo tauri build` → installer in `target/release/bundle/` (setup: `app/README.md`)
- Old Electron app (until v2 replaces it): Node.js 20+, `cd frontend && npm install && npm run dev`

## Pairing
- Only step where a person decides trust
- Hub → PC (once), proves it's the right Pi:
  - daemon + hub running
  - on the Pi: `sudo -u farmlab hub-node --config /etc/farmlab-hub/hub.toml pair`
  - prints the Pi's IP and a 12-digit code (e.g. `4821-7730-1956`), open for 10 minutes
  - App → Pairing → Add hub → IP + code → Pair hub
  - only the Pi that printed the code can finish; an impostor can't guess it offline
  - 3 wrong codes or one success closes the window; run `pair` again for a new code
  - the running hub connects to the PC within a few seconds
  - new PC later: `hub-node pair --replace`
- Node → hub:
  - Node running, hold BOOT 3 s (not while plugging in: that's flashing mode)
  - Node asks to pair for 60 s
  - Fingerprint must match serial log → name it → Approve
  - Node logs `PAIRED as node 1 with hub 23C8-…` → match the app sidebar
- Phone → hub, by QR code:
  - Desktop Pairing → Add phone → QR code (hub must be online)
  - scan it with the phone app → paired, no approval click
  - works once, for 5 minutes; a hub restart cancels open codes
  - the code also tells the phone which hub to trust, so it can't be fooled by another hub
  - the QR is a key: don't let others photograph it while it's open
  - (the scanner comes with the v2 phone app; until then use the manual way)
- Phone → hub, manually:
  - Phone Settings → hub IP → Pair with hub
  - Desktop Pairing → "Phone app" → match fingerprint → name → Approve
- Unpair a node:
  - Devices → Unpair (hub must be online)
  - and hold BOOT on the node 10 s
  - id never reused; recipes that use it stop working (edit or delete them)
- Unpair a phone: Pairing → Paired phones → Unpair (hub must be online)

## First start (v2 app)
- Setup asks, one screen each (all changeable later in Settings):
  - Mode: Lab (blue), Farm (green) or Home (orange); changes colors and words
  - Look: Standard, or Boomer Mode (big text and buttons, plain words, no animations, rules as a sentence)
  - Theme: light, dark or match the system
  - Performance: Lite (under 3 GB of memory) or Normal (under 6 GB), plus a processor limit slider; shows this computer's memory
  - Link the hub: the code from `hub-node pair` (desktop) or the QR code from the desktop (phone)
- Sunlight (Settings): black on white for bright sun; on by default for Farm on the phone
- Limits: Linux and Windows enforce them (Settings shows "Enforced by the system"); elsewhere the app holds back by itself

## Using the desktop app
- ⌘K / Ctrl+K: search and jump; `G` then a letter jumps to a page; `C` starts a new recipe
- Overview: live values with sparklines, outputs (switching one shows Undo), recent alerts; nodes report every 10 s
- History: chart per channel, 1 h / 24 h / 7 d / 30 d
- Automation: recipes, the same ones as the phone app
  - Simple: "When [sensor] is above [30] °C, turn [Fans] on", optionally switch it back and send an alert
  - Blocks: drag blocks into when / do / otherwise (and / or / not, several actions) → Save to hub
  - stored and run on the hub; the PC only passes them on (hub must be connected to edit)
  - On/Off switch per recipe
- Alerts: sensor faults, recipe alerts
- Devices: rename nodes / channels, key-exchange count
- Safety: node hears nothing for 5 min → all outputs OFF

## The phone app and recipes
- Build / install: `mobile/README.md`
- Talks to the hub on TCP 7302 with v2 hybrid post-quantum encryption (Ed25519 + ML-DSA-65, X25519 + ML-KEM-768)
- Live / History / Alerts: last 30 days (what the hub keeps)
- Pairing tab: nodes holding BOOT → approve from phone (PC must be on, it picks the id)
- Recipes:
  - when (sensor comparisons + and / or / not) → do (outputs on/off, alert) → otherwise
  - run on the hub → work with PC and phone off
  - max 32
  - off / edit / delete while active → runs "otherwise" first
  - same recipes as the desktop Automation page
- Lost phone: unpair it on the desktop; until then it still works

## Adding sensors and outputs
- Edit the channel list in `data-node/src/main.rs`
  - `chans.ds18b20(0, p.GPIO4);` thermometer
  - `chans.analog_in(1, photo);` ADC1 pin (GPIO0-4), millivolts
  - `chans.digital_out(2, p.GPIO5);` siren
  - `chans.digital_out(3, p.GPIO6);` solenoid
  - `chans.digital_in(4, p.GPIO20, Pull::Up);` e.g. float switch to GND
- Max 7 channels per node
- New channels show up by themselves (node announces after boot)

## Hub recovery (recovery phrase)
- New SD card, install hub-node as above
- `sudo -u farmlab hub-node --config /etc/farmlab-hub/hub.toml restore` → type the 24 words
- Existing identity → needs `--force` (old-key nodes stop working)
- Same identity back → nodes keep working
- Copy `paired.json` (and `recipes.json`) from a backup of `/var/lib/farmlab-hub`, or re-pair
- Re-approve the hub on the PC if the PC lost its database

## Troubleshooting
- "Daemon offline" → start `pc-daemon`; token in `~/.config/farmlab-daemon/api-token`
- Hub never shows for pairing → `pc_addr`, PC firewall TCP 7400
- Node `wifi connect failed` → SSID/password, 2.4 GHz only, rebuild after env changes
- Node keeps sending HELLO → `FARMLAB_HUB_IP`; unpaired? hold BOOT 10 s, pair again
- `SX1262 init failed` → wiring (BUSY/RESET); Ebyte E22 needs `tcxo = true` (hub) / `tcxo: true` in `setup_lora` (node)
- Temperature missing / "Sensor fault" → 4.7 kΩ pull-up DATA ↔ 3V3
- Readings stop with PC off → expected; hub stores 30 days, sends backlog later
- Hub log "answered with a key that is not pc_key_id" → remove `pc_key_id` from hub.toml or recopy it from the Pairing page
- Hub won't start, "pc_public_key from before v2" → delete that line (pair with `hub-node pair` instead)
- Hub log "not paired with a PC yet" → run `hub-node pair` and enter the code in the app
- Add hub: "can't reach the hub" → `hub-node pair` still running? right IP? TCP 7402 open on the Pi?
- Add hub: "wrong pairing code" → retype it; after 3 tries run `hub-node pair` again
- After upgrading to v2: pair the hub again with `hub-node pair`, and pair each phone again (nodes keep working)
- Node log "saved pairing is for a different hub than FARMLAB_HUB_KEY" → pair again
- Command "dropped" → LoRa node out of reach, 16+ queued; resend later
- Phone "the hub didn't answer" → same Wi-Fi (not guest / mobile data), right IP, iOS Local Network allowed, TCP 7302 open on the Pi
- Phone pairing never approved → PC daemon running and hub connected to it
