//! Shared protocol for the ESP32 dual-DHT22 sensor logger.
//!
//! This crate defines the wire types and the irpc service surface used by both
//! the esp32 server binary and (eventually) the GUI client. It is deliberately
//! board-agnostic: no esp-idf dependencies and no `[patch]`, so it builds on the
//! host as well as on `xtensa-esp32-espidf`.

use irpc::channel::{mpsc, oneshot};
use irpc::rpc_requests;
use serde::{Deserialize, Serialize};

/// The ALPN for the sensor RPC protocol.
pub const SENSOR_ALPN: &[u8] = b"iroh-esp32/sensor/0";

/// A single sensor reading (temperature in °C, relative humidity in %).
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Reading {
    pub temperature: f32,
    pub humidity: f32,
}

/// One logged sample. `id` is a monotonic offset assigned on append: it keeps
/// increasing even as old records are dropped from the front of the buffer, so a
/// client can query by `id` (or an id range) against whatever is still retained.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Record {
    pub id: u64,
    /// Unix seconds (from the SNTP-synced clock); 0 if the clock isn't set yet.
    pub time: u64,
    pub inside: Option<Reading>,
    pub outside: Option<Reading>,
    /// Whether the cooling fan would be on (computed from the inside-vs-outside
    /// humidity advantage). Compute-only for now — no GPIO is actually driven.
    pub fan: bool,
}

/// Request the most recent record. Returns `None` until the first sample lands.
#[derive(Debug, Serialize, Deserialize)]
pub struct GetLatest;

/// Request a range of records, streamed oldest-first: all retained records with
/// `id >= start_id`, up to `limit` of them. If the client's `start_id` is older
/// than what's still retained, the stream simply begins at the oldest record —
/// the gap is visible because the first streamed `id` is greater than `start_id`.
#[derive(Debug, Serialize, Deserialize)]
pub struct GetLog {
    pub start_id: u64,
    pub limit: u32,
}

/// The sensor RPC service. `rpc_requests` generates the [`SensorMessage`] enum
/// (the wire/channel-carrying form) consumed by the server handler.
#[rpc_requests(message = SensorMessage)]
#[derive(Debug, Serialize, Deserialize)]
pub enum SensorProtocol {
    #[rpc(tx = oneshot::Sender<Option<Record>>)]
    GetLatest(GetLatest),
    /// Server-streaming: the device streams matching records out one at a time
    /// rather than allocating a large `Vec` (it has little RAM).
    #[rpc(tx = mpsc::Sender<Record>)]
    GetLog(GetLog),
}
