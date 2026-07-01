//! Browser GUI logic: spawns an iroh endpoint compiled to WebAssembly and drives
//! the dual-DHT22 sensor protocol against an ESP32 server.
//!
//! This is the browser equivalent of the native [`dht22-gui`](../../dht22-gui)
//! CLI: it speaks the same `SENSOR_ALPN` irpc service (`dht22_proto`) and runs the
//! same *live* polling logic — seed with `GetLatest`, then each tick fetch only the
//! records newer than the last one seen via incremental `GetLog`, with device-restart
//! detection. On connect it also backfills a trailing window of history (one ranged
//! `GetLog`) so the GUI has something to plot immediately.
//!
//! Presentation lives entirely in JS: each `Record` is serialized and handed to a JS
//! callback as a plain object, which the page both prints as text and feeds to a
//! chart. The Rust side is just the iroh + `dht22-proto` data path.
//!
//! Browsers cannot open raw UDP/QUIC sockets, so this endpoint reaches peers
//! exclusively over a **relay** (the `N0` preset wires up the n0 relays). That means
//! it interoperates only with the relay-capable (PSRAM) ESP32 variants — i.e. the
//! `server-esp32-spiram-dht22` board, not the bare-metal / no-SPIRAM ones.

use dht22_proto::{GetLatest, GetLog, Record, SensorProtocol, SENSOR_ALPN};
use irpc::Client;
use irpc_iroh::IrohLazyRemoteConnection;
use iroh_tickets::endpoint::EndpointTicket;
use tracing::level_filters::LevelFilter;
use tracing_subscriber_wasm::MakeConsoleWriter;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::spawn_local;

/// How often the live loop polls for new records (matches the native CLI's 2s).
const POLL_INTERVAL_MS: u32 = 2000;

/// How many trailing records to backfill on connect so the chart isn't empty. The
/// server clamps this to whatever it still retains, so over-asking is harmless.
const HISTORY: u64 = 1000;

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

    /// Subscribe to live sensor records from the server identified by `ticket`.
    ///
    /// This is the browser port of the native CLI's live loop: it backfills a window
    /// of history, then polls every 2s for records newer than the last one seen
    /// (incremental `GetLog`), detecting device restarts (id reset). Each record is
    /// delivered to `on_record(record)` as a plain JS object (the serialized
    /// [`Record`]); status updates (connecting, errors) go to `on_status(text)`.
    ///
    /// The loop runs until the page is closed — there is no unsubscribe yet, so call
    /// this at most once per `Node`.
    pub fn subscribe(&self, ticket: String, on_record: js_sys::Function, on_status: js_sys::Function) {
        let endpoint = self.endpoint.clone();
        spawn_local(async move {
            if let Err(err) = run_live(endpoint, ticket, &on_record, &on_status).await {
                emit(&on_status, &format!("error: {err}"));
            }
        });
    }
}

/// The live polling loop — a near-verbatim port of `dht22-gui`'s `main` live mode.
///
/// `IrohLazyRemoteConnection` reconnects under the hood if the relay link drops, so
/// the loop itself only deals with record IDs, not connection state.
async fn run_live(
    endpoint: iroh::Endpoint,
    ticket: String,
    on_record: &js_sys::Function,
    on_status: &js_sys::Function,
) -> Result<(), String> {
    let ticket: EndpointTicket = ticket.trim().parse().map_err(|e| format!("bad ticket: {e}"))?;
    let addr: iroh::EndpointAddr = ticket.into();

    let conn = IrohLazyRemoteConnection::new(endpoint, addr, SENSOR_ALPN.to_vec());
    let client: Client<SensorProtocol> = Client::boxed(conn);

    emit(on_status, "querying sensor endpoint…");

    let mut last_id: Option<u64> = None;

    // History backfill (one-shot): find the current newest id and stream a trailing
    // window ending at it. The server clamps `start_id` to what it still retains, so
    // this gives the chart an immediate baseline without a separate RPC.
    if let Ok(Some(latest)) = client.rpc(GetLatest).await {
        let start_id = latest.id.saturating_sub(HISTORY);
        if let Ok(mut rx) = client
            .server_streaming(GetLog { start_id, limit: u32::MAX }, 64)
            .await
        {
            while let Ok(Some(r)) = rx.recv().await {
                last_id = Some(r.id);
                emit_record(on_record, &r);
            }
        }
        emit(on_status, "live ✓");
    } else {
        emit(on_status, "no readings yet — waiting…");
    }

    // Live tail: fetch only records newer than the last one seen. This dedupes (no
    // id twice) and never misses records produced between polls. If the device
    // restarts (ids reset to 0), the new latest id is below what we've seen — detect
    // that and re-seed (JS clears its chart when it sees the id go backwards).
    loop {
        match last_id {
            None => match client.rpc(GetLatest).await {
                Ok(Some(r)) => {
                    emit_record(on_record, &r);
                    last_id = Some(r.id);
                    emit(on_status, "live ✓");
                }
                Ok(None) => emit(on_status, "no readings yet — waiting…"),
                Err(e) => emit(on_status, &format!("rpc error: {e}")),
            },
            Some(seen) => match client
                .server_streaming(GetLog { start_id: seen + 1, limit: u32::MAX }, 64)
                .await
            {
                Ok(mut rx) => {
                    let mut got_any = false;
                    loop {
                        match rx.recv().await {
                            Ok(Some(r)) => {
                                emit_record(on_record, &r);
                                last_id = Some(r.id);
                                got_any = true;
                            }
                            Ok(None) => break,
                            Err(e) => {
                                emit(on_status, &format!("stream error: {e}"));
                                break;
                            }
                        }
                    }
                    // No new records → maybe the device restarted (ids reset).
                    if !got_any {
                        if let Ok(Some(r)) = client.rpc(GetLatest).await {
                            if r.id < seen {
                                emit(on_status, "(device restarted)");
                                emit_record(on_record, &r);
                                last_id = Some(r.id);
                            }
                        }
                    }
                }
                Err(e) => emit(on_status, &format!("rpc error: {e}")),
            },
        }
        gloo_timers::future::TimeoutFuture::new(POLL_INTERVAL_MS).await;
    }
}

/// Hand a record to JS as a plain object (`{ id, time, inside, outside, fan }`).
///
/// Uses the `json_compatible` serializer, not the default: the default emits ES
/// `Map`s (so `rec.id` is undefined in JS) and u64 as BigInt. json_compatible gives
/// plain objects with numeric fields — what the log + chart read directly.
fn emit_record(f: &js_sys::Function, r: &Record) {
    use serde::Serialize;
    let serializer = serde_wasm_bindgen::Serializer::json_compatible();
    match r.serialize(&serializer) {
        Ok(v) => {
            let _ = f.call1(&JsValue::NULL, &v);
        }
        Err(e) => tracing::warn!("failed to serialize record: {e}"),
    }
}

/// Invoke a JS callback with a single string argument, ignoring the return value
/// and any exception it throws (a misbehaving callback shouldn't kill the loop).
fn emit(f: &js_sys::Function, text: &str) {
    let _ = f.call1(&JsValue::NULL, &JsValue::from_str(text));
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
