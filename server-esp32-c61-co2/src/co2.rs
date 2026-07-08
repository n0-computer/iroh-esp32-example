//! Sensirion SCD30 CO2 sensor over I2C — minimal inline driver (no driver crate).
//!
//! Wiring (ESP32-C61):
//!   SDA -> GPIO4
//!   SCL -> GPIO5
//!   VDD -> 3V3 (SCD30 accepts 3.3-5.5V; keep I2C logic at 3.3V for the ESP32)
//!   GND -> GND
//!   SEL -> GND or floating (selects I2C mode; VDD would select Modbus)
//! The SCD30 is an I2C device at address 0x61, standard-mode (100 kHz). Most
//! breakouts carry SDA/SCL pull-ups; the ESP32's internal ones are on as backup.
//!
//! The protocol is a handful of 16-bit commands with a Sensirion CRC-8 per 2-byte
//! word, so we talk to it directly over esp-idf-hal's I2C driver rather than pull
//! in a driver dependency. Reads need a short delay after the command pointer
//! write, hence the explicit sleeps.

use std::thread::sleep;
use std::time::Duration;

use esp_idf_svc::hal::delay::BLOCK;
use esp_idf_svc::hal::gpio::{Gpio4, Gpio5, Gpio8};
use esp_idf_svc::hal::i2c::{I2cConfig, I2cDriver, I2C0};
use esp_idf_svc::hal::spi::SPI2;
use esp_idf_svc::hal::units::FromValueType; // for `.kHz()`
use log::{error, info, warn};

use crate::led::{self, RgbLed};

/// Dim purple shown while warming up / when the sensor isn't responding yet — an
/// off-scale color so it's never confused with blue (which now means pristine air).
const COLOR_PENDING: (u8, u8, u8) = (18, 0, 24);

/// Best-effort LED update — the sensor keeps working even if the LED doesn't.
fn show(led: &mut Option<RgbLed>, (r, g, b): (u8, u8, u8)) {
    if let Some(l) = led.as_mut() {
        if let Err(e) = l.set(r, g, b) {
            warn!("[co2] LED write failed: {e}");
        }
    }
}

const ADDR: u8 = 0x61;

// Commands (big-endian).
const CMD_START_CONTINUOUS: u16 = 0x0010; // + 2-byte pressure arg + CRC
const CMD_DATA_READY: u16 = 0x0202;
const CMD_READ_MEASUREMENT: u16 = 0x0300;
const CMD_FIRMWARE_VERSION: u16 = 0xD100;

/// Own the I2C peripheral + pins and drive the SCD30 forever. Logs and exits the
/// thread on a fatal init error; recoverable read errors are logged and retried.
pub fn run(
    i2c: I2C0<'static>,
    sda: Gpio4<'static>,
    scl: Gpio5<'static>,
    spi: SPI2<'static>,
    led_pin: Gpio8<'static>,
) {
    // On-board WS2812 (GPIO8) as a CO2 traffic light, over SPI (the C61 has no RMT).
    // Best-effort: if it fails to init, the sensor still runs — just no visual.
    let mut led = match RgbLed::new(spi, led_pin) {
        Ok(l) => Some(l),
        Err(e) => {
            warn!("[co2] RGB LED init failed: {e}");
            None
        }
    };
    show(&mut led, COLOR_PENDING);

    // 100 kHz standard mode; internal pull-ups on as a backup to the breakout's.
    // The SCD30 clock-stretches (holds SCL low) while it prepares a measurement —
    // sometimes into the *next* command's start bit, which surfaces as
    // ESP_ERR_TIMEOUT on the read-measurement write. The default SCL timeout is far
    // too short for that, so bump it: on the C61 this register is a log2 value
    // capped at ~105 ms, and any request in ~52–105 ms lands on that max bucket.
    let config = I2cConfig::new()
        .baudrate(100_u32.kHz().into())
        .sda_enable_pullup(true)
        .scl_enable_pullup(true)
        .timeout(Duration::from_millis(100).into());

    let mut dev = match I2cDriver::new(i2c, sda, scl, &config) {
        Ok(d) => d,
        Err(e) => {
            error!("[co2] I2C init failed: {e}");
            return;
        }
    };

    // Self-healing: (re)initialize whenever the sensor is absent or drops off the
    // bus. This means you can flash first and wire the SCD30 up afterwards — it
    // starts working within a couple seconds, no restart needed — and it recovers
    // if the sensor is briefly unplugged. A missing device just NACKs, so the I2C
    // calls return an error promptly rather than blocking.
    let mut announced_missing = false;
    'reinit: loop {
        // Presence check: read the firmware version (major, minor + CRC).
        match read_words(&mut dev, CMD_FIRMWARE_VERSION, 1) {
            Ok(w) => info!("[co2] SCD30 found, firmware {}.{}", w[0] >> 8, w[0] & 0xff),
            Err(_) => {
                // Log once per absence so we don't spam while it's unwired.
                if !announced_missing {
                    warn!("[co2] no SCD30 on GPIO4/5 (addr 0x61) — waiting; wire it up and it'll pick up");
                    announced_missing = true;
                }
                show(&mut led, COLOR_PENDING);
                sleep(Duration::from_secs(2));
                continue 'reinit;
            }
        }
        announced_missing = false;

        // Start continuous measurement, pressure compensation off (arg = 0).
        if let Err(e) = write_arg(&mut dev, CMD_START_CONTINUOUS, 0) {
            warn!("[co2] start continuous measurement failed ({e}); retrying");
            sleep(Duration::from_secs(2));
            continue 'reinit;
        }
        info!("[co2] continuous measurement started (~2 s interval)");

        // Read loop. A run of errors means the sensor fell off the bus → re-init.
        let mut errors = 0u32;
        loop {
            sleep(Duration::from_secs(2));

            // Data-ready status: one word, 1 = a fresh sample is waiting.
            match read_words(&mut dev, CMD_DATA_READY, 1) {
                Ok(w) if w[0] == 1 => {}
                Ok(_) => continue, // not ready yet
                Err(e) => {
                    errors += 1;
                    if errors >= 3 {
                        warn!("[co2] SCD30 stopped responding ({e}); re-initializing");
                        continue 'reinit;
                    }
                    continue;
                }
            }

            // Measurement: 6 words = 3 big-endian f32 (CO2 ppm, T °C, RH %).
            match read_words(&mut dev, CMD_READ_MEASUREMENT, 6) {
                Ok(w) => {
                    errors = 0;
                    let co2 = f32_from_words(w[0], w[1]);
                    let temp = f32_from_words(w[2], w[3]);
                    let rh = f32_from_words(w[4], w[5]);
                    info!("[co2] CO2 = {co2:.0} ppm   T = {temp:.1} °C   RH = {rh:.1} %");
                    show(&mut led, led::co2_color(co2));
                }
                Err(e) => {
                    errors += 1;
                    warn!("[co2] measurement read failed: {e}");
                    if errors >= 3 {
                        continue 'reinit;
                    }
                }
            }
        }
    }
}

/// Send a bare command pointer, wait, then read `n_words` × (2 data bytes + CRC),
/// verifying each CRC. Returns the decoded 16-bit words.
fn read_words(dev: &mut I2cDriver<'_>, cmd: u16, n_words: usize) -> Result<Vec<u16>, String> {
    dev.write(ADDR, &cmd.to_be_bytes(), BLOCK)
        .map_err(|e| format!("write cmd {cmd:#06x}: {e}"))?;
    // SCD30 needs > 3 ms between the pointer write and the read.
    sleep(Duration::from_millis(4));

    let mut buf = vec![0u8; n_words * 3];
    dev.read(ADDR, &mut buf, BLOCK)
        .map_err(|e| format!("read {cmd:#06x}: {e}"))?;

    let mut words = Vec::with_capacity(n_words);
    for chunk in buf.chunks_exact(3) {
        if crc8(&chunk[0..2]) != chunk[2] {
            return Err(format!("CRC mismatch on {cmd:#06x}"));
        }
        words.push(u16::from_be_bytes([chunk[0], chunk[1]]));
    }
    Ok(words)
}

/// Send a command with a 16-bit argument and its CRC (e.g. start-continuous).
fn write_arg(dev: &mut I2cDriver<'_>, cmd: u16, arg: u16) -> Result<(), String> {
    let a = arg.to_be_bytes();
    let frame = [cmd.to_be_bytes()[0], cmd.to_be_bytes()[1], a[0], a[1], crc8(&a)];
    dev.write(ADDR, &frame, BLOCK)
        .map_err(|e| format!("write cmd {cmd:#06x}: {e}"))
}

/// Reassemble a big-endian IEEE-754 f32 from its high and low 16-bit words.
fn f32_from_words(hi: u16, lo: u16) -> f32 {
    f32::from_bits(((hi as u32) << 16) | lo as u32)
}

/// Sensirion CRC-8: poly 0x31, init 0xFF, no reflection, no final XOR.
fn crc8(data: &[u8]) -> u8 {
    let mut crc = 0xffu8;
    for &byte in data {
        crc ^= byte;
        for _ in 0..8 {
            crc = if crc & 0x80 != 0 {
                (crc << 1) ^  0x31
            } else {
                crc << 1
            };
        }
    }
    crc
}
