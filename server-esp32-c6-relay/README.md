# iroh on an ESP32-C6 with relay (no discovery)

An iroh endpoint on the ESP32-C6 (RISC-V) reachable **over an n0 relay** — so it
works across NATs, not just LAN-direct like [`server-esp32-c6`](../server-esp32-c6/README.md).
It pins a **single** relay (not all four) so the connection is dialed via the
**long/relay ticket** (which carries the relay URL).

> **No short-ticket discovery.** We tried full relay + pkarr discovery first; it
> doesn't work here — and not (only) for RAM reasons. pkarr publish to n0 DNS is
> **real-cert HTTPS**, and this firmware uses a deliberately minimal crypto provider
> (X25519 + AES-128-GCM, no RSA, `skip_verify`) that can't do it — it errored as
> `pkarr_publish` on hardware. Short-ticket discovery is a **crypto-provider wall**,
> independent of memory, and needs the full-crypto SPIRAM target. This variant is
> the salvageable half: relay connectivity without discovery.

## Why single-relay, and what it cost

`RelayMode::Default` probes all 4 production relays concurrently in `net_report`
reportgen — and racing them blew the heap on the no-PSRAM C6 (`memory allocation of
43 bytes failed → abort`), the same failure the no-SPIRAM S3 documented. Pinning one
relay (`RelayMode::Custom`, EU by default) removes the race and it fits.

The C6 is actually a *better* host for relay than the no-SPIRAM S3: the S3 had to
needle its stack to 88 KB because the rustls deframer couldn't find an 8 KB
**contiguous** heap block at higher stack sizes. The C6's unified, far-less-fragmented
SRAM makes that block easy, so it runs a comfortable 100 KB stack with heap to spare.

## Build / run

The C6 has built-in USB Serial/JTAG — connect over USB directly.

```bash
WIFI_CONFIG='SSID:PASSWORD' cargo run --release
```

It prints both tickets. Dial with the **long ticket** (it carries the relay URL).
The **short ticket won't resolve** — no discovery (see above). Change the relay
region in [`src/main.rs`](src/main.rs) (`euc1-1` → `use1-1` / `usw1-1` / `aps1-1`).

## Differences from `server-esp32-c6` (LAN-direct)

- **iroh deps**: `refactor-hickory` branch (relay), vs the `esp32-no-spiram` branch
  with relay stripped.
- **Endpoint** (`src/main.rs`): `RelayMode::Custom([one relay])` + `max_tls_tickets(0)`,
  vs `RelayMode::Disabled`. No pkarr.
- **Stack**: 100 KB (relay TLS handshake) vs 48 KB.
- **Buffers**: no-spiram floor (same as the LAN-direct C6), tuned so the relay
  handshake's heap fits.

## Tuning

If it OOMs during relay setup, lower `CONFIG_ESP_MAIN_TASK_STACK_SIZE`
([`sdkconfig.defaults`](sdkconfig.defaults)) to return SRAM to the heap; if a
handshake path stack-overflows (watchpoint trap), raise it. Watch the per-connection
log (free heap, largest contiguous block, stack high-water) to find the balance.

## Identity

`IROH_SECRET` is baked at build time (env var if set, else generated and cached in
`OUT_DIR`; see [`build.rs`](build.rs)) — stable endpoint ID across reboots.
