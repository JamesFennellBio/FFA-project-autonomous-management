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
  - `pc_addr`: PC's IP
  - `pc_key_id`: from pc-daemon's log / app Pairing page (hub only talks to that PC)
  - `lora.enabled = true`
  - `app_listen`: phone port (TCP), default `0.0.0.0:7302`
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
- Run at login (Linux): `deploy/farmlab-daemon.service`
- App needs Node.js 20+: `cd frontend && npm install && npm run dev`
- Installer (AppImage / .exe / .dmg): `cd frontend && npm run dist`

## Pairing
- Only step where a person decides trust → check fingerprints
- Hub → PC (once):
  - `pc_key_id` set, daemon + hub running
  - App → Pairing → "Hub (Raspberry Pi)"
  - Fingerprint must match the "phones, PC" line of `hub-node fingerprint` → Approve
- Node → hub:
  - Node running, hold BOOT 3 s (not while plugging in: that's flashing mode)
  - Node asks to pair for 60 s
  - Fingerprint must match serial log → name it → Approve
  - Node logs `PAIRED as node 1 with hub 23C8-…` → match the app sidebar
- Phone → hub:
  - Phone Settings → hub IP → Pair with hub
  - Desktop Pairing → "Phone app" → match fingerprint → name → Approve
- Unpair a node:
  - Devices → Unpair (hub must be online)
  - and hold BOOT on the node 10 s
  - id never reused; recipes that use it stop working (edit or delete them)
- Unpair a phone: Pairing → Paired phones → Unpair (hub must be online)

## Using the desktop app
- Dashboard: live values, On/Off for outputs; nodes report every 10 s
- History: chart per channel, 1 h / 24 h / 7 d / 30 d
- Automation: Blockly recipes, the same ones as the phone app
  - New recipe → drag blocks into when / do / otherwise → Save to hub
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
- Hub log "answered with a key that is not pc_key_id" → recopy the key ID from the Pairing page
- Hub won't start, "pc_public_key from before v2" → replace it with `pc_key_id` (Pairing page)
- After upgrading to v2: approve the hub again on the Pairing page, and pair each phone again (nodes keep working)
- Node log "saved pairing is for a different hub than FARMLAB_HUB_KEY" → pair again
- Command "dropped" → LoRa node out of reach, 16+ queued; resend later
- Phone "the hub didn't answer" → same Wi-Fi (not guest / mobile data), right IP, iOS Local Network allowed, TCP 7302 open on the Pi
- Phone pairing never approved → PC daemon running and hub connected to it
