# iroh on ESP32 (SPIRAM) + dual DHT22

An iroh endpoint running on an ESP32-WROVER (4 MB flash, 4 MB PSRAM) that also
reads two DHT22 temperature/humidity sensors and mirrors them to an optional
I2C character LCD.

This variant is the `server-esp32-spiram` base (relay + n0 pkarr discovery, **no
mDNS**) with a bit-banged dual-DHT22 sensor loop added. It serves an **irpc**
`GetLatest` RPC over `SENSOR_ALPN` (the most recent record) alongside the
original **echo** protocol, and mirrors readings to serial + the LCD.

## Crate layout (3 crates, intentionally not a workspace)

The protocol is split out so a host GUI can share it; only this binary carries the
`[patch.crates-io]`, so the three can't be a single workspace:

- [`dht22-proto`](../dht22-proto) — board-agnostic shared types: `Record`,
  `Reading`, the irpc `SensorProtocol`, and `SENSOR_ALPN`. Builds on host and xtensa.
- `server-esp32-spiram-dht22` (this crate) — the esp32 binary. Carries the iroh +
  irpc/irpc-iroh patches that make the stack ring-free on Xtensa.
- [`dht22-gui`](../dht22-gui) — host GUI client (placeholder, not implemented yet).

It targets `xtensa-esp32-espidf` (ESP32 / LX6) and depends on a custom iroh
branch (`refactor-hickory`).

## Wiring

| Pin | Function | Notes |
|-----|----------|-------|
| GPIO27 | Inside DHT22 data | open-drain + internal pull-up (add 4.7k–10k to 3.3V for reliability) |
| GPIO26 | Outside DHT22 data | same |
| GPIO13 | I2C SDA (LCD) | PCF8574 backpack @ 0x27 |
| GPIO14 | I2C SCL (LCD) | |
| GPIO25 | Fan output (LED for now) | HIGH = on; LED + ~220–330 Ω → GND |

Both DHT22s: VCC → **3.3V** (not 5V — the data line swings to VCC), GND → GND.
The LCD is **optional** — if it doesn't initialize, the firmware logs a warning
and keeps reading sensors. The fan state is computed from the inside-vs-outside
humidity advantage (hysteresis) and driven on **GPIO25** — wire an LED there for
now; a real fan/relay can replace it later. No actual relay is driven yet.

## Build / run

An ESP32 device must be connected over USB while running the server. Flashing
and the serial monitor are handled by `espflash` (configured as the cargo
runner in `.cargo/config.toml`).

```bash
ESPFLASH_BAUD=230400 WIFI_CONFIG='SSID:PASSWORD' cargo run --release
```

`WIFI_CONFIG` is read at build time and embedded into the firmware (SSID and
password separated by a colon). `ESPFLASH_BAUD=230400` keeps flashing reliable
over cheap USB-to-UART bridges.

On startup the device prints an endpoint ticket to the serial monitor. Pass that
ticket to the [`client`](../client/README.md) to dial the echo protocol. The
sensor readings appear as `Inside:`/`Outside:` log lines every couple of
seconds.
