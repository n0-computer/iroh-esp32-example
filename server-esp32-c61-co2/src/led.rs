//! On-board addressable RGB LED (WS2812 on GPIO8) driven over **SPI** — no driver
//! crate, no RMT.
//!
//! Why SPI: the ESP32-C61 has **no RMT peripheral** (a cost-reduced C6 — its SOC
//! caps advertise SPI and I2S but not RMT), so the usual WS2812 path is gone. And
//! the ws2812 driver crates can't be used anyway — they pin esp-idf-sys 0.36,
//! which predates the C61. So we clock the WS2812 one-wire waveform out of the SPI
//! MOSI line: at 2.4 MHz each SPI bit is ~417 ns, and we encode each WS2812 bit as
//! **three** SPI bits — `110` for a '1' (long high) and `100` for a '0' (short
//! high). 24 GRB bits → 72 SPI bits → 9 bytes per pixel. No SCLK/CS pin is used
//! (`new_without_sclk`); the line idles low between our multi-second updates, far
//! longer than the >50 µs reset, so the pixel latches on its own.

use esp_idf_svc::hal::gpio::{AnyIOPin, Gpio8};
use esp_idf_svc::hal::spi::config::{Config, DriverConfig};
use esp_idf_svc::hal::spi::{SpiDeviceDriver, SpiDriver, SPI2};
use esp_idf_svc::hal::units::FromValueType; // for `.kHz()`
use esp_idf_svc::sys::EspError;

pub struct RgbLed<'d> {
    spi: SpiDeviceDriver<'d, SpiDriver<'d>>,
}

impl<'d> RgbLed<'d> {
    pub fn new(spi: SPI2<'d>, data_pin: Gpio8<'d>) -> Result<Self, EspError> {
        // MOSI = GPIO8, no SCLK, no MISO. 2.4 MHz → ~417 ns/bit, 3 bits per WS2812 bit.
        let driver =
            SpiDriver::new_without_sclk(spi, data_pin, Option::<AnyIOPin>::None, &DriverConfig::new())?;
        let spi = SpiDeviceDriver::new(
            driver,
            Option::<AnyIOPin>::None, // no chip-select
            &Config::new().baudrate(2_400_u32.kHz().into()),
        )?;
        Ok(Self { spi })
    }

    /// Set the single pixel. WS2812 expects GRB, MSB first.
    pub fn set(&mut self, r: u8, g: u8, b: u8) -> Result<(), EspError> {
        let grb: u32 = ((g as u32) << 16) | ((r as u32) << 8) | b as u32;
        self.spi.write(&encode(grb))
    }
}

/// Expand 24 WS2812 bits into 72 SPI bits (9 bytes), MSB first: '1' → 0b110,
/// '0' → 0b100 (each three SPI bits ≈ one 1.25 µs WS2812 bit at 2.4 MHz).
fn encode(grb: u32) -> [u8; 9] {
    let mut buf = [0u8; 9];
    let mut pos = 0usize; // SPI bit index across the 9 bytes, MSB first
    for i in (0..24).rev() {
        let pattern: u8 = if (grb >> i) & 1 == 1 { 0b110 } else { 0b100 };
        for k in (0..3).rev() {
            if (pattern >> k) & 1 == 1 {
                buf[pos / 8] |= 0x80 >> (pos % 8);
            }
            pos += 1;
        }
    }
    buf
}

/// CO2 → color, smoothly faded (kept dim — WS2812 at full is blinding). The scale
/// runs **blue** (≈ outdoor 420 ppm, cleanest) → **green** (fresh) → **yellow**
/// (moderate) → **red** (stuffy), interpolating in RGB between these stops so the
/// color drifts continuously as the air changes rather than snapping at thresholds.
pub fn co2_color(ppm: f32) -> (u8, u8, u8) {
    // (ppm, (r, g, b)) stops, ascending.
    const STOPS: [(f32, (u8, u8, u8)); 4] = [
        (420.0, (0, 0, 45)),   // blue   — outdoor ambient, cleanest
        (650.0, (0, 45, 0)),   // green  — fresh indoor
        (1000.0, (45, 35, 0)), // yellow — moderate
        (1500.0, (55, 0, 0)),  // red    — stuffy, ventilate
    ];
    if ppm <= STOPS[0].0 {
        return STOPS[0].1;
    }
    for w in STOPS.windows(2) {
        let (p0, c0) = w[0];
        let (p1, c1) = w[1];
        if ppm <= p1 {
            let t = (ppm - p0) / (p1 - p0);
            return (lerp(c0.0, c1.0, t), lerp(c0.1, c1.1, t), lerp(c0.2, c1.2, t));
        }
    }
    STOPS[STOPS.len() - 1].1 // above the last stop → red
}

/// Linear-interpolate one 8-bit channel: `a` at t=0, `b` at t=1 (t in 0..=1).
fn lerp(a: u8, b: u8, t: f32) -> u8 {
    (a as f32 + (b as f32 - a as f32) * t).round() as u8
}
