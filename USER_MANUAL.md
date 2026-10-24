# User manual

1. [Parts](#parts)
2. [Wiring a data node](#wiring-a-data-node)
3. [Flashing a node](#flashing-a-node)
4. [Setting up the Pi hub](#setting-up-the-pi-hub)
5. [Setting up the PC](#setting-up-the-pc)
6. [Pairing](#pairing)
7. [Using the app](#using-the-app)
8. [Adding your own sensors and outputs](#adding-your-own-sensors-and-outputs)
9. [If the hub dies: recovery phrase](#if-the-hub-dies-recovery-phrase)
10. [Troubleshooting](#troubleshooting)

## Parts

| Qty | Part | Notes |
|---|---|---|
| 3 | ESP32-C3 board (SuperMini or DevKitM-1) | already have |
| 1 | Raspberry Pi 3B v1.2 + 16 GB+ SD card | already have; Raspberry Pi OS Lite **64-bit** |
| 4 | SX1262 LoRa breakout, **915 MHz** (US) | e.g. Waveshare Core1262-HF or Ebyte E22-900M22S. One per node + one for the Pi. Add an antenna: **never transmit without one**. |
| 1 | DS18B20 (waterproof probe version) + 4.7 kΩ resistor | thermometer |
| 1 | Photoresistor (LDR) + 10 kΩ resistor | light sensor |
| 2 | Logic-level N-MOSFET modules (e.g. IRLZ44N) or 3.3 V relay modules | for the siren and the solenoid |
| 1 | 12 V solenoid valve + 1N4007 diode + 12 V supply | the diode goes **across the coil** (stripe to +) |
| 1 | 5 V or 12 V siren/buzzer | |

## Wiring a data node

```
ESP32-C3                      Parts
────────                      ─────
3V3 ───────┬──────────────── DS18B20 VDD (red)
           ├── 4.7 kΩ ──┐
GPIO4 ─────┼────────────┴─── DS18B20 DATA (yellow)
GND ───────┼──────────────── DS18B20 GND (black)
           │
3V3 ───────┴── LDR ──┬────── GPIO3      (photoresistor voltage divider)
GND ─────── 10 kΩ ───┘

GPIO5 ───── MOSFET/relay IN ─── siren   (siren powered from its own supply)
GPIO6 ───── MOSFET/relay IN ─── solenoid (+ 1N4007 across the solenoid coil!)
GND ─────── MOSFET/relay GND    (common ground with the 12 V supply)

LoRa (only with --features lora)
GPIO10 SCK · GPIO7 MOSI · GPIO21 MISO · GPIO2 NSS · GPIO0 RESET · GPIO1 BUSY · GPIO20 DIO1 · 3V3 · GND
```

Notes:
- **BOOT (GPIO9)** is the pairing button. **GPIO8** is the onboard LED (it flashes when transmitting).
- LoRa uses GPIO20/21 (the UART pins), so watch logs over the **USB** port.
- Never drive the solenoid or siren straight from a GPIO pin. A GPIO gives 3.3 V at about 20 mA.

## Flashing a node

One-time setup on your PC:

```bash
rustup target add riscv32imc-unknown-none-elf
```

```bash
cargo install espflash --locked
```

Each node: fill in your network, the Pi's IP (give it a DHCP reservation on
your router) and the **hub's public key**, which `hub-node fingerprint` prints
on the Pi. With `FFA_HUB_KEY` set, the node refuses to pair with any other hub.
Without it, someone nearby could answer the node's pairing request first.

```bash
cd data-node && FFA_WIFI_SSID="FarmNet" FFA_WIFI_PASSWORD="your-password" FFA_HUB_IP="192.168.1.10" FFA_HUB_KEY="<hub public key>" FFA_TRANSPORT="wifi" cargo run --release
```

`FFA_TRANSPORT` is `wifi`, `lora` or `auto` (Wi-Fi while connected, otherwise
LoRa). `lora` and `auto` need the LoRa module and `--features lora`:

```bash
cd data-node && FFA_WIFI_SSID="FarmNet" FFA_WIFI_PASSWORD="your-password" FFA_HUB_IP="192.168.1.10" FFA_HUB_KEY="<hub public key>" FFA_TRANSPORT="auto" cargo run --release --features lora
```

Set up the Pi first (next section) so you have the hub key.

On first boot the node creates its identity and prints a fingerprint like
`FFA data node, fingerprint 8FA7-0608-CEBB-45C1`. Write it down for pairing.

## Setting up the Pi hub

1. Flash **Raspberry Pi OS Lite (64-bit)**, enable SSH, connect it to the same network as the PC.
2. Enable SPI (for LoRa): `sudo raspi-config` → Interface Options → SPI → Yes.
3. Wire the SX1262 (BCM numbering): SCK GPIO11 (pin 23), MOSI GPIO10 (pin 19), MISO GPIO9 (pin 21), **NSS GPIO25 (pin 22)**, RESET GPIO22 (pin 15), BUSY GPIO23 (pin 16), DIO1 GPIO24 (pin 18), 3V3 pin 17, GND pin 20.
4. Build on your PC (needs Docker for `cross`):

```bash
cargo install cross --locked
```

```bash
cross build --release --target aarch64-unknown-linux-gnu -p hub-node --features lora
```

5. Copy it over and install:

```bash
scp target/aarch64-unknown-linux-gnu/release/hub-node hub-node/hub.example.toml deploy/ffa-hub.service pi@ffa-hub.local:~
```

On the Pi:

```bash
sudo useradd -r -G spi,gpio ffa; sudo install hub-node /usr/local/bin/; sudo mkdir -p /etc/ffa-hub /var/lib/ffa-hub; sudo chown ffa /var/lib/ffa-hub; sudo cp hub.example.toml /etc/ffa-hub/hub.toml; sudo nano /etc/ffa-hub/hub.toml
```

Set `pc_addr` to your PC's IP, `pc_public_key` to the PC's public key
(start `pc-daemon` first; the key is in its log and on the app's Pairing
page), and `lora.enabled = true`. The hub will only ever talk to that PC.
Then create the hub identity:

```bash
sudo -u ffa hub-node --config /etc/ffa-hub/hub.toml init
```

**Write the 24 words on paper.** The words are shown once and never saved.
The private key made from them *is* saved in `/var/lib/ffa-hub/identity.key`
so the hub can start by itself, so keep the SD card safe. Note the
**Hub public key** it prints: that's `FFA_HUB_KEY` for the nodes (you can
see it again with `hub-node fingerprint`). Then start the service:

```bash
sudo cp ffa-hub.service /etc/systemd/system/ && sudo systemctl enable --now ffa-hub
```

The hub keeps readings in RAM and writes them to the SD card **once a day**
(and when the service stops). It keeps 30 days. Pairings are saved right away.

## Setting up the PC

```bash
cargo build --release -p pc-daemon
```

```bash
./target/release/pc-daemon
```

Leave port **7400** open to the Pi on your firewall (the API on 7401 stays localhost-only).
For running it at login on Linux, see `deploy/ffa-daemon.service`.

The app needs [Node.js 20+](https://nodejs.org):

```bash
cd frontend && npm install && npm run dev
```

To build an installer (AppImage / .exe / .dmg):

```bash
cd frontend && npm run dist
```

## Pairing

Pairing is how devices learn to trust each other's keys. It's the only step
where a **person** decides, so check fingerprints carefully.

1. **Hub → PC (once):** with `pc_public_key` set in hub.toml and both the
   daemon and hub running, open **Pairing** in the app. "Hub (Raspberry Pi)"
   appears. Check that its fingerprint matches `hub-node fingerprint` on the
   Pi, then click **Approve**.
2. **Node → hub:** with the node running, **press and hold BOOT for 3 seconds**
   (not while plugging it in; that starts the ESP32's flashing mode). For
   60 seconds the node asks to pair. In the app, check that the fingerprint
   matches the node's serial log, give it a name, and click **Approve**.
   The node logs `PAIRED as node 1 with hub 23C8-…`. Check that this hub
   fingerprint matches the one in the app's sidebar.
3. **Unpair a node:** Devices → Unpair in the app (the hub must be online), *and* hold BOOT on the node for 10 seconds.
   Unpairing also deletes the node's automation rules. Its id is never given to another node.

## Using the app

- **Dashboard:** live values for each node and On/Off buttons for outputs. Nodes report every 10 s.
- **History:** a chart per channel over 1 h / 24 h / 7 d / 30 d.
- **Automation:** rules such as *"if Greenhouse / Ch 0 (temperature) is above 30 °C, turn Siren ON, and back OFF when it clears"*.
- **Alerts:** sensor faults (e.g. a DS18B20 that stopped answering) and rules that fired.
- **Devices:** rename nodes and channels, see how many key exchanges each node has done.

**Safety:** if a node hears nothing from the hub for 5 minutes, it turns
**all outputs off** (so a solenoid can't stay open forever).

## Adding your own sensors and outputs

Edit the channel list in `data-node/src/main.rs`:

```rust
chans.ds18b20(0, p.GPIO4);              // thermometer
chans.analog_in(1, photo);              // any ADC1 pin (GPIO0-4), millivolts
chans.digital_out(2, p.GPIO5);          // siren
chans.digital_out(3, p.GPIO6);          // solenoid
chans.digital_in(4, p.GPIO20, Pull::Up); // e.g. float switch to GND
```

Up to 7 channels per node. New channels show up in the app by themselves (the node announces them after it boots).

## If the hub dies: recovery phrase

Set up a new SD card, install hub-node as above, then:

```bash
sudo -u ffa hub-node --config /etc/ffa-hub/hub.toml restore
```

(On a Pi that already has an identity, add `--force`. Only do this if you
mean to replace it: nodes paired with the old key stop working.)

Type the 24 words. You get the same hub identity back, so the nodes keep
working. Copy `paired.json` from a backup of `/var/lib/ffa-hub`, or re-pair the
nodes. You also need to re-approve the hub on the PC if the PC lost its database.

## Troubleshooting

| Problem | Fix |
|---|---|
| App says "Daemon offline" | Start `pc-daemon`. The app reads its token from `~/.config/ffa-daemon/api-token`. |
| Hub never shows up for pairing | Check `pc_addr` in hub.toml, and that the PC firewall allows TCP 7400 from the Pi. |
| Node says `wifi connect failed` | Check SSID/password (2.4 GHz only; the C3 has no 5 GHz). Rebuild after changing env vars. |
| Node keeps sending HELLO, no session | Check `FFA_HUB_IP`. Was the node unpaired on the hub? Hold BOOT 10 s, then pair again. |
| `SX1262 init failed` | Check wiring, especially BUSY/RESET. Ebyte E22 modules need `tcxo = true` (hub) / `tcxo: true` in `setup_lora` (node). |
| Temperature missing / "Sensor fault" alert | DS18B20 needs the 4.7 kΩ pull-up between DATA and 3V3. |
| Readings stop when the PC is off | That's expected. The hub stores them (30 days) and sends the backlog when the PC comes back. |
| Hub log: "answered with a key that is not pc_public_key" | `pc_public_key` in hub.toml doesn't match this PC. Copy it again from the app's Pairing page. |
| Node log: "saved pairing is for a different hub than FFA_HUB_KEY" | The node was paired to another hub before. Pair it again. |
| Command shows "dropped" | A LoRa node was out of reach long enough for 16+ commands to queue. Send it again once the node reports in. |
