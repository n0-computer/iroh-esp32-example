//! iroh echo client — 100% released crates, zero patches.
//!
//! This client uses iroh from crates.io (with ring crypto provider).
//! The ESP32 server uses rustls-rustcrypto. Both speak standard QUIC.
//!
//! Usage:
//!     cargo run -- <endpoint-ticket>

use anyhow::Context;
use iroh::endpoint::presets;
use iroh::{Endpoint, RelayMode, SecretKey};
use iroh_mdns_address_lookup::MdnsAddressLookup;
use iroh_tickets::endpoint::EndpointTicket;

const ECHO_ALPN: &[u8] = b"echo/0";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("Failed to install ring crypto provider");

    // Client identity. Set IROH_SECRET (hex) to keep a stable EndpointId across
    // runs; otherwise generate one and print it so it can be reused. A stable
    // identity lets us reconnect as the *same* peer (e.g. to tell per-remote
    // accumulation on the server apart from per-connection behaviour).
    let secret_key = match std::env::var("IROH_SECRET") {
        Ok(s) => s
            .parse::<SecretKey>()
            .context("IROH_SECRET must be a hex-encoded iroh secret key")?,
        Err(_) => {
            let s = SecretKey::generate();
            let hex: String = s.to_bytes().iter().map(|b| format!("{b:02x}")).collect();
            println!("Generated a new client secret. To reuse, set:");
            println!("\tIROH_SECRET={hex}");
            s
        }
    };
    println!("Client EndpointId: {}", secret_key.public());

    // Flags: relay and mDNS are independent and both default ON. The remaining
    // positional argument is the endpoint ticket.
    let mut no_relay = false;
    let mut no_mdns = false;
    let mut ticket_str = None;
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--no-relay" => no_relay = true,
            "--no-mdns" => no_mdns = true,
            flag if flag.starts_with("--") => anyhow::bail!("unknown flag: {flag}"),
            positional => {
                if ticket_str.replace(positional.to_string()).is_some() {
                    anyhow::bail!("expected a single endpoint ticket");
                }
            }
        }
    }
    let ticket_str = ticket_str
        .context("usage: esp32-echo-client [--no-relay] [--no-mdns] <endpoint-ticket>")?;

    let ticket: EndpointTicket = ticket_str.parse()?;
    let addr: iroh::EndpointAddr = ticket.into();

    // Build discovery from the flags. Relay on => the n0 preset (relay + pkarr DNS).
    // Relay off => the Minimal preset + RelayMode::Disabled, which also drops pkarr —
    // so `--no-relay` (with mDNS still on) forces LAN-only mDNS resolution, the clean
    // way to prove mDNS actually did the work. mDNS is layered on top either way.
    let mut builder = if no_relay {
        Endpoint::builder(presets::Minimal).relay_mode(RelayMode::Disabled)
    } else {
        Endpoint::builder(presets::N0)
    };
    builder = builder.secret_key(secret_key);
    if !no_mdns {
        builder = builder.address_lookup(MdnsAddressLookup::builder());
    }
    println!(
        "Discovery: relay={}, mdns={}",
        if no_relay { "off" } else { "on" },
        if no_mdns { "off" } else { "on" },
    );

    let endpoint = builder.bind().await?;

    println!("Connecting to ESP32...");
    let conn = endpoint.connect(addr, ECHO_ALPN).await?;
    println!("Connected!");

    let (mut send, mut recv) = conn.open_bi().await?;
    let msg = b"Hello from iroh (crates.io)!";
    send.write_all(msg).await?;
    send.finish()?;
    println!("Sent: {}", String::from_utf8_lossy(msg));

    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        match recv.read(&mut chunk).await? {
            Some(n) => buf.extend_from_slice(&chunk[..n]),
            None => break,
        }
    }
    println!("Received: {}", String::from_utf8_lossy(&buf));

    assert_eq!(buf, msg, "echo mismatch!");
    println!("Echo OK — crates.io iroh <-> ESP32!");

    conn.close(0u32.into(), b"done");
    endpoint.close().await;

    Ok(())
}
