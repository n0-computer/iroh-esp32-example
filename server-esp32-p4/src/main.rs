use iroh::endpoint::Connection;
use iroh::protocol::{AcceptError, ProtocolHandler, Router};
use iroh::SecretKey;
use iroh::tls::CaTlsConfig;
use log::info;

// Allocation-profiling allocator. Kept in-repo but NOT hooked up by default — it
// logs a backtrace per allocation via blocking ROM printf, which starves the app
// under load (throughput craters, the client times out). To profile, uncomment
// the `#[global_allocator]` below (and tune `alloc_log::THRESHOLD`), and run
// without `--monitor` so the serial console doesn't throttle it.
#[allow(dead_code)]
mod alloc_log;
mod std_dns_resolver;
mod quic_crypto_provider;
mod insecure_verifier;

// #[global_allocator]
// static GLOBAL_ALLOC: alloc_log::LoggingAlloc = alloc_log::LoggingAlloc;

/// The ALPN for the echo protocol
const ECHO_ALPN: &[u8] = b"echo/0";

/// Optional: bake in a fixed secret key so the node ID is stable across reboots.
/// Set via: IROH_SECRET=<64 hex chars or base32> cargo build
const IROH_SECRET: Option<&str> = option_env!("IROH_SECRET");

// NOTE: no WIFI_CONFIG here — the ESP32-P4 has no radio. The build needs no
// credentials at all; the self-test runs entirely over lwIP loopback.

fn parse_secret_key() -> Option<SecretKey> {
    let s = IROH_SECRET?;
    Some(
        s.parse()
            .expect("IROH_SECRET must be valid hex (64 chars) or base32"),
    )
}

// ESP-IDF doesn't provide gethostname, but resolv_conf (via hickory-resolver) references it.
#[no_mangle]
unsafe extern "C" fn gethostname(name: *mut core::ffi::c_char, len: usize) -> core::ffi::c_int {
    if len > 0 && !name.is_null() {
        unsafe {
            *name = 0;
        }
    }
    0
}

/// Echo protocol handler
#[derive(Debug, Clone)]
struct Echo;

impl ProtocolHandler for Echo {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let endpoint_id = connection.remote_id();
        // Per-connection heap probe: a leak shows as `free` dropping each
        // connection; fragmentation shows as `free` ~steady but `largest_8bit`
        // shrinking (no contiguous block left).
        info!(
            "Accepted connection from {endpoint_id} [heap free={} largest_8bit={}]",
            unsafe { esp_idf_svc::sys::esp_get_free_heap_size() },
            unsafe {
                esp_idf_svc::sys::heap_caps_get_largest_free_block(
                    esp_idf_svc::sys::MALLOC_CAP_8BIT,
                )
            },
        );

        let (mut send, mut recv) = connection.accept_bi().await?;
        info!("Got bidi stream");

        // Echo with a 1 KiB STACK buffer instead of tokio::io::copy, whose 8 KiB
        // heap buffer OOM'd on the 2nd connection on the bare-LX6 boards. A stack
        // buffer puts zero pressure on the heap on the data path.
        let mut buf = [0u8; 1024];
        let mut total: u64 = 0;
        loop {
            let n = AsyncReadExt::read(&mut recv, &mut buf).await?;
            if n == 0 {
                break;
            }
            AsyncWriteExt::write_all(&mut send, &buf[..n]).await?;
            total += n as u64;
        }
        info!("Copied over {total} byte(s)");

        send.finish()?;

        connection.closed().await;
        info!("Connection closed");

        // Stack high-water: minimum free bytes ever seen on this (the main) task's
        // stack. tokio current_thread runs every async task here — during the
        // self-test that includes BOTH sides of the QUIC/TLS handshake — so this
        // is the deepest the whole stack ever got. Whatever's left is headroom
        // that could be trimmed from CONFIG_ESP_MAIN_TASK_STACK_SIZE.
        let stack_total = esp_idf_svc::sys::CONFIG_ESP_MAIN_TASK_STACK_SIZE;
        let stack_free = unsafe {
            esp_idf_svc::sys::uxTaskGetStackHighWaterMark(core::ptr::null_mut())
        };
        info!("[stack] main task high-water: {stack_free} bytes free of {stack_total}");

        Ok(())
    }
}

/// QUIC transport config shared by both self-test endpoints. The defaults size
/// flow-control windows for internet throughput (MB-scale per connection) —
/// shrink hard: one bidi stream (echo only), tiny windows, no datagrams.
fn small_transport_config() -> iroh::endpoint::QuicTransportConfig {
    use iroh::endpoint::{QuicTransportConfig, VarInt};
    QuicTransportConfig::builder()
        .max_concurrent_bidi_streams(VarInt::from_u32(1))
        .max_concurrent_uni_streams(VarInt::from_u32(0))
        .stream_receive_window(VarInt::from_u32(4 * 1024))
        .receive_window(VarInt::from_u32(8 * 1024))
        .send_window(8 * 1024)
        .datagram_receive_buffer_size(None)
        .build()
}

fn endpoint_builder(
    provider: std::sync::Arc<rustls::crypto::CryptoProvider>,
) -> iroh::endpoint::Builder {
    iroh::Endpoint::builder(iroh::endpoint::presets::Empty)
        .crypto_provider(provider)
        .ca_tls_config(CaTlsConfig::custom_server_cert_verifier(
            insecure_verifier::skip_verify(),
        ))
        .dns_resolver(iroh::dns::DnsResolver::custom(
            std_dns_resolver::StdDnsResolver,
        ))
        .transport_config(small_transport_config())
        // Kill the TLS client session-resumption cache (see the C61 variant for
        // the full story) — we never resume on this embedded server.
        .max_tls_tickets(0)
        // No radio → no route to a relay or to pkarr. Explicitly disable the
        // relay and configure no address lookup at all. Peer-to-peer TLS uses
        // raw public keys (no web-PKI, no wall-clock validity check), which is
        // also why this build needs no SNTP: there is no network to sync from,
        // and nothing that checks the clock.
        .relay_mode(iroh::RelayMode::Disabled)
}

/// Dial our own echo server over 127.0.0.1 with a SECOND iroh endpoint on the
/// same chip. No radio needed: this still exercises the full stack — UDP via
/// lwIP's loopback netif, QUIC, TLS handshake with the rustcrypto provider,
/// stream open, echo, clean close — i.e. everything except a physical network.
async fn loopback_self_test(
    provider: std::sync::Arc<rustls::crypto::CryptoProvider>,
    server_id: iroh::EndpointId,
    port: u16,
) {
    info!("[self-test] binding client endpoint...");
    let client = endpoint_builder(provider)
        .bind()
        .await
        .expect("self-test: unable to bind client endpoint");

    let mut server_addr = iroh::EndpointAddr::new(server_id);
    server_addr
        .addrs
        .insert(iroh::TransportAddr::Ip(std::net::SocketAddr::new(
            std::net::Ipv4Addr::LOCALHOST.into(),
            port,
        )));

    info!("[self-test] dialing 127.0.0.1:{port}...");
    let conn = client
        .connect(server_addr, ECHO_ALPN)
        .await
        .expect("self-test: connect over loopback failed");
    info!("[self-test] connected");

    let (mut send, mut recv) = conn
        .open_bi()
        .await
        .expect("self-test: open_bi failed");
    let msg = b"Hello from iroh on an ESP32-P4 - no radio, just loopback!";
    send.write_all(msg).await.expect("self-test: write failed");
    send.finish().expect("self-test: finish failed");

    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        match recv.read(&mut chunk).await.expect("self-test: read failed") {
            Some(n) => buf.extend_from_slice(&chunk[..n]),
            None => break,
        }
    }
    assert_eq!(buf, msg, "self-test: echo mismatch!");

    conn.close(0u32.into(), b"done");
    client.close().await;
    info!(
        "[self-test] PASSED: {} bytes echoed over QUIC via loopback",
        msg.len()
    );
}

fn main() {
    // It is necessary to call this function once. Otherwise, some patches to the runtime
    // implemented by esp-idf-sys might not link properly. See https://github.com/esp-rs/esp-idf-template/issues/71
    esp_idf_svc::sys::link_patches();

    // Bind the log crate to the ESP Logging facilities
    esp_idf_svc::log::EspLogger::initialize_default();

    // Register eventfd VFS — needed by mio's poll implementation which powers tokio I/O
    let eventfd_config = esp_idf_svc::sys::esp_vfs_eventfd_config_t {
        max_fds: 5,
        ..Default::default()
    };
    unsafe { esp_idf_svc::sys::esp_vfs_eventfd_register(&eventfd_config) };

    // Start the network stack WITHOUT any network hardware: esp_netif_init spins
    // up lwIP's tcpip thread, and with CONFIG_LWIP_NETIF_LOOPBACK=y that gives
    // us a working 127.0.0.1 — all the self-test needs. (On the WiFi variants
    // EspWifi does this implicitly; the P4 has no modem peripheral to take.)
    esp_idf_svc::sys::esp!(unsafe { esp_idf_svc::sys::esp_netif_init() })
        .expect("esp_netif_init failed");

    // Pure-Rust crypto provider with minimal QUIC support
    let provider = std::sync::Arc::new(quic_crypto_provider::provider());

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(1)
        .thread_stack_size(4096)
        .build()
        .expect("Failed to create tokio runtime");

    rt.block_on(async {
        info!("[heap] before endpoint setup: free={} largest_8bit={}", unsafe {
            esp_idf_svc::sys::esp_get_free_heap_size()
        }, unsafe {
            esp_idf_svc::sys::heap_caps_get_largest_free_block(esp_idf_svc::sys::MALLOC_CAP_8BIT)
        });

        let mut builder = endpoint_builder(provider.clone());
        if let Some(key) = parse_secret_key() {
            builder = builder.secret_key(key);
        }
        let endpoint = builder.bind().await.expect("unable to bind endpoint");

        info!(
            "[heap] after bind: free={} largest_8bit={}",
            unsafe { esp_idf_svc::sys::esp_get_free_heap_size() },
            unsafe {
                esp_idf_svc::sys::heap_caps_get_largest_free_block(
                    esp_idf_svc::sys::MALLOC_CAP_8BIT,
                )
            },
        );

        let endpoint_id = endpoint.addr().id;
        let port = endpoint
            .bound_sockets()
            .first()
            .map(|s| s.port())
            .expect("no bound socket");

        let _router = Router::builder(endpoint).accept(ECHO_ALPN, Echo).spawn();

        info!("Iroh endpoint bound");
        info!("  Listening on: 127.0.0.1:{port} (loopback only — no radio)");
        info!("  Endpoint ID: {endpoint_id}");
        info!("Router started, accepting connections");

        // Prove the whole thing works with no network hardware at all.
        loopback_self_test(provider, endpoint_id, port).await;

        // Keep the router running indefinitely — if the board ever grows a
        // netif (Ethernet PHY, ESP-Hosted companion), it becomes dialable.
        loop {
            tokio::time::sleep(tokio::time::Duration::from_secs(3600)).await;
        }
    });
}
