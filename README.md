# Example project for using iroh on an ESP32

This repository contains two completely separate Rust projects (no Cargo workspace):

- `server/` — ESP32 endpoint project (using custom iroh branch)
- `client/` — Desktop test client (published crates.io iroh)

Each directory has its own `Cargo.toml` and lockfile.

## Build / run

### Server (ESP32)

An ESP32 device must be connected over USB-C when running the server. You need
an ESP32 with SPIRAM to run this example, e.g. an ESP32-WROVER that is frequently
included in ESP32 dev kits.

```bash
cd server
WIFI_CONFIG='SSID:PASSWORD' cargo run --release
```

### Client

```bash
cd client
cargo run <endpoint-ticket>
```

Use a long ticket (with IP address) for direct dialing.