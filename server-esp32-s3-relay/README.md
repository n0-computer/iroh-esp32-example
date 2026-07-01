# iroh on ESP32-S3 — relay-only (experimental)

An iroh endpoint on an ESP32-S3 that is reachable **only through a relay**, with
no direct IP transport advertised. It is the counterpart to
[`server-esp32-s3`](../server-esp32-s3/README.md) (which is LAN-direct, relay
disabled): same board and tuning, but the networking model is flipped.

> ⚠️ **Experimental.** The other variants stripped the relay client to save RAM;
> this one adds it back. The relay path pulls in more code and memory (and a TLS
> connection to the relay), so on a no-SPIRAM S3 it may be tight or may not fit
> at all — that's the point of the experiment. It also depends on the minimal
> crypto provider being able to complete the relay's TLS handshake. If the board
> can't reach a relay it will block at `Waiting for home relay...`.

## How it differs from `server-esp32-s3`

- `relay_mode` is a `RelayMode::Custom` map with a **single** relay (the EU prod
  relay, hard-coded by URL) instead of `Disabled`. It is deliberately *not*
  `RelayMode::Default`: that hands net_report all 4 prod relays, and the reportgen
  actor probes them concurrently to pick the closest — racing all 4 at once
  overflowed the heap on the no-SPIRAM S3 (a ~12 KB allocation aborted). One
  relay = no race = it fits. Change the host in `main.rs` for your region.
- The IP transport is **kept** (not `clear_ip_transports()`), and "relay-only" is
  achieved by advertising only the relay URL in the ticket. This is a deliberate
  trade-off forced by memory: relay *selection* needs a net_report probe, and the
  only two are QAD (lightweight QUIC, but requires an IP transport — iroh gates it
  on `has_ip_transports`) and the HTTPS probe (which drags in the full reqwest/hyper
  client, ~85 KB → OOM on the no-SPIRAM S3). Clearing the IP transport disables QAD
  and *forces* the reqwest probe, so on this chip it's actually cheaper to keep the
  IP transport and let QAD pick the relay. A truly transport-less relay-only build
  would need net_report skipped entirely (pin the home relay) — an iroh-side change.
- After bind it waits for `endpoint.online()` so the home relay is assigned (via QAD).
- The ticket is built from `endpoint.addr()` with direct IP addresses filtered out
  (`TransportAddr::is_relay()`), so only the **relay URL** is advertised — the client
  reaches the device through the relay even though the IP transport exists locally.

## Build / run

Targets `xtensa-esp32s3-espidf`. Connect the board over USB while running.

```bash
WIFI_CONFIG='SSID:PASSWORD' cargo run --release
```

`WIFI_CONFIG` (SSID:PASSWORD) is embedded at build time. On startup the device
joins WiFi, registers with a relay, and prints a **relay ticket** to the serial
monitor — hand that to the [`client`](../client/README.md), which dials the
endpoint over the relay (no need to be on the same LAN, unlike the direct
variants).

## Heap logging

Like the other variants, it logs `[heap] before endpoint setup`, `[heap] after
bind`, and per-connection free heap / largest contiguous block — useful here to
see exactly how much the relay client costs versus the direct-only S3.
