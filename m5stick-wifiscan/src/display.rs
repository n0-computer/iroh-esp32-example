use esp_idf_svc::hal::delay::Ets;
use esp_idf_svc::hal::gpio::{Gpio12, Gpio13, Gpio14, Gpio15, Gpio27, Gpio5, PinDriver};
use esp_idf_svc::hal::peripheral::Peripheral;
use esp_idf_svc::hal::prelude::*;
use esp_idf_svc::hal::spi::{config, SpiDeviceDriver, SpiSingleDeviceDriver, SpiDriverConfig};
use display_interface_spi::SPIInterfaceNoCS;
use embedded_graphics::mono_font::{ascii::FONT_8X13_BOLD, MonoTextStyle};
use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::{PrimitiveStyle, Rectangle};
use embedded_graphics::text::Text;
use mipidsi::Builder;
use log::{info, warn};
use std::sync::{Mutex, OnceLock};

type LcdSpi = SpiSingleDeviceDriver<'static>;
type LcdDcPin = PinDriver<'static, Gpio14, esp_idf_svc::hal::gpio::Output>;
type LcdRstPin = PinDriver<'static, Gpio12, esp_idf_svc::hal::gpio::Output>;
type LcdBlPin = PinDriver<'static, Gpio27, esp_idf_svc::hal::gpio::Output>;
type LcdInterface = SPIInterfaceNoCS<LcdSpi, LcdDcPin>;
type LcdDisplay = mipidsi::Display<LcdInterface, mipidsi::models::ST7789, LcdRstPin>;

struct DisplayState {
    display: LcdDisplay,
    _backlight: LcdBlPin,
    last_lines: Vec<String>,
}

static DISPLAY_STATE: OnceLock<Mutex<DisplayState>> = OnceLock::new();

fn draw_lines(state: &mut DisplayState, lines: &[String]) -> Result<(), String> {
    const MAX_LINES: usize = 8;
    const LINE_HEIGHT: i32 = 16;
    const BASELINE_START: i32 = 14;
    const FONT_HEIGHT: i32 = 13;
    const SCREEN_WIDTH: u32 = 240;

    let style = MonoTextStyle::new(&FONT_8X13_BOLD, Rgb565::WHITE);
    let new_lines: Vec<String> = lines.iter().take(MAX_LINES).cloned().collect();
    let line_count = state.last_lines.len().max(new_lines.len());

    for idx in 0..line_count {
        let old_line = state.last_lines.get(idx);
        let new_line = new_lines.get(idx);

        if old_line == new_line {
            continue;
        }

        let baseline_y = BASELINE_START + (idx as i32) * LINE_HEIGHT;
        let top_y = baseline_y - FONT_HEIGHT - 1;

        Rectangle::new(
            Point::new(0, top_y.max(0)),
            Size::new(SCREEN_WIDTH, LINE_HEIGHT as u32),
        )
        .into_styled(PrimitiveStyle::with_fill(Rgb565::BLACK))
        .draw(&mut state.display)
        .map_err(|e| format!("Clear line {idx}: {e:?}"))?;

        if let Some(line) = new_line {
            Text::new(line, Point::new(2, baseline_y), style)
                .draw(&mut state.display)
                .map_err(|e| format!("Draw line {idx}: {e:?}"))?;
        }
    }

    state.last_lines = new_lines;

    Ok(())
}

pub fn show_lines(lines: Vec<String>) {
    let Some(lock) = DISPLAY_STATE.get() else {
        return;
    };

    let mut state = match lock.lock() {
        Ok(guard) => guard,
        Err(_) => {
            warn!("Display mutex poisoned, skipping update");
            return;
        }
    };

    if let Err(e) = draw_lines(&mut state, &lines) {
        warn!("Display update failed: {e}");
    }
}

/// Initialize M5StickC Plus2 display and render a constant string.
///
/// Pins (Plus2):
///   MOSI=GPIO15  CLK=GPIO13  DC=GPIO14  RST=GPIO12  CS=GPIO5  BL=GPIO27
pub fn init_display(
    spi: impl Peripheral<P = esp_idf_svc::hal::spi::SPI2> + 'static,
    dc: impl Peripheral<P = Gpio14> + 'static,
    rst: impl Peripheral<P = Gpio12> + 'static,
    cs: impl Peripheral<P = Gpio5> + 'static,
    mosi: impl Peripheral<P = Gpio15> + 'static,
    sclk: impl Peripheral<P = Gpio13> + 'static,
    backlight: impl Peripheral<P = Gpio27> + 'static,
) -> Result<(), String> {
    info!("Initializing M5StickC Plus2 display");

    let rst_pin = PinDriver::output(rst).map_err(|e| format!("RST: {e}"))?;
    let dc_pin  = PinDriver::output(dc).map_err(|e| format!("DC: {e}"))?;
    let mut backlight_pin = PinDriver::output(backlight).map_err(|e| format!("BL: {e}"))?;

    // Turn on backlight early so we can at least see if the panel powers up.
    backlight_pin.set_high().map_err(|e| format!("BL high: {e}"))?;

    // Keep SPI under the ESP-IDF full-duplex limit for reliable init.
    let spi_config = config::Config::new()
        .baudrate(26.MHz().into())
        .data_mode(embedded_hal::spi::MODE_0);

    let device = SpiDeviceDriver::new_single(
        spi,
        sclk,
        mosi,
        None::<esp_idf_svc::hal::gpio::Gpio38>, // no MISO
        Some(cs),
        &SpiDriverConfig::new(),
        &spi_config,
    ).map_err(|e| format!("SPI: {e}"))?;

    let di = SPIInterfaceNoCS::new(device, dc_pin);

    let mut delay = Ets;

    // st7789_pico1 = 135x240, inverted colors, correct offsets — matches Plus2
    let mut display = Builder::st7789_pico1(di)
        .with_orientation(mipidsi::Orientation::Landscape(true))
        .init(&mut delay, Some(rst_pin))
        .map_err(|e| format!("Display init: {e:?}"))?;

    // The ST7789's GRAM powers up full of garbage, and draw_lines() only clears
    // the per-line rectangles it writes. Wipe the whole panel once so everything
    // outside the text is black instead of noise.
    display
        .clear(Rgb565::BLACK)
        .map_err(|e| format!("Clear screen: {e:?}"))?;

    let mut state = DisplayState {
        display,
        _backlight: backlight_pin,
        last_lines: Vec::new(),
    };

    draw_lines(&mut state, &["M5 Boot OK".to_string()])?;

    if DISPLAY_STATE.set(Mutex::new(state)).is_err() {
        warn!("Display was already initialised, ignoring second init");
    }

    info!("Display ready");
    Ok(())
}
