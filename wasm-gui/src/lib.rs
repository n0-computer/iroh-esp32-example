//! Browser GUI logic: spawns an iroh endpoint compiled to WebAssembly and exposes
//! its endpoint ID to JavaScript.
//!
//! Structure mirrors the `browser-echo` example in iroh-examples — a
//! `#[wasm_bindgen(start)]` init hook plus an exported `Node` type that JS drives.
//! Right now `Node` only spawns and reports its ID; the echo connect/accept methods
//! are the next step.
//!
//! Browsers cannot open raw UDP/QUIC sockets, so this endpoint reaches peers
//! exclusively over a **relay** (the `N0` preset wires up the n0 relays). That means
//! it interoperates only with the relay-capable (PSRAM) ESP32 variants.

use iroh_tickets::endpoint::EndpointTicket;
use tracing::level_filters::LevelFilter;
use tracing_subscriber_wasm::MakeConsoleWriter;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::future_to_promise;

/// The echo protocol's ALPN — identical to the CLI client and the ESP32 servers.
const ECHO_ALPN: &[u8] = b"echo/0";

/// Runs once, automatically, when the wasm module is initialized in the browser.
#[wasm_bindgen(start)]
fn start() {
    // Readable panics in the console instead of an opaque wasm trap.
    console_error_panic_hook::set_once();

    // Route `tracing` (including iroh's logs) to the browser console.
    tracing_subscriber::fmt()
        .with_max_level(LevelFilter::INFO)
        .with_writer(
            // Map TRACE down so the browser doesn't attach a JS backtrace to every line.
            MakeConsoleWriter::default().map_trace_level_to(tracing::Level::DEBUG),
        )
        .without_time() // wall-clock time isn't available the usual way in wasm
        .with_ansi(false)
        .init();

    tracing::info!("wasm-gui initialized");
}

/// An iroh endpoint running in the browser.
#[wasm_bindgen]
pub struct Node {
    endpoint: iroh::Endpoint,
    /// Hex-encoded secret key, so JS can persist it (localStorage) for a stable
    /// identity across reloads. Stashed at spawn so we don't depend on the
    /// endpoint exposing its key.
    secret_hex: String,
}

#[wasm_bindgen]
impl Node {
    /// Spawn the endpoint. Uses the `N0` preset (n0 relays + discovery); since the
    /// browser has no direct sockets, connectivity is relay-only.
    ///
    /// `secret` is an optional hex-encoded iroh secret key. Pass the one persisted
    /// from a previous session to keep a stable endpoint ID; pass `null`/`undefined`
    /// to generate a fresh identity (then read it back via [`Node::secret_hex`]).
    pub async fn spawn(secret: Option<String>) -> Result<Node, JsError> {
        let secret_key = match secret.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            Some(hex) => hex.parse::<iroh::SecretKey>().map_err(js_err)?,
            None => iroh::SecretKey::generate(),
        };
        let secret_hex = hex_encode(&secret_key.to_bytes());

        let endpoint = iroh::Endpoint::builder(iroh::endpoint::presets::N0)
            .secret_key(secret_key)
            .bind()
            .await
            .map_err(js_err)?;
        tracing::info!(id = %endpoint.id(), "endpoint bound");
        Ok(Node {
            endpoint,
            secret_hex,
        })
    }

    /// The endpoint's ID (its public key). Available immediately after `spawn` —
    /// it's derived from the secret key, not from any relay connection.
    pub fn endpoint_id(&self) -> String {
        self.endpoint.id().to_string()
    }

    /// The endpoint's secret key, hex-encoded — persist this to reuse the same
    /// identity next time. Treat it like a private key: anyone with it can
    /// impersonate this endpoint.
    pub fn secret_hex(&self) -> String {
        self.secret_hex.clone()
    }

    /// Connect to an echo server via its endpoint ticket, send `payload`, and
    /// resolve the returned promise with the echoed text.
    ///
    /// This speaks the same `echo/0` protocol as the CLI client and the ESP32
    /// servers — it's the browser equivalent of `cargo run -- <ticket>`. The
    /// ticket must carry a relay URL (i.e. come from a relay-capable / PSRAM
    /// server), since the browser has no direct sockets.
    ///
    /// Returns a `Promise<string>` rather than being an `async fn`: an exported
    /// async method can't borrow `&self` across an await, so we clone the
    /// endpoint and drive the future to a JS promise.
    pub fn connect(&self, ticket: String, payload: String) -> js_sys::Promise {
        let endpoint = self.endpoint.clone();
        future_to_promise(async move {
            match echo_once(endpoint, ticket, payload).await {
                Ok(echoed) => Ok(JsValue::from_str(&echoed)),
                Err(err) => Err(err.into()),
            }
        })
    }
}

/// Run one echo round-trip: connect, open a bi stream, send the payload, then
/// read the echoed bytes back until the server closes its send side.
async fn echo_once(
    endpoint: iroh::Endpoint,
    ticket: String,
    payload: String,
) -> Result<String, JsError> {
    let ticket: EndpointTicket = ticket.trim().parse().map_err(js_err)?;
    let addr: iroh::EndpointAddr = ticket.into();

    tracing::info!("connecting…");
    let conn = endpoint.connect(addr, ECHO_ALPN).await.map_err(js_err)?;
    let (mut send, mut recv) = conn.open_bi().await.map_err(js_err)?;

    send.write_all(payload.as_bytes()).await.map_err(js_err)?;
    send.finish().map_err(js_err)?;
    tracing::info!(bytes = payload.len(), "sent payload");

    // Read until the server finishes its send side (mirrors the CLI client).
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    while let Some(n) = recv.read(&mut chunk).await.map_err(js_err)? {
        buf.extend_from_slice(&chunk[..n]);
    }
    conn.close(0u32.into(), b"done");
    tracing::info!(bytes = buf.len(), "received echo");

    String::from_utf8(buf).map_err(js_err)
}

/// Wrap any `Display` error as a `JsError` for the JS boundary.
fn js_err(err: impl std::fmt::Display) -> JsError {
    JsError::new(&err.to_string())
}

/// Lowercase hex encoding (matches the `IROH_SECRET` format the CLI client uses).
fn hex_encode(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}
