//! ES8311 + I2S audio out.
//!
//! Signal path: the mixer (see `pokedex_core::audio`) renders 16-bit stereo
//! blocks, which are pushed into a **looping DMA stream** feeding I2S0 TX.
//! The ES8311 is the I2S *slave* and is configured over I2C (control port
//! 0x18); the SoC drives MCLK/BCLK/WS and owns the sample rate — see
//! `SAMPLE_RATE` in `audio_data.rs` (11025 Hz, so the 10512 Hz GBA cries need
//! no resampling: they simply play 4.9% fast, well under a semitone).
//!
//! Three voices, two buses:
//!   * cry   — the current species' cry, triggered on every page change
//!   * music — the looping BGM track, or the one-shot boot jingle
//!   * sfx   — UI blips and the boot ball bounces
//!
//! The DMA descriptor chain is circular (`DmaTxStreamBuf`), so playback never
//! stops between blocks: `pump()` just tops the ring back up from the main
//! loop and the hardware keeps reading it. That is what makes gapless playback
//! possible without interrupts or a second core.

use esp_hal::{
    dma::{aligned::DmaAlignedMut, DmaTxStreamBuf},
    gpio::AnyPin,
    i2c::master::{Config as I2cConfig, I2c},
    i2s::master::{I2s, I2sTxDmaTransfer, TdmConfig},
    peripherals::{DMA_CH0, I2C0, I2S0},
    time::Rate,
    Blocking,
};

use pokedex_core::audio::{Blobs, Levels, Mixer};

use crate::audio_data::{self as ad};

/// ES8311 7-bit I2C address (ai-passport `bsp_pins.h`).
const ES8311_ADDR: u8 = 0x18;
/// Control-port clock; the codec is a slow I2C device.
const I2C_HZ: Rate = Rate::from_khz(100);

/// Per-source playback levels (0..=255). The three buses share one 16-bit
/// stream, so the levels must add up to no more than full scale — a cry over
/// an unducked music bed and a blip is 167 of those 255. Anything above full
/// scale clips on every page turn.
///
/// Tuned by ear on the device. The cries went 140 → 105 → (halved) 52 → 47 →
/// 42, then back to 52: with the bed slow to clear, going quieter only buried
/// the cry under the music. Fixing the duck speed (below) is what makes the
/// quieter cry audible, so 52 stands.
const LEVELS: Levels = Levels {
    cry: 52,
    music: 60,
    music_ducked: 24,
    sfx: 55,
    // Level units per mixer block (23 ms). 12 clears the bed from 60 to 24 in
    // 3 blocks ≈ 70 ms — fast enough that the cry's attack is not masked,
    // slow enough not to sound like a gate. (6 took ~140 ms and was audible.)
    duck_step: 12,
};

/// Bytes of the DMA ring: 8 descriptors x 1 KB = 8192 B = 2048 stereo frames
/// ≈ 186 ms of audio. Each descriptor is 256 frames ≈ 23 ms.
const STREAM_BYTES: usize = 8 * 1024;
const CHUNK: usize = 1024;
/// Descriptors handed to the DMA when a stream starts. **Not** the whole ring:
/// `prepare()` leaves the write cursor just past the pre-filled bytes, so a
/// full pre-fill would leave nothing writable until the DMA had drained all of
/// it — an underrun every 186 ms. Starting with a few leaves room to top up
/// from the first pump.
const PREFILL_CHUNKS: usize = 3;

/// The four I2S pins plus the I2C pair, as type-erased GPIOs so `Player::new`
/// does not need a pin type parameter for each of them.
pub struct Wiring {
    pub sda: AnyPin<'static>,
    pub scl: AnyPin<'static>,
    pub mclk: AnyPin<'static>,
    pub bclk: AnyPin<'static>,
    pub ws: AnyPin<'static>,
    pub dout: AnyPin<'static>,
}

/// The audio facade the UI and the boot animation talk to. A failed codec probe
/// leaves `Player` with no stream rather than bricking the dex — the display
/// has to work even if the codec does not answer on I2C.
pub trait Audio {
    /// Top up the DMA ring. Cheap when nothing is playing.
    fn pump(&mut self);
    /// Play the cry of dex number `no` (1-based), interrupting any current one.
    fn cry(&mut self, no: usize);
    /// One-shot sound effect by index into `audio_data::SFX`.
    fn sfx(&mut self, idx: usize);
    /// One-shot clip on the music bus (the boot jingle).
    fn jingle(&mut self, idx: usize);
    /// Start BGM track `track` looping from the top.
    fn bgm(&mut self, track: usize);
}

/// BGM track title, for the boot log.
pub fn track_title(track: usize) -> &'static str {
    ad::TRACKS.get(track).map(|t| t.title).unwrap_or("?")
}

/// Number of BGM tracks in the build.
pub const TRACK_COUNT: usize = ad::TRACKS.len();

/// Owns the I2S stream and the mixer. `tx` is `None` when the codec did not
/// come up; every method then does nothing but the dex still runs.
pub struct Player {
    mixer: Mixer,
    tx: Option<I2sTxDmaTransfer<'static, Blocking, DmaTxStreamBuf>>,
}

impl Player {
    pub fn new(
        i2c: I2C0<'static>,
        i2s: I2S0<'static>,
        dma: DMA_CH0<'static>,
        delay: &mut esp_hal::delay::Delay,
        w: Wiring,
    ) -> Self {
        let mixer = Mixer::new(
            Blobs {
                cry: ad::CRY_DATA,
                music: ad::BGM_DATA,
                sfx: ad::SFX_DATA,
            },
            LEVELS,
        );
        match bring_up(i2c, i2s, dma, delay, w) {
            Ok(tx) => {
                esp_println::println!(
                    "audio: ES8311 up, {} Hz, {} tracks",
                    ad::SAMPLE_RATE,
                    TRACK_COUNT
                );
                Self {
                    mixer,
                    tx: Some(tx),
                }
            }
            Err(e) => {
                esp_println::println!("audio: unavailable ({e}), running silent");
                Self { mixer, tx: None }
            }
        }
    }
}

impl Audio for Player {
    fn pump(&mut self) {
        if self.tx.is_none() {
            return;
        }
        // A streaming transfer only reports "done" when the DMA ran off the end
        // of the descriptor chain — i.e. we failed to keep it fed. Rebuild the
        // stream instead of leaving the device silent for the rest of the run.
        if self.tx.as_ref().is_some_and(|t| t.is_done()) {
            self.restart();
            return;
        }
        let Some(t) = self.tx.as_mut() else {
            return;
        };
        // Fill the ring completely: every byte the DMA has drained is runway
        // for the next render. Trying to hold a target queue level instead
        // leaves the write cursor mid-ring, so freed descriptors behind it are
        // invisible to `available_bytes()` and the ring silently drains.
        loop {
            let pushed = t.push_with(|buf| {
                let n = buf.len() & !3; // whole stereo frames only
                if n == 0 {
                    return 0;
                }
                self.mixer.fill(&mut buf[..n]);
                n
            });
            if pushed == 0 {
                break;
            }
        }
    }

    fn cry(&mut self, no: usize) {
        let Some(c) = ad::CRIES.get(no.wrapping_sub(1)) else {
            return;
        };
        let (off, len, gain) = (c.off, c.len, c.gain);
        self.mixer.play_cry(off, len, gain);
    }

    fn sfx(&mut self, idx: usize) {
        if let Some(c) = ad::SFX.get(idx) {
            self.mixer.play_sfx(c.off, c.len, c.gain);
        }
    }

    fn jingle(&mut self, idx: usize) {
        if let Some(c) = ad::SFX.get(idx) {
            self.mixer.play_music(c.off, c.len, c.gain, false);
        }
    }

    fn bgm(&mut self, track: usize) {
        if let Some(t) = ad::TRACKS.get(track) {
            self.mixer.play_music(t.off, t.len, t.gain, true);
        }
    }
}

impl Player {
    /// Rebuild the DMA stream after an underrun stopped it: give the buffer
    /// back to the peripheral with the first few descriptors already filled,
    /// and let the normal `pump()` path keep it going from there.
    fn restart(&mut self) {
        let Some(t) = self.tx.take() else {
            return;
        };
        let (i2s, buf) = t.stop();
        let (desc, data) = buf.split();
        let Ok(mut fresh) = DmaTxStreamBuf::new(desc, data) else {
            return;
        };
        fresh.push_with(|b| {
            let n = (PREFILL_CHUNKS * CHUNK).min(b.len()) & !3;
            self.mixer.fill(&mut b[..n]);
            n
        });
        if let Ok(t) = i2s.write(fresh) {
            self.tx = Some(t);
            esp_println::println!("audio: dma underrun, stream restarted");
        }
    }
}

/// I2C + codec + I2S bring-up. The I2S stream starts **before** the codec is
/// configured: the ES8311 locks its internal PLL to MCLK, so the clock has to
/// be running first (and the ring is harmless while it is still silent).
fn bring_up(
    i2c: I2C0<'static>,
    i2s: I2S0<'static>,
    dma: DMA_CH0<'static>,
    delay: &mut esp_hal::delay::Delay,
    w: Wiring,
) -> Result<I2sTxDmaTransfer<'static, Blocking, DmaTxStreamBuf>, &'static str> {
    let mut i2c = I2c::new(i2c, I2cConfig::default().with_frequency(I2C_HZ))
        .map_err(|_| "i2c config")?
        .with_sda(w.sda)
        .with_scl(w.scl);
    // Probe before touching the I2S: an absent codec is the common case on a
    // board with a dead codec rail, and it should not cost us the display.
    i2c.write(ES8311_ADDR, &[0x00, 0x1F])
        .map_err(|_| "ES8311 does not answer on I2C")?;

    let config = TdmConfig::new_tdm_philips().with_sample_rate(Rate::from_hz(ad::SAMPLE_RATE));
    let i2s = I2s::new(i2s, dma, config).map_err(|_| "i2s config")?;
    let i2s = i2s.with_mclk(w.mclk);
    let tx = i2s
        .i2s_tx
        .with_bclk(w.bclk)
        .with_ws(w.ws)
        .with_dout(w.dout)
        .build();

    let (_rx_buf, _rx_desc, tx_buf, tx_desc) =
        esp_hal::dma_buffers_chunk_size!(4, STREAM_BYTES, CHUNK);
    let tx_desc = DmaAlignedMut::new(tx_desc).map_err(|_| "dma descriptor alignment")?;
    let tx_buf = DmaAlignedMut::new(tx_buf).map_err(|_| "dma buffer alignment")?;
    let mut stream = DmaTxStreamBuf::new(tx_desc, tx_buf).map_err(|_| "dma stream buffer")?;

    // Hand the DMA a few descriptors of silence (the buffer starts zeroed) so
    // the write cursor starts mid-ring and the first `pump()` has somewhere to
    // write; see PREFILL_CHUNKS.
    stream.push_with(|b| (PREFILL_CHUNKS * CHUNK).min(b.len()));
    let tx = tx.write(stream).map_err(|_| "i2s start")?;

    codec_init(&mut i2c, delay)?;
    Ok(tx)
}

/// ES8311 playback-only setup, in the three phases the ESP-IDF driver uses:
/// open → sample rate/clock → start (power-up, unmute, volume).
///
/// Values are transcribed from `espressif/esp-adf` (esp_codec_dev v1.6.2, the
/// same version the ai-passport BSP links against): `es8311_open()`,
/// `es8311_config_sample()` with the `coeff_div[]` row for
/// `(mclk 2822400, rate 11025)`, and `es8311_start()` + the volume writes that
/// follow it. `no_dac_ref = true` (REG44 = 0x08) matches the BSP's codec config.
fn codec_init(
    i2c: &mut I2c<'_, Blocking>,
    delay: &mut esp_hal::delay::Delay,
) -> Result<(), &'static str> {
    const INIT: &[(u8, u8)] = &[
        (0x0D, 0xFA), // force the analog power-down baseline
        (0x44, 0x08), // GPIO/I2C noise immunity; written twice on purpose —
        (0x44, 0x08), // the first I2C write after power-up can be dropped
        (0x01, 0x30), // clock gates: MCLK + BCLK on, ADC/DAC/analog still off
        (0x02, 0x00), // pre-divider 1, pre-multiplier x1 -> DIG_MCLK = MCLK
        (0x03, 0x10), // single speed, ADC OSR 0x10
        (0x16, 0x24), // ADC mic PGA baseline
        (0x04, 0x10), // DAC OSR (rate-dependent, rewritten below)
        (0x05, 0x00), // ADC/DAC clock dividers 1
        (0x0B, 0x00),
        (0x0C, 0x00),
        (0x10, 0x1F),
        (0x11, 0x7F),
        (0x00, 0x80), // leave reset, CSM_ON = 1, MSC = 0 (I2S slave)
        (0x01, 0x3F), // all internal clocks on, MCLK from the MCLK pin
        (0x06, 0x10), // clear BCLK invert (slave: not used for generation)
        (0x13, 0x10), // HPSW: enable the output driver to the PA
        (0x1B, 0x0A), // ADC ALC
        (0x1C, 0x6A), // ADC EQ bypass + HPF
        (0x44, 0x08), // no_dac_ref = true
        (0x09, 0x0C), // SDP_IN: 16-bit, I2S/Philips
        (0x0A, 0x0C), // SDP_OUT: same
        // --- sample-rate block, fs = 11025 Hz with MCLK = 256 x fs ---
        (0x02, 0x00),
        (0x05, 0x00),
        (0x03, 0x10),
        (0x04, 0x20), // DAC OSR 0x20 for 11.025 kHz
        (0x07, 0x00), // LRCK divider high
        (0x08, 0xFF), // LRCK divider low
        (0x06, 0x03), // BCLK divider — do not touch after this point
        // --- start ---
        (0x00, 0x80),
        (0x01, 0x3F),
        (0x09, 0x0C),
        (0x0A, 0x0C),
        (0x17, 0xBF), // ADC digital volume 0 dB
        (0x0E, 0x02), // analog bias / modulator enable
        (0x12, 0x00), // PDN_DAC = 0: DAC powered up
        (0x14, 0x1A), // line select, PGA = 10
        (0x0D, 0x01), // analog bias / VREF / VMID power-up
        (0x15, 0x40), // ADC ramp rate
        (0x37, 0x08), // DAC EQ bypass
        (0x45, 0x00),
        (0x31, DAC_MUTE_OFF),
        (0x32, DAC_VOLUME), // 0.5 dB/step; without this the codec stays mute
    ];
    for &(reg, val) in INIT {
        i2c.write(ES8311_ADDR, &[reg, val])
            .map_err(|_| "ES8311 register write failed")?;
    }
    delay.delay_millis(2);

    // Read a plain RW register back: proves the codec really answered and the
    // writes landed, which a write-only check cannot (see bsp_audio.c's
    // "ES8311 does not answer" diagnostics).
    let mut back = [0u8; 1];
    i2c.write_read(ES8311_ADDR, &[0x32], &mut back)
        .map_err(|_| "ES8311 read-back failed")?;
    if back[0] != DAC_VOLUME {
        return Err("ES8311 read-back mismatch");
    }
    Ok(())
}

/// DAC digital volume, REG32: 0.5 dB per step, 0x00 = -95.5 dB, 0xBF = 0 dB.
/// 0xB9 is about -3 dB: the digital mix runs with a deliberate ~4.5 dB of
/// headroom (see `LEVELS`), so the loudness is made up here instead — with
/// 16-bit samples this costs nothing in resolution.
const DAC_VOLUME: u8 = 0xB9;
/// REG31 with DSMMUTE (bit 6) and DEMMUTE (bit 5) clear.
const DAC_MUTE_OFF: u8 = 0x00;
