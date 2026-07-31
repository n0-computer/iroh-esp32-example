# iroh on an ESP32-P4 (RISC-V, no radio — loopback self-test)

> [!WARNING]
> **This variant is a starting point, not a usable iroh node.** The ESP32-P4 has
> no radio, and this project wires up **no connectivity at all** — the endpoint
> talks only to itself over `127.0.0.1`. Nothing can dial it, and it can dial
> nothing. It exists to prove the iroh stack builds and runs on P4 silicon
> (toolchain, QUIC, TLS, echo — all exercised by the on-chip loopback
> self-test), as groundwork for a real transport later (Ethernet PHY,
> ESP-Hosted). If you want an ESP32 iroh node you can actually dial today, use
> one of the WiFi variants in this repo instead.

An iroh endpoint running on an ESP32-P4 — Espressif's big dual-core 32-bit
**RISC-V** (RV32IMAFC, 400 MHz) application chip with **768 KB internal SRAM**
and (on most modules, in-package) 16/32 MB PSRAM. It targets
`riscv32imafc-esp-espidf` — its own rust target, because unlike the C6/C61
(RV32IMAC) the P4 has a single-precision FPU.

The catch: the P4 has **no radio at all** — no WiFi, no Bluetooth. So how do you
test a networking library on it?

**By dialing ourselves.** On startup the firmware binds the usual iroh echo
server, then brings up a *second* iroh endpoint on the same chip and dials the
server over `127.0.0.1` through lwIP's loopback netif. That exercises the entire
iroh stack — UDP sockets, QUIC, the TLS handshake with the pure-Rust
`rustls-rustcrypto` provider, stream open, echo, clean close — everything except
a physical network:

```
I (…) server_esp32_p4: [self-test] dialing 127.0.0.1:…
I (…) server_esp32_p4: Accepted connection from … [heap free=33729124 largest_8bit=33030144]
I (…) server_esp32_p4: [self-test] PASSED: 57 bytes echoed over QUIC via loopback
```

(Verified on real hardware: an ESP32-P4 **rev v1.3** with 16 MB flash and 32 MB
in-package PSRAM — the `heap free=33.7 MB` above is that PSRAM. On a P4, "no
memory chips on the board" means nothing: flash is a tiny part on the PCB back
and PSRAM is inside the chip package; `ESP32-P4NRW16/NRW32` markings mean 16/32 MB.)

No `WIFI_CONFIG` needed — the build takes no credentials.

## What changed from the C61

Based on [`server-esp32-c61-psram`](../server-esp32-c61-psram/README.md) (the
other newest-tooling RISC-V variant; same ESP-IDF v5.5.4, esp-idf-svc 0.52,
espflash 4.4+):

- **`.cargo/config.toml`** — target `riscv32imafc-esp-espidf` (FPU), `MCU=esp32p4`.
- **`src/main.rs`** — WiFi is *gone*, replaced by a bare `esp_netif_init()` (just
  to start lwIP's tcpip thread and its loopback netif). SNTP is gone too: there
  is no network to sync from, and nothing needs the clock — peer-to-peer iroh
  TLS uses raw public keys, and with `RelayMode::Disabled` there's no web-PKI
  connection either. Relay and pkarr discovery are off (no route to reach them);
  the new `loopback_self_test` dials the echo server from a second endpoint.
- **`sdkconfig.defaults`** — all `CONFIG_ESP_WIFI_*` symbols removed (they don't
  exist for this chip). `CONFIG_LWIP_NETIF_LOOPBACK=y` made explicit. PSRAM on,
  but with `CONFIG_SPIRAM_IGNORE_NOTFOUND=y` so a PSRAM-less P4 still boots —
  768 KB internal SRAM is plenty for this relay-less build. Main-task stack
  stays 96 KB: the self-test nests both sides of the QUIC/TLS handshake on the
  one tokio `current_thread` stack (measured high-water: ~50 KB used).

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

## Build / run

The P4 has a built-in USB Serial/JTAG peripheral — flash over that port (the
board's second USB-C is the P4's USB-OTG controller: it enumerates nothing
unless firmware implements a USB device, so it's no use for flashing):

```bash
cargo run --release
```

Optionally bake in a stable identity: `IROH_SECRET=<64 hex chars> cargo run --release`.

**Post-flash quirk:** after flashing, the chip may come back up with
`boot:0x4 (DOWNLOAD…)` / `waiting for download` instead of booting the app —
the "force download" latch espflash uses to talk to the ROM can survive the
soft reset (seen with espflash 4.4.0). Press **Ctrl+R** in the espflash monitor
(or the board's RESET button) and it boots normally
(`boot:0xc SPI_FAST_FLASH_BOOT`).

If the port refuses to open (`Failed to open serial port`), check for a stale
espflash still holding it: `lsof /dev/cu.usbmodem*`.

## Getting it on a real network

The self-test proves iroh runs; making the board *dialable* needs a netif from
somewhere. The P4's options, none wired up here (yet):

- **Ethernet** — the P4 has an internal 10/100 EMAC; boards like the
  ESP32-P4-Function-EV-Board pair it with an IP101 PHY. This is the natural next
  step: `EspEth` + the existing code path would give LAN-direct echo, and with
  the PSRAM headroom probably the full relay + pkarr build.
- **ESP-Hosted** — many P4 boards (Function-EV, Waveshare P4-NANO, …) carry an
  ESP32-C6 companion chip that provides WiFi over SDIO via the ESP-Hosted
  protocol. Supported in ESP-IDF; not yet surfaced nicely in `esp-idf-svc`.
- **USB** — USB-ECM/RNDIS to a host. Exotic, but the P4 has USB-OTG HS.

Once any of those provides a netif, the endpoint (already running, already
accepting) becomes reachable — add the netif's IP to a long ticket, or flip
`RelayMode` back on for internet-wide reachability.

## Memory tuning

Same instrumentation as the siblings: per-connection heap probes, and a
main-task **stack high-water** line after each connection — during the
self-test that covers both the client and server side of the handshake, so it's
a realistic worst case. See the
[C61 README](../server-esp32-c61-psram/README.md#memory-tuning) for how to read
it. The P4 is the least memory-constrained chip in this repo; if anything, the
numbers tell you how much slack you have.

### Profiling the heap

The [`src/alloc_log.rs`](src/alloc_log.rs) logging allocator is present but not
hooked up by default. Decode backtraces with the **RISC-V** toolchain addr2line:

```
riscv32-esp-elf-addr2line -e target/riscv32imafc-esp-espidf/release/server-esp32-p4 <pc>…
```
