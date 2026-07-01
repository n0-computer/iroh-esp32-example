# iroh echo + WiFi scan on the M5StickC PLUS2

A variant of [`server-esp32-spiram`](../server-esp32-spiram/README.md) targeted
at the **M5StickC PLUS2**: it runs the iroh echo endpoint **and** a periodic WiFi
scan in parallel, and renders a live **channel-occupancy view on the built-in
ST7789 display**.

The scan loop runs on its own thread, scanning every 10 seconds. For each access
point it prints (over serial) signal strength (dBm), channel, BSSID, auth method,
and SSID (`<hidden>` for non-broadcast), strongest-first. On the display it shows
a compact summary: total AP/channel counts, then the busiest channels with their
AP count and strongest signal.

The original point still holds: see whether the iroh QUIC connection survives the
single 2.4 GHz radio briefly hopping off-channel during each scan.

It targets `xtensa-esp32-espidf` (ESP32 / LX6 — the PLUS2's ESP32-PICO) and
depends on a custom iroh branch (`refactor-hickory`).

## Display

ST7789 (135×240) over SPI2, using `mipidsi` + `embedded-graphics`, reusing the
proven M5 setup. M5StickC PLUS2 pins: MOSI=15, SCLK=13, DC=14, RST=12, CS=5,
backlight=27. The **HOLD pin (GPIO4) is asserted high at boot** so the board
stays powered when running on battery. Display init failure is non-fatal — the
scanner and echo endpoint still run headless.

## Build / run

An ESP32 device must be connected over USB-C while running. Flashing and the
serial monitor are handled by `espflash` (configured as the cargo runner in
`.cargo/config.toml`). `WIFI_CONFIG` is required here — iroh needs a real
network connection:

```bash
ESPFLASH_BAUD=230400 WIFI_CONFIG='SSID:PASSWORD' cargo run --release
```

`WIFI_CONFIG` is read at build time and embedded into the firmware (SSID and
password separated by a colon). `ESPFLASH_BAUD=230400` keeps flashing reliable
over the cheap USB-to-UART bridges these boards use.

On startup the device prints an endpoint ticket to the serial monitor. Pass
that ticket to the [`client`](../client/README.md) to dial it, then watch the
serial log to confirm echoes still complete across scan cycles.
