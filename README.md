# Example projects for using iroh on an ESP32

This repository contains several completely separate Rust projects (no Cargo workspace):

- [`esp32-spiram/`](esp32-spiram/README.md) — ESP32 endpoint project for boards with SPIRAM (using custom iroh branch)
- [`esp32-s3/`](esp32-s3/README.md) — ESP32-S3 endpoint project tuned to run without SPIRAM
- [`esp32-no-spiram/`](esp32-no-spiram/README.md) — bare ESP32 (LX6) endpoint project tuned to run without SPIRAM
- [`client/`](client/README.md) — Desktop test client (published crates.io iroh)

Each directory has its own `Cargo.toml` and lockfile, and its own README with
detailed build and run instructions.

## Hardware variants

| [![M5StickC PLUS2](images/m5stickc-plus2.jpg)](esp32-spiram/README.md) | [![Bare ESP32 (LX6)](images/esp32.jpg)](esp32-no-spiram/README.md) | [![Bare ESP32-S3](images/esp32-s3.jpg)](esp32-s3/README.md) |
| :---: | :---: | :---: |
| **M5StickC PLUS2 / ESP32-WROVER**<br>(with SPIRAM) | **Bare ESP32** (LX6)<br>(no SPIRAM) | **Bare ESP32-S3**<br>(no SPIRAM) |
| [`esp32-spiram/`](esp32-spiram/README.md) | [`esp32-no-spiram/`](esp32-no-spiram/README.md) | [`esp32-s3/`](esp32-s3/README.md) |

## Which server variant?

- Board has SPIRAM (e.g. an ESP32-WROVER, or the M5StickC PLUS2)? Use
  [`esp32-spiram/`](esp32-spiram/README.md).
- Bare ESP32 (LX6) without SPIRAM? Use
  [`esp32-no-spiram/`](esp32-no-spiram/README.md).
- Bare ESP32-S3 without SPIRAM? Use
  [`esp32-s3/`](esp32-s3/README.md).

Run the [`client/`](client/README.md) on a desktop computer to dial whichever
server you flashed.

## Image credits

See [`images/CREDITS.md`](images/CREDITS.md) for image sources and licenses.
