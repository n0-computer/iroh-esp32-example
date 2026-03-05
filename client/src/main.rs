//! Vanilla iroh echo client — 100% released crates, zero patches.
//!
//! This client uses iroh from crates.io (with ring crypto provider).
//! The ESP32 server uses rustls-rustcrypto. Both speak standard QUIC.
//!
//! Usage:
//!     cargo run -- <endpoint-ticket>

use iroh::Endpoint;
use iroh_tickets::endpoint::EndpointTicket;

const ECHO_ALPN: &[u8] = b"echo/0";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("Failed to install ring crypto provider");

    let ticket_str = std::env::args()
        .nth(1)
        .expect("usage: esp32-echo-client <endpoint-ticket>");

    let ticket: EndpointTicket = ticket_str.parse()?;
    let addr: iroh::EndpointAddr = ticket.into();

    let endpoint = Endpoint::builder().bind().await?;

    println!("Connecting to ESP32...");
    let conn = endpoint.connect(addr, ECHO_ALPN).await?;
    println!("Connected!");

    let (mut send, mut recv) = conn.open_bi().await?;
    let msg = b"Hello from vanilla iroh (crates.io)!";
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
    println!("Echo OK — vanilla crates.io iroh <-> ESP32!");

    conn.close(0u32.into(), b"done");
    endpoint.close().await;

    Ok(())
}
