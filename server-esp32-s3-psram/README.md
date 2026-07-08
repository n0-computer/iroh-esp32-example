# iroh on ESP32-S3 (with PSRAM)

![A bare ESP32-S3 development board](../images/esp32-s3.jpg)

An iroh endpoint running on an ESP32-S3 **with** PSRAM. Like the
[`server-esp32-psram`](../server-esp32-psram/README.md) variant, the PSRAM gives
it enough RAM for the full feature set — **relay + pkarr discovery** — so it is
reachable internet-wide (not just LAN-direct like the no-PSRAM S3 build).

It targets `xtensa-esp32s3-espidf` (ESP32-S3 / LX7) and uses **published iroh**
(crates.io `1.0.2`) — no git branch, no `[patch]`. That keeps hickory-resolver in
the graph (compiled but never run — `src/std_dns_resolver.rs` swaps it out at
runtime), which the S3's flash and SRAM comfortably absorb.

> **Needs a ≥8 MB-flash board.** The app is ~4.35 MB, so it won't fit a **4 MB**
> S3 (N4R2 / N4R8). 8 MB (N8R2 / N8R8) or 16 MB (N16R8) is required — see the
> `CONFIG_ESPTOOLPY_FLASHSIZE_8MB` floor in `sdkconfig.defaults`. On a 4 MB S3+PSRAM
> board, use a slimmed iroh build (relay/hickory removed) instead.

This variant is configured for **octal (8MB) PSRAM** (`CONFIG_SPIRAM_MODE_OCT`,
e.g. an ESP32-S3-WROOM-1-N16R8). For a quad/2MB board (N8R2) drop the
`CONFIG_SPIRAM_MODE_OCT=y` line in `sdkconfig.defaults` — quad is the ESP-IDF
default.

## Build / run

An ESP32-S3 device must be connected over USB-C while running the server.
Flashing and the serial monitor are handled by `espflash` (configured as the
cargo runner in `.cargo/config.toml`).

```bash
WIFI_CONFIG='SSID:PASSWORD' cargo run --release
```

`WIFI_CONFIG` is read at build time and embedded into the firmware; it is the
SSID and password of the WiFi network the device should join, separated by a
colon. The S3 flashes over native USB-Serial/JTAG, so no `ESPFLASH_BAUD` is
needed (unlike the LX6 PSRAM board, which flashes over an external USB-to-UART
bridge).

On startup the device prints an endpoint ticket to the serial monitor. Pass
that ticket to the [`client`](../client/README.md) to dial it.
