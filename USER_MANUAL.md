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
  - `FFA_WIFI_SSID`, `FFA_WIFI_PASSWORD`
  - `FFA_HUB_IP`: Pi's IP
  - `FFA_HUB_KEY`: from `hub-node fingerprint`; node refuses any other hub
  - `FFA_TRANSPORT`: `wifi`, `lora` or `auto` (Wi-Fi when connected, else LoRa)
- Without `FFA_HUB_KEY`, someone nearby could answer the pairing request first
- Wi-Fi: `cd data-node && FFA_WIFI_SSID="FarmNet" FFA_WIFI_PASSWORD="..." FFA_HUB_IP="192.168.1.10" FFA_HUB_KEY="<hub key>" FFA_TRANSPORT="wifi" cargo run --release`
- LoRa / auto: same with `FFA_TRANSPORT="auto"` and `cargo run --release --features lora`
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
- Copy: `scp target/aarch64-unknown-linux-gnu/release/hub-node hub-node/hub.example.toml deploy/ffa-hub.service pi@ffa-hub.local:~`
- Install on the Pi: `sudo useradd -r -G spi,gpio ffa; sudo install hub-node /usr/local/bin/; sudo mkdir -p /etc/ffa-hub /var/lib/ffa-hub; sudo chown ffa /var/lib/ffa-hub; sudo cp hub.example.toml /etc/ffa-hub/hub.toml`
- Edit `/etc/ffa-hub/hub.toml`:
  - `pc_addr`: PC's IP
  - `pc_public_key`: from pc-daemon's log / app Pairing page (hub only talks to that PC)
  - `lora.enabled = true`
  - `app_listen`: phone port, default `0.0.0.0:7301`
- Create identity: `sudo -u ffa hub-node --config /etc/ffa-hub/hub.toml init`
  - 24 words shown once, never saved → write on paper
  - derived key IS saved in `/var/lib/ffa-hub/identity.key` → keep SD card safe
  - prints hub public key = `FFA_HUB_KEY` (again: `hub-node fingerprint`)
- Start: `sudo cp ffa-hub.service /etc/systemd/system/ && sudo systemctl enable --now ffa-hub`
- Readings in RAM, written to SD once a day + on stop, 30 days kept
- Pairings and recipes saved right away

## Setting up the PC
- `cargo build --release -p pc-daemon`
- `./target/release/pc-daemon`
- Firewall: allow TCP 7400 from the Pi (API on 7401 stays localhost-only)
- Run at login (Linux): `deploy/ffa-daemon.service`
- App needs Node.js 20+: `cd frontend && npm install && npm run dev`
- Installer (AppImage / .exe / .dmg): `cd frontend && npm run dist`

## Pairing
- Only step where a person decides trust → check fingerprints
- Hub → PC (once):
  - `pc_public_key` set, daemon + hub running
  - App → Pairing → "Hub (Raspberry Pi)"
  - Fingerprint must match `hub-node fingerprint` → Approve
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
  - deletes its rules; id never reused
- Unpair a phone: Pairing → Paired phones → Unpair (hub must be online)

## Using the desktop app
- Dashboard: live values, On/Off for outputs; nodes report every 10 s
- History: chart per channel, 1 h / 24 h / 7 d / 30 d
- Automation: e.g. temp > 30 °C → siren ON, OFF when it clears (runs on the PC)
- Alerts: sensor faults, rules fired, recipe alerts
- Devices: rename nodes / channels, key-exchange count
- Safety: node hears nothing for 5 min → all outputs OFF

## The phone app and recipes
- Build / install: `mobile/README.md`
- Talks to the hub on UDP 7301, same encryption as nodes
- Live / History / Alerts: last 30 days (what the hub keeps)
- Pairing tab: nodes holding BOOT → approve from phone (PC must be on, it picks the id)
- Recipes:
  - when (sensor comparisons + and / or / not) → do (outputs on/off, alert) → otherwise
  - run on the hub → work with PC and phone off
  - max 32
  - off / edit / delete while active → runs "otherwise" first
  - separate from the desktop Automation rules
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
- `sudo -u ffa hub-node --config /etc/ffa-hub/hub.toml restore` → type the 24 words
- Existing identity → needs `--force` (old-key nodes stop working)
- Same identity back → nodes keep working
- Copy `paired.json` (and `recipes.json`) from a backup of `/var/lib/ffa-hub`, or re-pair
- Re-approve the hub on the PC if the PC lost its database

## Troubleshooting
- "Daemon offline" → start `pc-daemon`; token in `~/.config/ffa-daemon/api-token`
- Hub never shows for pairing → `pc_addr`, PC firewall TCP 7400
- Node `wifi connect failed` → SSID/password, 2.4 GHz only, rebuild after env changes
- Node keeps sending HELLO → `FFA_HUB_IP`; unpaired? hold BOOT 10 s, pair again
- `SX1262 init failed` → wiring (BUSY/RESET); Ebyte E22 needs `tcxo = true` (hub) / `tcxo: true` in `setup_lora` (node)
- Temperature missing / "Sensor fault" → 4.7 kΩ pull-up DATA ↔ 3V3
- Readings stop with PC off → expected; hub stores 30 days, sends backlog later
- Hub log "answered with a key that is not pc_public_key" → recopy key from Pairing page
- Node log "saved pairing is for a different hub than FFA_HUB_KEY" → pair again
- Command "dropped" → LoRa node out of reach, 16+ queued; resend later
- Phone "the hub didn't answer" → same Wi-Fi (not guest / mobile data), right IP, iOS Local Network allowed, UDP 7301 open on the Pi
- Phone pairing never approved → PC daemon running and hub connected to it
