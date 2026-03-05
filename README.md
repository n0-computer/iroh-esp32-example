# esp32-blog-post

This repository now contains two completely separate Rust projects (no Cargo workspace):

- `server/` — ESP32 endpoint project (esp-idf + iroh espidf-support branch)
- `client/` — Desktop test client (published crates.io iroh)

Each directory has its own `Cargo.toml` and lockfile.

## Build / run

### Server (ESP32)

An ESP32 device must be connected (e.g. over USB serial) when running the server.

```bash
cd server
WIFI_CONFIG='SSID:PASSWORD' cargo run --release
```

### Client (desktop)

```bash
cd client
cargo run -- <endpoint-ticket>
```

Use a long ticket (with IP address) for direct dialing.
