use core::convert::TryInto;

use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::delay::FreeRtos;
use esp_idf_svc::hal::gpio::*;
use esp_idf_svc::hal::i2c::{I2cConfig, I2cDriver};
use esp_idf_svc::hal::modem::Modem;
use esp_idf_svc::hal::peripherals::Peripherals;
use esp_idf_svc::hal::units::*;
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use esp_idf_svc::wifi::{BlockingWifi, ClientConfiguration, Configuration, EspWifi};
use iroh::endpoint::Connection;
use iroh::protocol::{AcceptError, ProtocolHandler, Router};
use iroh::tls::CaTlsConfig;
use iroh::SecretKey;
use dht22_proto::{Reading, Record, SensorMessage, SensorProtocol, SENSOR_ALPN};
use iroh_tickets::endpoint::EndpointTicket;
use irpc::WithChannels;
use irpc_iroh::read_request;
use lcd_lcm1602_i2c::sync_lcd::Lcd;
use log::{info, warn};
use std::collections::VecDeque;
use std::fmt::Write as _;
use std::sync::{Arc, Mutex};

mod insecure_verifier;
mod quic_crypto_provider;
mod std_dns_resolver;

/// The ALPN for the echo protocol
const ECHO_ALPN: &[u8] = b"echo/0";

// SENSOR_ALPN now comes from dht22_proto.

/// Optional: bake in a fixed secret key so the node ID is stable across reboots.
/// Set via: IROH_SECRET=<64 hex chars or base32> cargo build
const IROH_SECRET: Option<&str> = option_env!("IROH_SECRET");

const WIFI_CONFIG: &str = match option_env!("WIFI_CONFIG") {
    Some(value) => value,
    None => panic!("WIFI_CONFIG is not set. Build with WIFI_CONFIG='SSID:PASSWORD' cargo build"),
};

/// How often to sample the sensors. The DHT22 maxes out at 0.5 Hz (one read per
/// 2 s); we keep it fast during bring-up for quick serial feedback. Once the data
/// protocol / logging is added this becomes the 60 s logging cadence.
const SENSOR_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);

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

// ---------------------------------------------------------------------------
// DHT22 sensor (bit-banged single-wire protocol)
// ---------------------------------------------------------------------------

/// Microseconds since boot (from ESP-IDF hardware timer).
fn micros() -> i64 {
    unsafe { esp_idf_svc::sys::esp_timer_get_time() }
}

/// Busy-wait for `us` microseconds using the ESP32 ROM delay (hardware-calibrated).
fn busy_wait_us(us: u32) {
    unsafe { esp_idf_svc::sys::esp_rom_delay_us(us) }
}

/// Per-edge timeout for the DHT22 bit-bang. The longest legitimate interval is
/// ~200µs, so this is generous on purpose: long cables add capacitance and slow
/// the pull-up's rise, delaying edges, and the bit-bang can be briefly preempted
/// by WiFi/QUIC. A failed read returns at the first timed-out edge, so a large
/// value here costs little on failure.
const DHT_EDGE_TIMEOUT_US: i64 = 3_000;


// Reading and Record now come from dht22_proto.

/// Retention-windowed log of sensor records, shared between the sensor thread
/// (writer) and the iroh RPC handler (reader).
#[derive(Debug, Default)]
struct Buffer {
    records: VecDeque<Record>,
    next_id: u64,
}

/// ~24h of history at one sample per minute.
const BUFFER_CAP: usize = 1440;

impl Buffer {
    fn push(&mut self, time: u64, inside: Option<Reading>, outside: Option<Reading>, fan: bool) {
        let id = self.next_id;
        self.next_id += 1;
        self.records.push_back(Record {
            id,
            time,
            inside,
            outside,
            fan,
        });
        while self.records.len() > BUFFER_CAP {
            self.records.pop_front();
        }
    }

    fn latest(&self) -> Option<Record> {
        self.records.back().cloned()
    }

    /// Snapshot up to `limit` retained records with `id >= start_id`, oldest
    /// first. Records are stored in ascending-id order, so this is a cheap
    /// filter+take. Cloned so the caller can drop the lock before streaming.
    fn range(&self, start_id: u64, limit: usize) -> Vec<Record> {
        self.records
            .iter()
            .filter(|r| r.id >= start_id)
            .take(limit)
            .cloned()
            .collect()
    }
}

type SharedBuffer = Arc<Mutex<Buffer>>;

/// Busy-wait until the pin reaches `level` or `timeout_us` elapses.
fn wait_for(
    pin: &PinDriver<'_, impl IOPin, InputOutput>,
    level: Level,
    timeout_us: i64,
) -> Result<(), &'static str> {
    let start = micros();
    while pin.get_level() != level {
        if micros() - start > timeout_us {
            return Err("timeout");
        }
    }
    Ok(())
}

/// Read 40 bits of data from a DHT22 sensor.
///
/// The pin must be configured as open-drain input/output with a pull-up.
fn read_dht22(
    pin: &mut PinDriver<'_, impl IOPin, InputOutput>,
) -> Result<Reading, &'static str> {
    // Send start signal: pull low for ≥1 ms
    pin.set_low().map_err(|_| "set_low")?;
    busy_wait_us(3_000); // 3 ms

    // Release the line (open-drain → pull-up brings it high)
    pin.set_high().map_err(|_| "set_high")?;
    busy_wait_us(40); // give pull-up time to rise

    // DHT22 responds after 20-40 µs: pulls low ~80 µs, then high ~80 µs
    wait_for(pin, Level::Low, DHT_EDGE_TIMEOUT_US)?;
    wait_for(pin, Level::High, DHT_EDGE_TIMEOUT_US)?;
    wait_for(pin, Level::Low, DHT_EDGE_TIMEOUT_US)?;

    // Read 40 data bits
    let mut data = [0u8; 5];
    for i in 0..40 {
        // Each bit starts with ~50 µs low, then variable-length high
        wait_for(pin, Level::High, DHT_EDGE_TIMEOUT_US)?;
        let t = micros();
        wait_for(pin, Level::Low, DHT_EDGE_TIMEOUT_US)?;
        // 26-28 µs high → 0, ~70 µs high → 1
        if micros() - t > 40 {
            data[i / 8] |= 1 << (7 - (i % 8));
        }
    }

    // Verify checksum (sum of first 4 bytes, truncated to u8)
    let sum: u8 = data[..4].iter().map(|&b| b as u16).sum::<u16>() as u8;
    if sum != data[4] {
        return Err("checksum mismatch");
    }

    let humidity = ((data[0] as u16) << 8 | data[1] as u16) as f32 / 10.0;
    let raw = ((data[2] as u16 & 0x7F) << 8) | data[3] as u16;
    let temperature = if data[2] & 0x80 != 0 {
        -(raw as f32)
    } else {
        raw as f32
    } / 10.0;

    Ok(Reading {
        temperature,
        humidity,
    })
}

/// Read a DHT22, retrying once on failure. Most failures here are transient — the
/// bit-bang frame (~8 ms) gets preempted by WiFi/QUIC and an edge is missed — so a
/// single immediate re-read recovers the bulk of them. A failed read bails fast (at
/// an edge timeout), so even when the retry also fails the extra cost is only a few
/// ms against the multi-second `SENSOR_INTERVAL`.
fn read_dht22_retry(
    pin: &mut PinDriver<'_, impl IOPin, InputOutput>,
) -> Result<Reading, &'static str> {
    match read_dht22(pin) {
        Ok(reading) => Ok(reading),
        Err(_) => {
            busy_wait_us(2_000); // let the line settle before a fresh start pulse
            read_dht22(pin)
        }
    }
}

/// Saturation vapor pressure (hPa) via the Magnus formula.
fn saturation_vapor_pressure(temp_c: f32) -> f32 {
    6.112 * (17.67 * temp_c / (temp_c + 243.5)).exp()
}

/// What relative humidity (%) the `outside` air would have if warmed to
/// `inside_temp_c` — i.e. how humid bringing that air inside would make it.
fn virtual_rh(outside: &Reading, inside_temp_c: f32) -> f32 {
    let vapor_pressure = outside.humidity / 100.0 * saturation_vapor_pressure(outside.temperature);
    vapor_pressure / saturation_vapor_pressure(inside_temp_c) * 100.0
}

/// Ventilation hysteresis on the inside-minus-virtual-outside RH advantage (% RH):
/// turn "on" above 10%, back "off" below 2%. The resulting state drives the fan
/// GPIO (an LED for now) and is recorded in each `Record` for the log/GUI.
const FAN_ON_THRESHOLD: f32 = 10.0;
const FAN_OFF_THRESHOLD: f32 = 2.0;

/// Run the sensor loop forever on a dedicated thread: read both DHT22 sensors,
/// log to serial, and mirror to the LCD if one is present.
///
/// Inside sensor on GPIO27, outside sensor on GPIO26, LCD on I2C (SDA=GPIO13,
/// SCL=GPIO14) at address 0x27. The LCD is optional — if it doesn't init, we
/// just log a warning and keep reading sensors.
fn run_sensors(
    inside: Gpio27,
    outside: Gpio26,
    fan: Gpio25,
    i2c0: esp_idf_svc::hal::i2c::I2C0,
    sda: Gpio13,
    scl: Gpio14,
    buffer: SharedBuffer,
) {
    let mut inside_pin =
        PinDriver::input_output_od(inside).expect("Failed to configure GPIO27 (inside)");
    inside_pin.set_pull(Pull::Up).expect("pull-up");
    inside_pin.set_high().expect("high");

    let mut outside_pin =
        PinDriver::input_output_od(outside).expect("Failed to configure GPIO26 (outside)");
    outside_pin.set_pull(Pull::Up).expect("pull-up");
    outside_pin.set_high().expect("high");

    // Fan output (an LED for now): push-pull, starts off (LOW).
    let mut fan_pin = PinDriver::output(fan).expect("Failed to configure GPIO25 (fan)");
    fan_pin.set_low().expect("fan low");

    // I2C LCD on GPIO13 (SDA) / GPIO14 (SCL), PCF8574 at 0x27. Optional: a missing
    // or mis-wired display must not take down the sensor logging or iroh.
    let mut delay = FreeRtos;
    let mut i2c = I2cDriver::new(i2c0, sda, scl, &I2cConfig::new().baudrate(100.kHz().into())).ok();
    let mut lcd = match i2c.as_mut() {
        Some(i2c) => match Lcd::new(i2c, &mut delay)
            .with_address(0x27)
            .with_rows(2)
            .init()
        {
            Ok(mut l) => {
                l.clear().ok();
                l.write_str("Starting...").ok();
                Some(l)
            }
            Err(e) => {
                warn!("LCD init failed ({e:?}); continuing without display");
                None
            }
        },
        None => {
            warn!("I2C init failed; continuing without display");
            None
        }
    };

    info!("Sensor loop: inside=GPIO27, outside=GPIO26, fan/LED=GPIO25, LCD=I2C(SDA13,SCL14)@0x27");

    // DHT22 needs ≥1 s after power-on before the first read.
    std::thread::sleep(std::time::Duration::from_secs(2));

    let mut line = String::new();
    // Computed ventilation state, persisted across cycles for hysteresis. Holds
    // its value when a read fails (we can't recompute without both sensors).
    let mut fan_on = false;
    let mut tick = 0u32;
    loop {
        let inside = read_dht22_retry(&mut inside_pin);
        busy_wait_us(1_000); // brief gap between reads
        let outside = read_dht22_retry(&mut outside_pin);

        match &inside {
            Ok(r) => info!("Inside:  {:.1}°C {:.1}%", r.temperature, r.humidity),
            Err(e) => warn!("Inside sensor failed: {e}"),
        }
        match &outside {
            Ok(r) => info!("Outside: {:.1}°C {:.1}%", r.temperature, r.humidity),
            Err(e) => warn!("Outside sensor failed: {e}"),
        }

        if let Some(lcd) = lcd.as_mut() {
            // Inside on row 0, outside on row 1; `<label> 25C 50%`, or `<label> --`
            // on a failed read. Pad to 16 chars to overwrite any previous text.
            for (row, label, reading) in [(0u8, "In ", &inside), (1u8, "Out", &outside)] {
                line.clear();
                match reading {
                    Ok(r) => {
                        let _ = write!(line, "{label} {:.0}C {:.0}%", r.temperature, r.humidity);
                    }
                    Err(_) => {
                        let _ = write!(line, "{label} --");
                    }
                }
                while line.len() < 16 {
                    line.push(' ');
                }
                lcd.set_cursor(row, 0).ok();
                lcd.write_str(&line).ok();
            }
        }

        // Compute the would-be ventilation state (compute-only; no GPIO driven).
        // Needs both sensors; on a failed read we keep the previous state.
        if let (Ok(ins), Ok(out)) = (&inside, &outside) {
            let virt = virtual_rh(out, ins.temperature);
            let advantage = ins.humidity - virt;
            if !fan_on && advantage >= FAN_ON_THRESHOLD {
                fan_on = true;
            } else if fan_on && advantage < FAN_OFF_THRESHOLD {
                fan_on = false;
            }
            info!(
                "VirtRH: {virt:.1}% | advantage: {advantage:.1}% | fan(computed): {}",
                if fan_on { "on" } else { "off" }
            );
        }

        // Drive the fan output (LED) to match the current state. Done every cycle,
        // outside the compute block, so it also tracks the value held across a
        // failed read. HIGH = on.
        let _ = if fan_on {
            fan_pin.set_high()
        } else {
            fan_pin.set_low()
        };

        // Append a record to the shared log. Unix time comes from the SNTP-synced
        // clock; before the first sync it reads as 0. A failed read is stored as
        // None so the timeline (id/time) never stalls.
        let time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        if let Ok(mut buf) = buffer.lock() {
            buf.push(time, inside.ok(), outside.ok(), fan_on);
        }

        // Periodic heap watch (~every 10 cycles). Internal RAM is what the network
        // send buffers exhaust — PSRAM doesn't help there — so watch the internal
        // `min` (low-water mark) trend downward to catch the leak/degradation.
        tick = tick.wrapping_add(1);
        if tick % 10 == 0 {
            let (internal, internal_min, total) = unsafe {
                (
                    esp_idf_svc::sys::heap_caps_get_free_size(esp_idf_svc::sys::MALLOC_CAP_INTERNAL),
                    esp_idf_svc::sys::heap_caps_get_minimum_free_size(
                        esp_idf_svc::sys::MALLOC_CAP_INTERNAL,
                    ),
                    esp_idf_svc::sys::esp_get_free_heap_size(),
                )
            };
            info!("heap: internal {internal} B (min {internal_min}), total {total} B");
        }

        std::thread::sleep(SENSOR_INTERVAL);
    }
}

// ---------------------------------------------------------------------------
// WiFi / time
// ---------------------------------------------------------------------------

fn connect_wifi(modem: Modem) -> (BlockingWifi<EspWifi<'static>>, std::net::Ipv4Addr) {
    let (ssid, password) = WIFI_CONFIG
        .split_once(':')
        .expect("WIFI_CONFIG must be in the format SSID:PASSWORD");

    info!("Connecting to WiFi network: {ssid}");

    let sys_loop = EspSystemEventLoop::take().expect("Failed to take event loop");
    let nvs = EspDefaultNvsPartition::take().expect("Failed to take NVS partition");

    let mut wifi = BlockingWifi::wrap(
        EspWifi::new(modem, sys_loop.clone(), Some(nvs)).expect("Failed to create EspWifi"),
        sys_loop,
    )
    .expect("Failed to create BlockingWifi");

    let config = Configuration::Client(ClientConfiguration {
        ssid: ssid.try_into().expect("SSID too long"),
        password: password.try_into().expect("Password too long"),
        ..Default::default()
    });

    wifi.set_configuration(&config)
        .expect("Failed to set WiFi configuration");
    wifi.start().expect("Failed to start WiFi");
    info!("WiFi started");

    wifi.connect().expect("Failed to connect to WiFi");
    info!("WiFi connected");

    wifi.wait_netif_up().expect("Failed to wait for netif up");
    let ip_info = wifi
        .wifi()
        .sta_netif()
        .get_ip_info()
        .expect("Failed to get IP info");
    info!("WiFi DHCP info: {ip_info:?}");

    let ip = ip_info.ip;
    (
        wifi,
        std::net::Ipv4Addr::new(
            ip.octets()[0],
            ip.octets()[1],
            ip.octets()[2],
            ip.octets()[3],
        ),
    )
}

fn sync_time_sntp() -> esp_idf_svc::sntp::EspSntp<'static> {
    info!("Starting SNTP time sync...");
    let sntp = esp_idf_svc::sntp::EspSntp::new_default().expect("Failed to start SNTP");
    let mut retries = 0;
    while sntp.get_sync_status() != esp_idf_svc::sntp::SyncStatus::Completed {
        retries += 1;
        if retries > 30 {
            warn!("SNTP sync timed out after 30s, continuing anyway");
            break;
        }
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
    if sntp.get_sync_status() == esp_idf_svc::sntp::SyncStatus::Completed {
        info!("SNTP synced");
    }
    sntp
}

// ---------------------------------------------------------------------------
// iroh echo protocol
// ---------------------------------------------------------------------------

/// Echo protocol handler
#[derive(Debug, Clone)]
struct Echo;

impl ProtocolHandler for Echo {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let endpoint_id = connection.remote_id();
        info!("Accepted connection from {endpoint_id}");

        let (mut send, mut recv) = connection.accept_bi().await?;
        info!("Got bidi stream");

        // Echo bytes back
        let bytes_sent = tokio::io::copy(&mut recv, &mut send).await?;
        info!("Copied over {bytes_sent} byte(s)");

        send.finish()?;

        connection.closed().await;
        info!("Connection closed");

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Sensor RPC server (irpc over iroh)
// ---------------------------------------------------------------------------
// The protocol itself (SensorProtocol/SensorMessage, Record, Reading, GetLatest,
// SENSOR_ALPN) lives in the board-agnostic `dht22_proto` crate.

/// iroh `ProtocolHandler` for the sensor RPC. Cloneable shared-state server (no
/// actor loop): every accepted connection reads requests and answers them
/// against the shared buffer.
#[derive(Debug, Clone)]
struct SensorServer {
    buffer: SharedBuffer,
}

impl ProtocolHandler for SensorServer {
    async fn accept(&self, conn: Connection) -> Result<(), AcceptError> {
        while let Some(msg) = read_request::<SensorProtocol>(&conn).await? {
            self.handle_message(msg).await;
        }
        conn.closed().await;
        Ok(())
    }
}

impl SensorServer {
    async fn handle_message(&self, msg: SensorMessage) {
        match msg {
            SensorMessage::GetLatest(msg) => {
                let WithChannels { tx, .. } = msg;
                let latest = self.buffer.lock().expect("poisoned").latest();
                tx.send(latest).await.ok();
            }
            SensorMessage::GetLog(msg) => {
                let WithChannels { inner, tx, .. } = msg;
                // Snapshot under the lock, then stream without holding it across
                // awaits (std Mutex must not be held over .await).
                let records = self
                    .buffer
                    .lock()
                    .expect("poisoned")
                    .range(inner.start_id, inner.limit as usize);
                for record in records {
                    if tx.send(record).await.is_err() {
                        break; // client hung up
                    }
                }
            }
        }
    }
}

fn main() {
    // It is necessary to call this function once. Otherwise, some patches to the runtime
    // implemented by esp-idf-sys might not link properly. See https://github.com/esp-rs/esp-idf-template/issues/71
    esp_idf_svc::sys::link_patches();

    // Bind the log crate to the ESP Logging facilities
    esp_idf_svc::log::EspLogger::initialize_default();

    // Silence iroh's high-volume tracing per target, while leaving tracing usable at
    // INFO for everything else (our own logs, genuine iroh info/warn). With no tracing
    // subscriber, the tracing->log bridge emits a log line per span creation; these
    // targets (`poll_send` per packet, `QADv4` relay probes, span lifecycle, the
    // remote-state actor) are the flood. Set them to Error so the noise is gone but a
    // real error on any of them still surfaces. EspLogger's `should_log` consults this
    // per-target level (via `esp_log_level_get`), so it filters the bridged records.
    for target in [
        "iroh::socket::transports",               // poll_send (per packet)
        "iroh::net_report",                       // QADv4 relay-latency probes
        "tracing::span",                          // span lifecycle (reportgen-actor, tx, …)
        "iroh::socket::remote_map::remote_state", // RemoteStateActor / path events
    ] {
        let _ = esp_idf_svc::log::set_target_level(target, log::LevelFilter::Error);
    }

    // Register eventfd VFS — needed by mio's poll implementation which powers tokio I/O
    let eventfd_config = esp_idf_svc::sys::esp_vfs_eventfd_config_t {
        max_fds: 5,
        ..Default::default()
    };
    unsafe { esp_idf_svc::sys::esp_vfs_eventfd_register(&eventfd_config) };

    // Pure-Rust crypto provider with minimal QUIC support
    let provider = std::sync::Arc::new(quic_crypto_provider::provider());

    // Take peripherals once and split them: the modem goes to WiFi, the sensor
    // pins + I2C to the sensor thread.
    let peripherals = Peripherals::take().expect("Failed to take peripherals");
    let modem = peripherals.modem;
    let inside = peripherals.pins.gpio27;
    let outside = peripherals.pins.gpio26;
    let sda = peripherals.pins.gpio13;
    let scl = peripherals.pins.gpio14;
    // Fan output — drive an LED for now (HIGH = fan on). GPIO25 is unused here and
    // sits right next to the DHT22 pins (26/27); no strapping/flash/PSRAM conflict.
    let fan = peripherals.pins.gpio25;
    let i2c0 = peripherals.i2c0;

    // Shared sensor log: the sensor thread appends, the iroh RPC handler reads.
    let buffer: SharedBuffer = Arc::new(Mutex::new(Buffer::default()));
    let sensor_buffer = buffer.clone();

    // Run the timing-sensitive DHT22 bit-bang on its own FreeRTOS task so it is
    // decoupled from the tokio/iroh runtime. Occasional read failures (when WiFi
    // or QUIC preempt mid-frame) are expected and simply retried next cycle.
    std::thread::Builder::new()
        .name("sensors".into())
        .stack_size(8192)
        .spawn(move || run_sensors(inside, outside, fan, i2c0, sda, scl, sensor_buffer))
        .expect("Failed to spawn sensor thread");

    let (_wifi, wifi_ip) = connect_wifi(modem);

    // Sync system clock via SNTP — needed for TLS certificate validation
    // Keep _sntp alive so the periodic re-sync continues
    let _sntp = sync_time_sntp();

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(1)
        .thread_stack_size(4096)
        .build()
        .expect("Failed to create tokio runtime");

    rt.block_on(async {
        let dns_resolver = iroh::dns::DnsResolver::custom(std_dns_resolver::StdDnsResolver);

        let mut builder = iroh::Endpoint::builder(iroh::endpoint::presets::Empty)
            .crypto_provider(provider)
            .ca_tls_config(CaTlsConfig::custom_server_cert_verifier(
                insecure_verifier::skip_verify(),
            ))
            .dns_resolver(dns_resolver)
            .relay_mode(iroh::RelayMode::Default)
            .address_lookup(iroh::address_lookup::PkarrPublisher::n0_dns())
            .address_lookup(iroh::address_lookup::PkarrResolver::n0_dns())
            // Disable HTTPS latency probes and captive-portal detection: both make
            // real-cert TLS connections, which our minimal crypto provider (no RSA,
            // AES-128-GCM + X25519 only) cannot verify. QAD (UDP) probes still
            // measure relay latency.
            .net_report_config({
                let mut c = iroh::NetReportConfig::default();
                c.https_probes = false;
                c.captive_portal_check = false;
                c
            });

        if let Some(key) = parse_secret_key() {
            builder = builder.secret_key(key);
        }

        let endpoint = builder.bind().await.expect("unable to bind endpoint");

        let endpoint_id = endpoint.addr().id;
        let port = endpoint
            .bound_sockets()
            .first()
            .map(|s| s.port())
            .expect("no bound socket");

        // Short ticket: just the endpoint ID (no addresses)
        let short_ticket = EndpointTicket::new(iroh::EndpointAddr::new(endpoint_id));

        // Long ticket: includes WiFi IP + bound port
        let mut addr_with_ip = endpoint.addr();
        addr_with_ip
            .addrs
            .insert(iroh::TransportAddr::Ip(std::net::SocketAddr::new(
                wifi_ip.into(),
                port,
            )));
        let long_ticket = EndpointTicket::new(addr_with_ip);

        let sensor_server = SensorServer {
            buffer: buffer.clone(),
        };
        let _router = Router::builder(endpoint)
            .accept(ECHO_ALPN, Echo)
            .accept(SENSOR_ALPN, sensor_server)
            .spawn();

        info!("Iroh endpoint bound");
        info!("  Listening on: {wifi_ip}:{port}");
        info!("  Endpoint ID: {endpoint_id}");
        info!("  Short ticket: {short_ticket}");
        info!("  Long ticket:  {long_ticket}");
        info!("  Echo ALPN:   {}", String::from_utf8_lossy(ECHO_ALPN));
        info!("  Sensor ALPN: {}", String::from_utf8_lossy(SENSOR_ALPN));

        info!("Router started, accepting connections");

        // Keep the router running indefinitely
        loop {
            tokio::time::sleep(tokio::time::Duration::from_secs(3600)).await;
        }
    });
}
