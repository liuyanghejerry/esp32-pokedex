//! FoloToy AI Passport Pokedex firmware.
//!
//! Shows the full 386-entry national dex on the 240x320 ST7789 display, with
//! each species' GBA cry played as it comes on screen and a randomly picked
//! FireRed town/route theme looping in the background.
//! The three board buttons share the GPIO0 ADC resistor ladder:
//! UP = previous species, DOWN = next species (hold to auto-repeat),
//! OK = toggle card/detail page. Data, sprites and audio are baked in from
//! open-pokefirered at build time (tools/gen_assets.py, tools/gen_audio.py).
//!
//! Pin map (ai-passport `components/bsp/include/bsp_pins.h`):
//!   SPI2: MOSI=GPIO9 SCLK=GPIO8 CS=GPIO1 DC=GPIO20 (no RST)
//!   backlight = GPIO21 (plain high = full on)
//!   buttons = GPIO0 (ADC1_CH0, external 10k pull-up ladder)
//!   I2C0: SDA=GPIO10 SCL=GPIO7 (ES8311 codec at 0x18)
//!   I2S0: MCLK=GPIO6 BCLK=GPIO5 WS=GPIO3 DOUT=GPIO2
//! Logs go to the native USB-Serial/JTAG port (GPIO18/19).

#![no_std]
#![no_main]

use esp_backtrace as _;
use esp_hal::{
    analog::adc::{Adc, AdcCalCurve, AdcConfig, Attenuation},
    clock::CpuClock,
    delay::Delay,
    gpio::{Level, Output, OutputConfig, Pin},
    spi::{master::Config as SpiConfig, master::Spi, Mode as SpiMode},
    time::Rate,
};

use pokedex_core::{audio::Rng, Action, ButtonEvent, Debouncer, DexModel, Page};

mod audio;
mod audio_data;
mod boot;
mod dex_data;
mod font_zh;
mod font_zh_data;
mod sprites;
mod st7789;
mod ui;

use audio::Audio;
use st7789::{FrameBuffer, St7789};

esp_bootloader_esp_idf::esp_app_desc!();

/// ADC button poll period; must match `pokedex_core::TICK_MS`.
const POLL_MS: u32 = pokedex_core::TICK_MS;
/// Turn the backlight off after this much time without input.
const BACKLIGHT_TIMEOUT_MS: u32 = 120_000;
/// SPI clock for the panel. BSP runs 80 MHz; 40 MHz is plenty for a
/// full-frame push (~30 ms) and leaves signal-integrity margin.
const LCD_SPI_HZ: Rate = Rate::from_mhz(40);

/// Classic "sent out of the pokéball" entrance for the new species: two
/// frames as a dark silhouette (the materialize flash), then a damped
/// up/down wobble that settles into place. Each frame is a full re-render
/// + push (~35 ms), ~9 frames ≈ 320 ms total.
fn animate_entrance<SPI, CS, DC>(
    lcd: &mut St7789<SPI, CS, DC>,
    fb: &mut FrameBuffer,
    model: &DexModel,
    audio: &mut dyn Audio,
) where
    SPI: embedded_hal::spi::SpiBus<u8>,
    CS: embedded_hal::digital::OutputPin,
    DC: embedded_hal::digital::OutputPin,
{
    let page = model.page();
    let amp: i32 = match page {
        Page::Card => 8,
        Page::Detail => 4,
    };
    let e = &dex_data::DEX[model.no() - 1];
    let no = model.no();

    // silhouette flash (materialize), then damped wobble
    const WOBBLE: &[i32] = &[-2, 0, 1, 0, -1, 0, 0];
    for (i, &w) in [0, 0].iter().chain(WOBBLE.iter()).enumerate() {
        let silhouette = i < 2;
        let dy = w * amp / 2;
        ui::render_anim(fb, e, no, dy, silhouette, page);
        lcd.push_frame(fb, &mut || audio.pump());
    }
}

#[esp_hal::main]
fn main() -> ! {
    esp_println::println!("pokedex: boot");

    let p = esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::max()));
    let mut delay = Delay::new();

    // Backlight comes up with the boot animation: its first frame is black, so
    // the panel fades in from dark instead of flashing the previous contents.
    let mut bl = Output::new(p.GPIO21, Level::Low, OutputConfig::default());

    let cs = Output::new(p.GPIO1, Level::High, OutputConfig::default());
    let dc = Output::new(p.GPIO20, Level::Low, OutputConfig::default());
    let spi = Spi::new(
        p.SPI2,
        SpiConfig::default()
            .with_frequency(LCD_SPI_HZ)
            .with_mode(SpiMode::_0),
    )
    .unwrap()
    .with_sck(p.GPIO8)
    .with_mosi(p.GPIO9);
    let mut lcd = St7789::new(spi, cs, dc);
    lcd.init(&mut delay);

    // Buttons: ADC ladder on GPIO0 (external pull-up; must not use the
    // internal one — see bsp_pins.h).
    let mut adc_cfg = AdcConfig::new();
    let mut btn_pin =
        adc_cfg.enable_pin_with_cal::<_, AdcCalCurve<_>>(p.GPIO0, Attenuation::_11dB);
    let mut adc = Adc::new(p.ADC1, adc_cfg);

    static mut FRAME: FrameBuffer = FrameBuffer::new();
    let fb: &mut FrameBuffer = unsafe { &mut *core::ptr::addr_of_mut!(FRAME) };

    let mut model = DexModel::new(dex_data::DEX_LEN, 1);

    // Pick this boot's background theme from the hardware RNG, so the device
    // opens on a different town/route each time.
    let mut rng = Rng::new(esp_hal::rng::Rng::new().random());
    let bgm_track = rng.below(audio::TRACK_COUNT as u32) as usize;
    esp_println::println!("audio: bgm pick {} ({})", bgm_track, audio::track_title(bgm_track));

    let mut player = audio::Player::new(
        p.I2C0,
        p.I2S0,
        p.DMA_CH0,
        &mut delay,
        audio::Wiring {
            sda: p.GPIO10.degrade(),
            scl: p.GPIO7.degrade(),
            mclk: p.GPIO6.degrade(),
            bclk: p.GPIO5.degrade(),
            ws: p.GPIO3.degrade(),
            dout: p.GPIO2.degrade(),
        },
    );

    // The animation ends on the first dex card, so the backlight comes up
    // with the ball.
    bl.set_high();
    let mut skip = || {
        matches!(
            adc.read_oneshot(&mut btn_pin),
            Ok(mv) if pokedex_core::classify_mv(mv).is_some()
        )
    };
    boot::run(&mut lcd, fb, &mut delay, &model, &mut player, bgm_track, &mut skip);
    esp_println::println!("pokedex: first frame shown, no={}", model.no());

    let mut debouncer = Debouncer::new();
    let mut idle_ms: u32 = 0;
    let mut backlight_on = true;

    loop {
        delay.delay_millis(POLL_MS);
        idle_ms = idle_ms.saturating_add(POLL_MS);
        player.pump();

        let mv = loop {
            match adc.read_oneshot(&mut btn_pin) {
                Ok(mv) => break mv,
                Err(nb::Error::WouldBlock) => {}
                Err(_) => break 3300,
            }
        };
        let ev = debouncer.feed(mv);
        if let Some(ev) = ev {
            esp_println::println!("btn: {:?} ({} mV)", ev, mv);
            idle_ms = 0;
            if !backlight_on {
                bl.set_high();
                backlight_on = true;
            }
            let fresh_press = matches!(ev, ButtonEvent::Press(_));
            match model.handle(ev) {
                // A fresh tap gets the entrance animation and the cry; auto-repeat
                // (browsing by holding) only gets a scroll blip, so holding the
                // button does not machine-gun 386 cries.
                Action::Navigate if fresh_press => {
                    player.sfx(audio_data::sfx::DEX_SCROLL);
                    player.cry(model.no());
                    animate_entrance(&mut lcd, fb, &model, &mut player);
                }
                Action::Navigate => {
                    player.sfx(audio_data::sfx::DEX_SCROLL);
                    ui::render(fb, &model);
                    lcd.push_frame(fb, &mut || player.pump());
                }
                Action::TogglePage => {
                    player.sfx(audio_data::sfx::DEX_PAGE);
                    ui::render(fb, &model);
                    lcd.push_frame(fb, &mut || player.pump());
                }
                Action::None => {}
            }
        }

        if backlight_on && idle_ms >= BACKLIGHT_TIMEOUT_MS {
            bl.set_low();
            backlight_on = false;
            esp_println::println!("pokedex: backlight off (idle)");
        }
    }
}
