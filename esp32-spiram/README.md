# iroh on ESP32 (with SPIRAM)

![An M5StickC PLUS2](../images/m5stickc-plus2.jpg)

An iroh endpoint running on an ESP32 with SPIRAM. Use this variant for boards
that have SPIRAM (PSRAM), e.g. an ESP32-WROVER that is frequently included in
ESP32 dev kits, or the M5StickC PLUS2.

It targets `xtensa-esp32-espidf` (ESP32 / LX6) and depends on a custom iroh
branch (`refactor-hickory`).

## Build / run

An ESP32 device must be connected over USB-C while running the server. Flashing
and the serial monitor are handled by `espflash` (configured as the cargo
runner in `.cargo/config.toml`).

```bash
WIFI_CONFIG='SSID:PASSWORD' cargo run --release
```

`WIFI_CONFIG` is read at build time and embedded into the firmware; it is the
SSID and password of the WiFi network the device should join, separated by a
colon.

On startup the device prints an endpoint ticket to the serial monitor. Pass
that ticket to the [`client`](../client/README.md) to dial it.
