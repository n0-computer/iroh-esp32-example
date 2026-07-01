# Example projects for using iroh on an ESP32

This repository contains several completely separate Rust projects (no Cargo workspace):

- [`server-esp32-spiram/`](server-esp32-spiram/README.md) — ESP32 endpoint project for boards with SPIRAM (using custom iroh branch)
- [`server-esp32-s3/`](server-esp32-s3/README.md) — ESP32-S3 endpoint project tuned to run without SPIRAM
- [`server-esp32-no-spiram/`](server-esp32-no-spiram/README.md) — bare ESP32 (LX6) endpoint project tuned to run without SPIRAM
- [`server-esp32-s3-relay/`](server-esp32-s3-relay/README.md) — ESP32-S3 endpoint reachable only via a relay, no direct IP (experimental)
- [`server-esp32-spiram-mdns/`](server-esp32-spiram-mdns/README.md) — SPIRAM endpoint adding mDNS local (LAN) discovery alongside relay + pkarr (experimental)
- [`client/`](client/README.md) — Desktop test client (published crates.io iroh)

Each directory has its own `Cargo.toml` and lockfile, and its own README with
detailed build and run instructions.

## Hardware variants

| [![M5StickC PLUS2](images/m5stickc-plus2.jpg)](server-esp32-spiram/README.md) | [![Bare ESP32 (LX6)](images/esp32.jpg)](server-esp32-no-spiram/README.md) | [![Bare ESP32-S3](images/esp32-s3.jpg)](server-esp32-s3/README.md) |
| :---: | :---: | :---: |
| **M5StickC PLUS2 / ESP32-WROVER**<br>(with SPIRAM) | **Bare ESP32** (LX6)<br>(no SPIRAM) | **Bare ESP32-S3**<br>(no SPIRAM) |
| [`server-esp32-spiram/`](server-esp32-spiram/README.md) | [`server-esp32-no-spiram/`](server-esp32-no-spiram/README.md) | [`server-esp32-s3/`](server-esp32-s3/README.md) |

## Which server variant?

- Board has SPIRAM (e.g. an ESP32-WROVER, or the M5StickC PLUS2)? Use
  [`server-esp32-spiram/`](server-esp32-spiram/README.md).
- Bare ESP32 (LX6) without SPIRAM? Use
  [`server-esp32-no-spiram/`](server-esp32-no-spiram/README.md).
- Bare ESP32-S3 without SPIRAM? Use
  [`server-esp32-s3/`](server-esp32-s3/README.md).
- Want the S3 reachable through a relay instead of direct on the LAN
  (experimental)? Use [`server-esp32-s3-relay/`](server-esp32-s3-relay/README.md).
- Have SPIRAM and want LAN discovery by node ID (short ticket) over mDNS in
  addition to the relay + pkarr path (experimental)? Use
  [`server-esp32-spiram-mdns/`](server-esp32-spiram-mdns/README.md).

Run the [`client/`](client/README.md) on a desktop computer to dial whichever
server you flashed.

## TODO: RISC-V variant (ESP32-C6)

All current variants are Xtensa (ESP32 LX6, ESP32-S3). The natural next target is
a **RISC-V** one — and the only really plausible single-chip candidate is the
**ESP32-C6** (`riscv32imac-esp-espidf`):

- It has WiFi (WiFi 6) on-chip and 512 KB of HP SRAM — the most RAM of the
  no-PSRAM RISC-V parts, so it has a fighting chance of fitting iroh, much like
  the [`server-esp32-s3`](server-esp32-s3/README.md).
- The other RISC-V ESP32s don't fit the bill: the ESP32-C3 is tighter (~400 KB),
  the C2 is smaller still, the H2 has no WiFi, and while the ESP32-P4 has the
  most RAM (and PSRAM), it has **no built-in WiFi** (needs a companion radio).

It would most likely reuse the no-SPIRAM tuning from the `server-esp32-s3` /
`server-esp32-no-spiram` projects. Not done yet for the simple reason that there isn't
an ESP32-C6 board lying around to test on.

## Image credits

See [`images/CREDITS.md`](images/CREDITS.md) for image sources and licenses.
