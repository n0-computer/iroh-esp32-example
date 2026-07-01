# Example projects for using iroh on an ESP32

This repository contains several completely separate Rust projects (no Cargo
workspace). Each is one **server** variant — an iroh echo endpoint for a specific
ESP32 — plus two **clients** to dial them.

## Servers (one per hardware variant)

Each keeps the largest configuration that runs *reliably* on that board:

- [`server-esp32-psram/`](server-esp32-psram/README.md) — ESP32-WROVER / M5StickC
  **with PSRAM**. The only variant with enough RAM for **relay + pkarr discovery**
  (custom iroh branch).
- [`server-esp32/`](server-esp32/README.md) — bare ESP32 (LX6),
  no PSRAM. LAN-direct echo (relay doesn't fit without PSRAM).
- [`server-esp32-s3/`](server-esp32-s3/README.md) — ESP32-S3, no PSRAM. LAN-direct.
- [`server-esp32-c6/`](server-esp32-c6/README.md) — ESP32-C6, the **RISC-V** variant
  (`riscv32imac-esp-espidf`), no PSRAM. LAN-direct.

## Clients

- [`client/`](client/README.md) — desktop CLI client (stock crates.io iroh + `ring`).
- [`wasm-gui/`](wasm-gui/README.md) — browser echo client (Rust → WebAssembly).
  Relay-only, so it talks to the PSRAM server.

Each directory has its own `Cargo.toml`, lockfile, and README with build/run
instructions.

## Hardware variants

| [![M5StickC PLUS2](images/m5stickc-plus2.jpg)](server-esp32-psram/README.md) | [![Bare ESP32 (LX6)](images/esp32.jpg)](server-esp32/README.md) | [![Bare ESP32-S3](images/esp32-s3.jpg)](server-esp32-s3/README.md) | [![ESP32-C6-DevKitC-1](images/esp32-c6.png)](server-esp32-c6/README.md) |
| :---: | :---: | :---: | :---: |
| **M5StickC PLUS2 / ESP32-WROVER**<br>(with PSRAM) | **Bare ESP32** (LX6)<br>(no PSRAM) | **Bare ESP32-S3**<br>(no PSRAM) | **ESP32-C6**<br>(RISC-V, no PSRAM) |
| [`server-esp32-psram/`](server-esp32-psram/README.md) | [`server-esp32/`](server-esp32/README.md) | [`server-esp32-s3/`](server-esp32-s3/README.md) | [`server-esp32-c6/`](server-esp32-c6/README.md) |

## Which server variant?

- Board has PSRAM (ESP32-WROVER, M5StickC PLUS2)? Use
  [`server-esp32-psram/`](server-esp32-psram/README.md) — the only one that gets
  relay + discovery (internet-wide reachability).
- Bare ESP32 (LX6)? Use [`server-esp32/`](server-esp32/README.md).
- Bare ESP32-S3? Use [`server-esp32-s3/`](server-esp32-s3/README.md).
- ESP32-C6 (RISC-V)? Use [`server-esp32-c6/`](server-esp32-c6/README.md).

The no-PSRAM boards are **LAN-direct only**: dial them with the long ticket (which
carries the IP), from the same network. Relay and pkarr discovery need the RAM
headroom that only PSRAM provides — see the PSRAM variant.

Run the [`client/`](client/README.md) on a desktop to dial whichever server you
flashed (or the [`wasm-gui/`](wasm-gui/README.md) in a browser, against the PSRAM
server).
