//! Native CLI client for the ESP32 dual-DHT22 sensor logger.
//!
//! Dials the esp32 endpoint over `dht22_proto::SENSOR_ALPN`. By default it polls
//! `GetLatest` every 2 s and prints the most recent record; `--once` does a single
//! call; `--range <start[..end]>` streams the buffered log over that offset range
//! (the `GetLog` server-streaming RPC) and exits. A GUI can grow on this data path.
//!
//! Usage:
//!     cargo run -- <endpoint-ticket>                  # poll latest every 2s
//!     cargo run -- --once <endpoint-ticket>           # latest, once
//!     cargo run -- --range 600 <endpoint-ticket>      # from id 600 to newest
//!     cargo run -- --range 600..700 <endpoint-ticket> # ids 600..=700

use std::time::Duration;

use anyhow::Context;
use dht22_proto::{GetLatest, GetLog, Reading, Record, SensorProtocol, SENSOR_ALPN};
use iroh::endpoint::presets;
use iroh::{Endpoint, SecretKey};
use iroh_tickets::endpoint::EndpointTicket;
use irpc::Client;
use irpc_iroh::IrohRemoteConnection;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("Failed to install ring crypto provider");

    // Optional stable client identity (same convention as the echo client).
    let secret_key = match std::env::var("IROH_SECRET") {
        Ok(s) => s
            .parse::<SecretKey>()
            .context("IROH_SECRET must be a hex-encoded iroh secret key")?,
        Err(_) => SecretKey::generate(),
    };

    // Args: optional --once / --range <start[..end]>, then the endpoint ticket.
    let mut once = false;
    let mut range: Option<(u64, Option<u64>)> = None;
    let mut ticket_str: Option<String> = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--once" {
            once = true;
        } else if arg == "--range" {
            let v = args.next().context("--range needs <start[..end]>")?;
            range = Some(parse_range(&v)?);
        } else if arg.starts_with("--") {
            anyhow::bail!("unknown flag: {arg}");
        } else if ticket_str.replace(arg).is_some() {
            anyhow::bail!("expected a single endpoint ticket");
        }
    }
    let ticket_str = ticket_str
        .context("usage: dht22-cli [--once] [--range <start[..end]>] <endpoint-ticket>")?;
    let ticket: EndpointTicket = ticket_str.parse()?;
    let addr: iroh::EndpointAddr = ticket.into();

    // Relay + pkarr (matches the server's discovery; no mDNS).
    let endpoint = Endpoint::builder(presets::N0)
        .secret_key(secret_key)
        .bind()
        .await?;

    if let Some((start_id, end_id)) = range {
        // Stream the buffered log over the offset range and exit. The server
        // streams from start_id onward; we apply the optional end bound here and
        // stop draining (which closes the stream, so the server stops sending).
        match end_id {
            Some(end) => println!("Fetching log range id {start_id}..={end}..."),
            None => println!("Fetching log from id {start_id}..."),
        }
        let client = connect(&endpoint, &addr).await?;
        let mut rx = client
            .server_streaming(GetLog { start_id, limit: u32::MAX }, 64)
            .await?;
        let mut count = 0u64;
        while let Some(record) = rx.recv().await? {
            if end_id.is_some_and(|end| record.id > end) {
                break;
            }
            println!("{}", format_record(&record));
            count += 1;
        }
        println!("({count} records)");
    } else {
        // Live reporting via incremental GetLog: seed with the current latest,
        // then each poll fetches only records newer than the last one seen. This
        // dedupes (no id printed twice) and never misses records produced between
        // polls. If the device restarts (ids reset to 0), the new latest id is
        // below what we've seen — detect that and re-seed.
        println!("Querying sensor endpoint (Ctrl-C to stop)...");
        let mut last_id: Option<u64> = None;
        let mut backoff = 0u32;
        // One long-lived connection, reused across polls. On a timeout/error we drop
        // it and manually dial a fresh one (no lazy auto-reconnect). This avoids the
        // handshake-per-poll storm that was eating the device's internal RAM.
        let mut client: Option<Client<SensorProtocol>> = None;
        loop {
            // (Re)connect if we have no live connection (startup, or after a failure).
            if client.is_none() {
                match connect(&endpoint, &addr).await {
                    Ok(c) => client = Some(c),
                    Err(e) => {
                        eprintln!("connect error: {e}");
                        backoff = (backoff + 1).min(5);
                        tokio::time::sleep(Duration::from_secs(2 * backoff as u64)).await;
                        continue;
                    }
                }
            }
            let client_ref = client.as_ref().unwrap();

            let mut failed = false;
            match last_id {
                None => match client_ref.rpc(GetLatest).await {
                    Ok(Some(r)) => {
                        println!("{}", format_record(&r));
                        last_id = Some(r.id);
                    }
                    Ok(None) => {} // no readings yet; retry next tick
                    Err(e) => {
                        eprintln!("rpc error: {e}");
                        failed = true;
                    }
                },
                Some(seen) => match client_ref
                    .server_streaming(GetLog { start_id: seen + 1, limit: u32::MAX }, 64)
                    .await
                {
                    Ok(mut rx) => {
                        let mut got_any = false;
                        loop {
                            match rx.recv().await {
                                Ok(Some(r)) => {
                                    println!("{}", format_record(&r));
                                    last_id = Some(r.id);
                                    got_any = true;
                                }
                                Ok(None) => break,
                                Err(e) => {
                                    eprintln!("stream error: {e}");
                                    failed = true;
                                    break;
                                }
                            }
                        }
                        // No new records and no error → maybe device restarted.
                        if !got_any && !failed {
                            if let Ok(Some(r)) = client_ref.rpc(GetLatest).await {
                                if r.id < seen {
                                    println!("(device restarted)");
                                    println!("{}", format_record(&r));
                                    last_id = Some(r.id);
                                }
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("rpc error: {e}");
                        failed = true;
                    }
                },
            }

            if once {
                break;
            }
            if failed {
                // Drop the dead connection so the next iteration dials a fresh one,
                // and back off (2,4,…,10 s) before retrying.
                client = None;
                backoff = (backoff + 1).min(5);
                tokio::time::sleep(Duration::from_secs(2 * backoff as u64)).await;
            } else {
                backoff = 0;
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        }
    }

    endpoint.close().await;
    Ok(())
}

/// Dial one QUIC connection to the sensor endpoint over `SENSOR_ALPN` and wrap it
/// as an irpc client. The caller holds it long-lived and re-calls this to dial a
/// fresh connection after a failure — a manual reconnect, deliberately not the lazy
/// auto-reconnecting connection, so reconnects happen at most once per failure
/// rather than opaquely.
async fn connect(
    endpoint: &Endpoint,
    addr: &iroh::EndpointAddr,
) -> anyhow::Result<Client<SensorProtocol>> {
    let conn = endpoint.connect(addr.clone(), SENSOR_ALPN).await?;
    Ok(Client::boxed(IrohRemoteConnection::new(conn)))
}

/// Parse a `--range` spec into `(start, end?)` offsets: `"600"` or `"600.."` →
/// from 600 to newest; `"600..700"` → ids 600 through 700 inclusive.
fn parse_range(spec: &str) -> anyhow::Result<(u64, Option<u64>)> {
    match spec.split_once("..") {
        Some((a, b)) => {
            let start = a.parse().context("range start must be a u64")?;
            let end = if b.is_empty() {
                None
            } else {
                Some(b.parse().context("range end must be a u64")?)
            };
            if let Some(end) = end {
                anyhow::ensure!(end >= start, "range end must be >= start");
            }
            Ok((start, end))
        }
        None => Ok((spec.parse().context("range start must be a u64")?, None)),
    }
}

fn format_record(r: &Record) -> String {
    format!(
        "[id {:>6} t={:>10}] inside: {}  outside: {}  fan: {}",
        r.id,
        r.time,
        format_reading(&r.inside),
        format_reading(&r.outside),
        if r.fan { "on" } else { "off" },
    )
}

fn format_reading(reading: &Option<Reading>) -> String {
    match reading {
        Some(r) => format!("{:5.1}°C {:4.1}%", r.temperature, r.humidity),
        None => "    --      ".to_string(),
    }
}
