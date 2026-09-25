//! Pure voice-mixing logic for the audio path.
//!
//! The firmware keeps three voices live at once — the current cry, the
//! looping BGM and a one-shot SFX — and mixes them into the I2S DMA ring.
//! Everything here is `no_std`, allocation-free and host-testable: the
//! blobs are plain `&[u8]` slices handed in by the caller, so the same code
//! runs on the target and in `cargo test`.

/// 8-bit unsigned PCM (silence = 128) to signed 16-bit for the DAC.
#[inline]
pub fn to_i16(sample: u8) -> i16 {
    ((sample as i16) - 128) << 8
}

/// Saturating sum of the three mix buses.
#[inline]
pub fn mix3(a: i32, b: i32, c: i32) -> i16 {
    let s = a + b + c;
    s.clamp(i16::MIN as i32, i16::MAX as i32) as i16
}

/// A clip player: a cursor over one window of a blob, with a gain.
#[derive(Debug, Clone, Copy)]
pub struct Voice {
    off: u32,
    len: u32,
    pos: u32,
    /// 0..=255, 255 = unity (already includes the channel level).
    gain: u16,
    looping: bool,
    active: bool,
}

impl Voice {
    pub const fn new() -> Self {
        Self {
            off: 0,
            len: 0,
            pos: 0,
            gain: 255,
            looping: false,
            active: false,
        }
    }

    /// Start playing `len` bytes of `blob` from `off`, scaled by `gain`
    /// (0..=255). A zero length plays nothing.
    pub fn start(&mut self, off: u32, len: u32, gain: u8) {
        self.off = off;
        self.len = len;
        self.pos = 0;
        self.gain = gain as u16;
        self.active = len > 0;
    }

    pub fn stop(&mut self) {
        self.active = false;
        self.pos = 0;
        self.len = 0;
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Playback position in samples (used for SFX-triggered timing).
    pub fn pos(&self) -> u32 {
        self.pos
    }

    pub fn set_looping(&mut self, looping: bool) {
        self.looping = looping;
    }

    /// Advance one sample. Returns 0 once the clip ends (or while idle).
    #[inline]
    pub fn next(&mut self, blob: &[u8]) -> i16 {
        if !self.active {
            return 0;
        }
        if self.pos >= self.len {
            if self.looping {
                self.pos = 0;
            } else {
                self.active = false;
                return 0;
            }
        }
        let idx = self.off + self.pos;
        self.pos += 1;
        let raw = match blob.get(idx as usize) {
            Some(&b) => b,
            // A table pointing past the blob must not wedge the voice on.
            None => {
                self.active = false;
                return 0;
            }
        };
        (to_i16(raw) as i32 * self.gain as i32 / 255) as i16
    }
}

impl Default for Voice {
    fn default() -> Self {
        Self::new()
    }
}

/// Linear level ramp, stepped once per DMA block (a few ms), used to duck
/// the BGM under a cry without audible zipper noise.
#[derive(Debug, Clone, Copy)]
pub struct Ramp {
    level: u16,
    step: u16,
}

impl Ramp {
    /// `step` is the per-block change in 0..=255 units.
    pub const fn new(level: u16, step: u16) -> Self {
        Self { level, step }
    }

    pub fn level(&self) -> u16 {
        self.level
    }

    /// Move one step toward `target`, without overshooting.
    pub fn toward(&mut self, target: u16) {
        if self.level < target {
            self.level = (self.level + self.step).min(target);
        } else if self.level > target {
            self.level = self.level.saturating_sub(self.step).max(target);
        }
    }
}

/// Playback levels per source (0..=255 = unity is the clip's own gain, which
/// is already folded in when a voice starts).
#[derive(Debug, Clone, Copy)]
pub struct Levels {
    pub cry: u8,
    pub music: u8,
    /// Music level while the cry bus is active.
    pub music_ducked: u8,
    pub sfx: u8,
    /// Ducking speed in level units per block.
    pub duck_step: u16,
}

/// The three blobs the mix reads from, as `&'static` slices so `Mixer` can be
/// a plain `const`-constructible struct on the target.
#[derive(Clone, Copy)]
pub struct Blobs {
    pub cry: &'static [u8],
    pub music: &'static [u8],
    pub sfx: &'static [u8],
}

/// Three-voice mixer feeding 16-bit stereo I2S blocks.
///
/// Voices are addressed by `(off, len, gain)` into the blobs: the dex/index
/// tables live in the firmware, the arithmetic lives here so it can be tested
/// on the host.
pub struct Mixer {
    cry: Voice,
    music: Voice,
    sfx: Voice,
    duck: Ramp,
    levels: Levels,
    blobs: Blobs,
}

impl Mixer {
    pub const fn new(blobs: Blobs, levels: Levels) -> Self {
        Self {
            cry: Voice::new(),
            music: Voice::new(),
            sfx: Voice::new(),
            duck: Ramp::new(levels.music as u16, levels.duck_step),
            levels,
            blobs,
        }
    }

    /// Start (or restart) the cry voice.
    pub fn play_cry(&mut self, off: u32, len: u32, gain: u8) {
        self.cry.start(off, len, scale(gain, self.levels.cry));
    }

    /// Start a one-shot sound effect, interrupting the previous one.
    pub fn play_sfx(&mut self, off: u32, len: u32, gain: u8) {
        self.sfx.start(off, len, scale(gain, self.levels.sfx));
    }

    /// Start the music voice: `looping` for BGM, not for the boot jingle.
    pub fn play_music(&mut self, off: u32, len: u32, gain: u8, looping: bool) {
        self.music.start(off, len, scale(gain, self.levels.music));
        self.music.set_looping(looping);
    }

    pub fn cry_active(&self) -> bool {
        self.cry.is_active()
    }

    pub fn music_active(&self) -> bool {
        self.music.is_active()
    }

    /// Fill one stereo 16-bit block, sampling all three voices. The block size
    /// must be a multiple of 4 (one stereo frame).
    pub fn fill(&mut self, out: &mut [u8]) {
        // The duck ramp advances once per block (a few dozen ms): inaudible as
        // a ramp, and free per sample.
        self.duck.toward(if self.cry.is_active() {
            self.levels.music_ducked as u16
        } else {
            self.levels.music as u16
        });
        let music_level = self.duck.level() as i32;

        for frame in out.chunks_exact_mut(4) {
            let cry = self.cry.next(self.blobs.cry) as i32;
            let music = self.music.next(self.blobs.music) as i32 * music_level / 255;
            let sfx = self.sfx.next(self.blobs.sfx) as i32;
            let s = mix3(cry, music, sfx);
            // The material is mono; both slots get the same sample so a stereo
            // codec needs no special mono mode.
            let b = s.to_le_bytes();
            frame[0] = b[0];
            frame[1] = b[1];
            frame[2] = b[0];
            frame[3] = b[1];
        }
    }
}

fn scale(gain: u8, level: u8) -> u8 {
    ((gain as u16 * level as u16) / 255) as u8
}

/// Tiny xorshift32 PRNG: picks the boot BGM from the track list. Seeded from
/// the hardware RNG at startup, so the choice differs per boot.
#[derive(Debug, Clone, Copy)]
pub struct Rng(u32);

impl Rng {
    pub fn new(seed: u32) -> Self {
        // xorshift is stuck at zero; the low bits of a timer read can be
        // zero right after reset, so fold in a constant.
        Self(seed ^ 0x9E37_79B9)
    }

    pub fn next_u32(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        x
    }

    /// Uniform-enough value in `0..n` (0 when `n` is 0).
    pub fn below(&mut self, n: u32) -> u32 {
        if n == 0 {
            0
        } else {
            self.next_u32() % n
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsigned_pcm_maps_to_signed() {
        assert_eq!(to_i16(128), 0);
        assert_eq!(to_i16(255), 32512);
        assert_eq!(to_i16(0), -32768);
    }

    #[test]
    fn voice_plays_its_window_then_stops() {
        let blob = [128u8, 200, 100, 50];
        let mut v = Voice::new();
        v.start(0, 3, 255);
        assert_eq!(v.next(&blob), 0);
        assert!(v.next(&blob) > 0);
        assert!(v.next(&blob) < 0);
        assert_eq!(v.next(&blob), 0, "past the end is silence");
        assert!(!v.is_active());
    }

    #[test]
    fn voice_offset_and_gain_apply() {
        let blob = [0u8, 128, 255, 64];
        let mut v = Voice::new();
        v.start(2, 2, 255);
        assert_eq!(v.next(&blob), to_i16(255));
        // half gain
        let mut v = Voice::new();
        v.start(2, 2, 128);
        assert_eq!(v.next(&blob), (to_i16(255) as i32 * 128 / 255) as i16);
    }

    #[test]
    fn looping_voice_wraps_instead_of_ending() {
        let blob = [128u8, 255];
        let mut v = Voice::new();
        v.start(0, 2, 255);
        v.set_looping(true);
        for _ in 0..10 {
            v.next(&blob);
            assert!(v.is_active());
        }
        v.set_looping(false);
        v.next(&blob);
        v.next(&blob);
        assert!(!v.is_active(), "non-looping voice stops at the end");
    }

    #[test]
    fn voice_out_of_range_blob_is_silent_not_a_panic() {
        let blob = [128u8];
        let mut v = Voice::new();
        v.start(0, 8, 255);
        for _ in 0..4 {
            v.next(&blob);
        }
        assert!(!v.is_active());
    }

    #[test]
    fn mix_saturates_instead_of_wrapping() {
        assert_eq!(mix3(1000, 2000, -500), 2500);
        assert_eq!(mix3(i16::MAX as i32, i16::MAX as i32, 0), i16::MAX);
        assert_eq!(mix3(i16::MIN as i32, -1000, 0), i16::MIN);
    }

    #[test]
    fn ramp_steps_toward_target_without_overshoot() {
        let mut r = Ramp::new(255, 20);
        r.toward(60);
        assert_eq!(r.level(), 235);
        for _ in 0..20 {
            r.toward(60);
        }
        assert_eq!(r.level(), 60, "stops exactly on the target");
        r.toward(255);
        for _ in 0..30 {
            r.toward(255);
        }
        assert_eq!(r.level(), 255);
    }

    #[test]
    fn rng_varies_and_stays_in_range() {
        let mut r = Rng::new(0);
        let mut seen = [false; 4];
        for _ in 0..64 {
            let i = r.below(4);
            assert!(i < 4);
            seen[i as usize] = true;
        }
        assert!(seen.iter().all(|&s| s), "all tracks get picked eventually");
        assert_eq!(Rng::new(0).below(0), 0, "zero range is safe");
    }

    #[test]
    fn rng_same_seed_same_sequence() {
        let a: heapless::Vec<u32, 8> = {
            let mut r = Rng::new(42);
            let mut v = heapless::Vec::new();
            for _ in 0..8 {
                let _ = v.push(r.next_u32());
            }
            v
        };
        let b: heapless::Vec<u32, 8> = {
            let mut r = Rng::new(42);
            let mut v = heapless::Vec::new();
            for _ in 0..8 {
                let _ = v.push(r.next_u32());
            }
            v
        };
        assert_eq!(a, b);
    }

    // --- mixer -------------------------------------------------------------

    /// 64 frames of a constant tone on every bus, so a block's energy is a
    /// direct read of that bus's level.
    static TONE: [u8; 64] = [200u8; 64];

    fn blobs() -> Blobs {
        Blobs {
            cry: &TONE,
            music: &TONE,
            sfx: &TONE,
        }
    }

    fn levels() -> Levels {
        Levels {
            cry: 255,
            music: 128,
            music_ducked: 32,
            sfx: 255,
            duck_step: 16,
        }
    }

    fn energy(buf: &[u8]) -> u64 {
        buf.chunks_exact(4)
            .map(|c| i16::from_le_bytes([c[0], c[1]]).unsigned_abs() as u64)
            .sum()
    }

    fn block() -> [u8; 64] {
        [0u8; 64]
    }

    #[test]
    fn silent_mixer_emits_digital_silence() {
        let mut m = Mixer::new(blobs(), levels());
        let mut out = block();
        m.fill(&mut out);
        assert!(out.iter().all(|&b| b == 0), "idle voices must be silent");
    }

    #[test]
    fn stereo_slots_carry_the_same_sample() {
        let mut m = Mixer::new(blobs(), levels());
        m.play_cry(0, TONE.len() as u32, 255);
        let mut out = block();
        m.fill(&mut out);
        for f in out.chunks_exact(4) {
            assert_eq!(f[..2], f[2..], "mono material must be duplicated");
        }
        assert!(energy(&out) > 0);
    }

    #[test]
    fn music_is_ducked_while_a_cry_plays() {
        // Cry bus carries digital silence, so a block's energy is exactly the
        // music bus: the drop then measures the ducking alone.
        static SILENCE: [u8; 64] = [128u8; 64];
        let mut m = Mixer::new(
            Blobs {
                cry: &SILENCE,
                music: &TONE,
                sfx: &TONE,
            },
            levels(),
        );
        m.play_music(0, TONE.len() as u32, 255, true);
        let mut out = block();
        for _ in 0..8 {
            m.fill(&mut out);
        }
        let loud = energy(&out);
        assert!(loud > 0, "music alone must be audible");

        m.play_cry(0, 64, 255); // silent, but holds the cry bus busy
        for _ in 0..8 {
            m.play_cry(0, 64, 255); // re-arm: 64 samples < one 128-frame run
            m.fill(&mut out);
        }
        assert!(m.cry_active());
        let ducked = energy(&out);
        assert!(
            ducked < loud / 2,
            "music behind a cry must drop well below its normal level \
             (loud {loud}, ducked {ducked})"
        );
    }

    #[test]
    fn clips_restart_when_retriggered() {
        let mut m = Mixer::new(blobs(), levels());
        m.play_sfx(0, TONE.len() as u32, 255);
        let mut out = block();
        m.fill(&mut out);
        assert!(energy(&out) > 0);
        m.play_sfx(0, TONE.len() as u32, 255); // restart
        let mut out2 = block();
        m.fill(&mut out2);
        assert_eq!(energy(&out), energy(&out2), "a retrigger restarts the clip");
    }

    #[test]
    fn a_finished_one_shot_does_not_hold_the_bus() {
        let mut m = Mixer::new(blobs(), levels());
        m.play_sfx(0, 8, 255); // shorter than one 16-frame block
        let mut out = block();
        m.fill(&mut out);
        let mut after = block();
        m.fill(&mut after);
        assert_eq!(energy(&after), 0, "sfx stops after its length");
    }
}
