# iroh on an ESP32-P4 (RISC-V, WiFi via an ESP32-C6 companion, relay + discovery)

An iroh endpoint running on an ESP32-P4 — Espressif's big dual-core 32-bit
**RISC-V** (RV32IMAFC, 400 MHz) application chip with **768 KB internal SRAM**
and (on most modules, in-package) 16/32 MB PSRAM. It targets
`riscv32imafc-esp-espidf` — its own rust target, because unlike the C6/C61
(RV32IMAC) the P4 has a single-precision FPU.

The catch: the P4 itself has **no radio at all** — no WiFi, no Bluetooth. Boards
solve this with a companion chip: this project was built and tested on a
Waveshare **ESP32-P4-NANO**, which pairs the P4 with an **ESP32-C6-MINI-1**
module (the metal can on the back) over SDIO. The C6 runs Espressif's
**ESP-Hosted** slave firmware (factory-flashed) and acts as a WiFi 6 network
card; the `esp_wifi_remote` + `esp_hosted` components on our side proxy the
standard `esp_wifi_*` API to it. `esp-idf-hal`/`esp-idf-svc` detect those
components and light up the normal `Modem` peripheral + `EspWifi` wrapper — so
[`src/main.rs`](src/main.rs) looks almost exactly like the native-WiFi variants.

With WiFi up, the P4 runs the **full relay + pkarr discovery** build (like the
C61/S3/ESP32 PSRAM targets) — reachable across networks via the short ticket.
With 32 MB of in-package PSRAM it is by far the roomiest chip in this repo
(`heap free=33.7 MB` after boot; verified on a rev v1.3 chip, 16 MB flash).

> If your P4 board has no companion radio chip, this variant won't get online —
> but iroh itself still runs: an earlier loopback-only revision of this project
> (see git history) proved the full QUIC/TLS stack against itself over
> `127.0.0.1` with zero network hardware.

## What changed from the C61

Based on [`server-esp32-c61-psram`](../server-esp32-c61-psram/README.md) (same
ESP-IDF v5.5.4, esp-idf-svc 0.52, espflash 4.4+):

- **`.cargo/config.toml`** — target `riscv32imafc-esp-espidf` (FPU), `MCU=esp32p4`.
- **`Cargo.toml`** — two extra managed components for esp-idf-sys:
  `espressif/esp_wifi_remote` and `espressif/esp_hosted`. That's the entire
  WiFi-through-a-companion-chip story; the ESP-Hosted defaults already match
  the board's wiring (SDIO CLK 18, CMD 19, D0–D3 14–17, C6 reset GPIO 54).
- **`src/main.rs`** — WiFi connect code is unchanged from the C61. New: tokio +
  iroh run on a **thread with a 96 KB PSRAM stack** (see below) instead of the
  main task.
- **`sdkconfig.defaults`** — the chip-revision pin (see the trap below), PSRAM
  with `IGNORE_NOTFOUND`, a small (24 KB) main-task stack, ESP-Hosted's
  mempool moved to PSRAM, and no WiFi buffer tuning at all — the buffers live
  on the C6, and the P4 side has RAM to spare.

## The chip-revision trap (read this if you get an illegal-instruction boot loop)

ESP-IDF v5.5.4's **default minimum chip revision for the P4 is v3.01**, but the
dev boards in the wild (2026) are **rev v1.x** — and this is not a benign
compatibility knob, it changes the **memory layout**. Rev ≥ 3 chips don't
reserve the ROM shared-buffer region, so the app links its RAM straight through
the address where the (rev < 3) 2nd-stage bootloader keeps its loader code. The
bootloader then **overwrites itself while copying the app's RAM segments** and
you get:

```
I (…) esp_image: segment 6: paddr=… vaddr=4ff2f100 … load
Guru Meditation Error: Core 0 panic'ed (Illegal instruction)
```

— a boot loop crashing before a single line of app output, with a register dump
full of misleading symbol names. The fix (already in
[`sdkconfig.defaults`](sdkconfig.defaults)) is **both** of:

```
CONFIG_ESP32P4_SELECTS_REV_LESS_V3=y
CONFIG_ESP32P4_REV_MIN_100=y
```

Both lines matter: `REV_MIN_100` *depends on* the `SELECTS_REV_LESS_V3` gate,
and without the gate the kconfig tooling drops the line **silently** and keeps
the v3.01 default. Also note: if you change `sdkconfig.defaults` in an already
built tree, delete the generated config first
(`rm target/*/release/build/esp-idf-sys-*/out/sdkconfig`) and `touch
sdkconfig.defaults` — defaults are only applied when the generated sdkconfig is
created fresh.

## Internal SRAM: why the tokio stack lives in PSRAM

The 768 KB of internal SRAM is less than it sounds on a rev v1.x chip: 128 KB
goes to L2 cache, ~79 KB is reserved for ROM data + the bootloader loader
region (the rev<3 layout), and the rest is split into a **~183 KB low region**
(where static data lands) and a high window. With `esp_hosted` +
`esp_wifi_remote` linked in, static data eats ~100 KB of the low region — and a
96 KB *internal* main-task stack no longer fits: boot dies in
`esp_startup_start_app` with `assert failed: … (res == pdTRUE)` (FreeRTOS
couldn't allocate the main task).

So this variant uses the escape hatch the C61 README only described:

- `CONFIG_ESP_MAIN_TASK_STACK_SIZE=24576` — the main task only does init +
  WiFi connect + thread spawn.
- tokio + the whole iroh stack run on a thread whose **96 KB stack is
  allocated in PSRAM** (`ThreadSpawnConfiguration { stack_alloc_caps:
  Spiram | Cap8bit }`, legal because
  `CONFIG_SPIRAM_ALLOW_STACK_EXTERNAL_MEMORY=y`).
- The one rule for PSRAM stacks: the thread must never run cache-disabling
  flash ops (NVS/flash writes, OTA). NVS + WiFi init happen on the main task
  before the spawn; the iroh path does no flash writes.
- ESP-Hosted's buffer mempool also goes to PSRAM
  (`CONFIG_ESP_HOSTED_MEMPOOL_PREFER_SPIRAM=y`) — internal SRAM is left for
  FreeRTOS objects and SDIO DMA, the things that genuinely need it.

## Build / run

The P4 has a built-in USB Serial/JTAG peripheral — flash over that port (the
board's second USB-C is the P4's USB-OTG controller: it enumerates nothing
unless firmware implements a USB device, so it's no use for flashing):

```bash
WIFI_CONFIG='SSID:PASSWORD' cargo run --release
```

`WIFI_CONFIG` is read at build time and embedded into the firmware. Optionally
bake in a stable identity: `IROH_SECRET=<64 hex chars>`.

On startup the device prints two tickets. Pass either to the
[`client`](../client/README.md):

- **Short ticket** — bare endpoint ID, resolved via pkarr/relay. Works from any
  network. Use this one.
- **Long ticket** — includes the WiFi IP, for a same-LAN direct fast path.

**Post-flash quirk:** after flashing, the chip may come back up with
`boot:0x4 (DOWNLOAD…)` / `waiting for download` instead of booting the app —
the "force download" latch espflash uses to talk to the ROM can survive the
soft reset (seen with espflash 4.4.0). Press **Ctrl+R** in the espflash monitor
(or the board's RESET button) and it boots normally
(`boot:0xc SPI_FAST_FLASH_BOOT`).

If the port refuses to open (`Failed to open serial port`), check for a stale
espflash still holding it: `lsof /dev/cu.usbmodem*`.

**Benign log noise:** two `E (…) system_api: N mac type is incorrect (not
found)` lines at startup are expected — the netif layer first looks for WiFi
MAC addresses in the P4's eFuse (there are none; no radio), then ESP-Hosted
supplies the C6's real MAC over SDIO.

## Memory tuning

Same instrumentation as the siblings: per-connection heap probes, and a
**stack high-water** line after each connection — now for the tokio thread's
PSRAM stack (`[stack] tokio thread high-water: … bytes free of 98304`). See the
[C61 README](../server-esp32-c61-psram/README.md#memory-tuning) for how to read
it. With 32 MB of PSRAM the P4 is the least memory-constrained target in this
repo; the numbers mostly tell you how much slack you have.

### Profiling the heap

The [`src/alloc_log.rs`](src/alloc_log.rs) logging allocator is present but not
hooked up by default. Decode backtraces with the **RISC-V** toolchain addr2line:

```
riscv32-esp-elf-addr2line -e target/riscv32imafc-esp-espidf/release/server-esp32-p4 <pc>…
```
