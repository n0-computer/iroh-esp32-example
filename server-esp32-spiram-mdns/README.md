# iroh on ESP32 (SPIRAM) with mDNS local discovery

![An M5StickC PLUS2](../images/m5stickc-plus2.jpg)

An iroh endpoint running on an ESP32 with SPIRAM that adds **mDNS** (multicast
DNS) local discovery *alongside* the usual relay + pkarr DNS discovery. Use this
variant for boards that have SPIRAM (PSRAM), e.g. an ESP32-WROVER (frequently
included in ESP32 dev kits) or the M5StickC PLUS2.

This is the SPIRAM baseline plus mDNS — the comfortable target, so mDNS is the
only new variable. SPIRAM has the heap to spare, so it keeps the full discovery
stack and does not carry the no-SPIRAM survival hacks (heap instrumentation,
stack-buffer echo, squeezed QUIC transport windows).

## What mDNS adds

- **Local multicast discovery.** The endpoint advertises its node ID and
  addresses over mDNS on the LAN and resolves peers by node ID — just UDP
  multicast via `swarm-discovery`, no server involved. This runs in addition to
  pkarr/n0 DNS publishing and the n0 relay.
- **The SHORT ticket is dialable on the LAN.** Peers resolve the address over
  mDNS, so the node-id-only ticket works locally. Off-LAN, the relay + pkarr path
  still provides NAT traversal and reachability. (The long ticket still embeds the
  IP+port for discovery-free direct dialing.)

It targets `xtensa-esp32-espidf` (ESP32 / LX6). The mDNS address lookup
(`iroh-mdns-address-lookup`) and `swarm-discovery` are currently pulled in via
local `[patch.crates-io]` overrides — see the comments in `Cargo.toml` for why
(Xtensa cross-compile and esp-idf `SO_REUSEPORT` fixes that are not yet
published upstream).

## Build / run

An ESP32 device must be connected over USB-C while running the server. Flashing
and the serial monitor are handled by `espflash` (configured as the cargo
runner in `.cargo/config.toml`).

```bash
ESPFLASH_BAUD=230400 WIFI_CONFIG='SSID:PASSWORD' cargo run --release
```

`WIFI_CONFIG` is read at build time and embedded into the firmware; it is the
SSID and password of the WiFi network the device should join, separated by a
colon. These boards flash over an external USB-to-UART bridge, so
`ESPFLASH_BAUD=230400` keeps flashing reliable (cheap bridges often can't
sustain the higher default rates). The S3 variants omit it — they flash over
native USB-Serial/JTAG, where the baud is virtual and ignored.

On startup the device prints an endpoint ticket to the serial monitor. Pass
that ticket to the [`client`](../client/README.md) to dial it. On the same LAN
the client reaches it directly via mDNS; off-LAN it falls back to the relay.
