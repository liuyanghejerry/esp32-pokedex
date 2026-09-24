//! Pure, host-testable logic for the Pokedex firmware.
//!
//! Everything here is independent from ESP-IDF/esp-hal/LVGL: button
//! classification from the GPIO0 ADC ladder, debounce/repeat, the dex
//! navigation model, type tables, and text wrapping. The firmware crate
//! renders whatever state these produce.

#![no_std]

#[cfg(test)]
#[macro_use]
extern crate std;

/// Physical buttons sharing the ADC ladder on GPIO0.
/// Windows (mV) come from ai-passport `bsp_pins.h`:
/// UP {0,150}, DOWN {150,447}, OK {447,1900}, released ~3300.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    Up,
    Down,
    Ok,
}

pub const MV_UP_MAX: u16 = 150;
pub const MV_DOWN_MAX: u16 = 447;
pub const MV_OK_MAX: u16 = 1900;

/// Map a millivolt reading to the pressed button, if any.
pub fn classify_mv(mv: u16) -> Option<Button> {
    if mv < MV_UP_MAX {
        Some(Button::Up)
    } else if mv < MV_DOWN_MAX {
        Some(Button::Down)
    } else if mv < MV_OK_MAX {
        Some(Button::Ok)
    } else {
        None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonEvent {
    /// A button was just pressed (debounced rising edge).
    Press(Button),
    /// Auto-repeat while the button stays held.
    Repeat(Button),
}

/// Sampling cadence the debouncer is fed at (main loop poll period).
pub const TICK_MS: u32 = 20;
/// Consecutive identical samples required to accept a state change.
const STABLE_N: u8 = 3;
/// Hold time before the first auto-repeat fires.
const REPEAT_DELAY_MS: u32 = 350;
/// Interval between subsequent auto-repeats.
const REPEAT_INTERVAL_MS: u32 = 150;

/// Debounces ADC samples into press/repeat events.
///
/// `feed` must be called once every `TICK_MS`; the caller's poll loop
/// guarantees that cadence.
#[derive(Debug)]
pub struct Debouncer {
    candidate: Option<Button>,
    run: u8,
    stable: Option<Button>,
    held_ms: u32,
    repeat_count: u32,
}

impl Debouncer {
    pub fn new() -> Self {
        Self {
            candidate: None,
            run: 0,
            stable: None,
            held_ms: 0,
            repeat_count: 0,
        }
    }

    /// Feed one ADC sample (mV); returns an event when one fires.
    pub fn feed(&mut self, mv: u16) -> Option<ButtonEvent> {
        let cur = classify_mv(mv);
        if cur != self.candidate {
            self.candidate = cur;
            self.run = 0;
        }
        self.run = self.run.saturating_add(1);
        if self.run == STABLE_N && self.stable != cur {
            self.stable = cur;
            self.held_ms = 0;
            self.repeat_count = 0;
            if let Some(b) = cur {
                return Some(ButtonEvent::Press(b));
            }
        }
        if self.stable.is_some() {
            self.held_ms = self.held_ms.saturating_add(TICK_MS);
            let next_at = if self.repeat_count == 0 {
                REPEAT_DELAY_MS
            } else {
                REPEAT_DELAY_MS + self.repeat_count * REPEAT_INTERVAL_MS
            };
            if self.held_ms >= next_at {
                self.repeat_count += 1;
                return self.stable.map(ButtonEvent::Repeat);
            }
        }
        None
    }
}

impl Default for Debouncer {
    fn default() -> Self {
        Self::new()
    }
}

/// The two screens of the dex.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    /// Sprite card: name, category, types, height/weight.
    Card,
    /// Detail: base stats and flavor text.
    Detail,
}

/// Navigation state over `count` entries (1-based dex numbers).
#[derive(Debug)]
pub struct DexModel {
    count: usize,
    /// 0-based index into the entry table.
    index: usize,
    page: Page,
}

impl DexModel {
    pub fn new(count: usize, start_no: usize) -> Self {
        let count = count.max(1);
        Self {
            count,
            index: start_no.saturating_sub(1).min(count - 1),
            page: Page::Card,
        }
    }

    pub fn no(&self) -> usize {
        self.index + 1
    }

    pub fn page(&self) -> Page {
        self.page
    }

    /// Apply one button event. Returns true when the view must be redrawn.
    pub fn handle(&mut self, ev: ButtonEvent) -> bool {
        match ev {
            ButtonEvent::Press(Button::Up) | ButtonEvent::Repeat(Button::Up) => {
                self.index = (self.index + self.count - 1) % self.count;
                self.page = Page::Card;
                true
            }
            ButtonEvent::Press(Button::Down) | ButtonEvent::Repeat(Button::Down) => {
                self.index = (self.index + 1) % self.count;
                self.page = Page::Card;
                true
            }
            ButtonEvent::Press(Button::Ok) => {
                self.page = match self.page {
                    Page::Card => Page::Detail,
                    Page::Detail => Page::Card,
                };
                true
            }
            ButtonEvent::Repeat(Button::Ok) => false,
        }
    }
}

/// GBA type ids as used by the source data (255 = none).
pub const TYPE_NONE: u8 = 255;

/// Display name for a type id.
pub fn type_name(id: u8) -> &'static str {
    match id {
        0 => "NORMAL",
        1 => "FIGHTING",
        2 => "FLYING",
        3 => "POISON",
        4 => "GROUND",
        5 => "ROCK",
        6 => "BUG",
        7 => "GHOST",
        8 => "STEEL",
        9 => "MYSTERY",
        10 => "FIRE",
        11 => "WATER",
        12 => "GRASS",
        13 => "ELECTRIC",
        14 => "PSYCHIC",
        15 => "ICE",
        16 => "DRAGON",
        17 => "DARK",
        _ => "???",
    }
}

/// Badge color (24-bit RGB) for a type id.
pub fn type_rgb(id: u8) -> (u8, u8, u8) {
    match id {
        0 => (168, 168, 120),
        1 => (192, 48, 40),
        2 => (168, 144, 240),
        3 => (160, 64, 160),
        4 => (224, 192, 104),
        5 => (184, 160, 56),
        6 => (168, 184, 32),
        7 => (112, 88, 152),
        8 => (184, 184, 208),
        9 => (104, 160, 144),
        10 => (240, 128, 48),
        11 => (104, 144, 240),
        12 => (120, 200, 80),
        13 => (248, 208, 48),
        14 => (248, 88, 136),
        15 => (152, 216, 216),
        16 => (112, 56, 248),
        17 => (112, 88, 72),
        _ => (120, 120, 120),
    }
}

/// Longest flavor line count we ever expect to need room for.
pub const MAX_WRAP_LINES: usize = 12;

/// Greedy word wrap: splits `text` into lines of at most `max_chars`
/// characters, breaking at spaces. A word longer than `max_chars` is
/// hard-split. Trailing/leading whitespace is collapsed.
pub fn wrap_text(text: &str, max_chars: usize) -> heapless::Vec<&str, MAX_WRAP_LINES> {
    let mut lines = heapless::Vec::new();
    let mut line_start = 0usize;
    let mut line_len = 0usize;
    let mut last_space: Option<usize> = None;
    let bytes = text.as_bytes();
    let mut i = 0usize;
    let _ = line_start;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b' ' {
            if line_len == 0 {
                line_start = i + 1;
                i += 1;
                continue;
            }
            if line_len + 1 > max_chars {
                let _ = lines.push(&text[line_start..i]);
                line_start = i + 1;
                line_len = 0;
                last_space = None;
                i += 1;
                continue;
            }
            last_space = Some(i);
            line_len += 1;
        } else {
            if line_len == max_chars {
                let (break_at, after) = match last_space {
                    Some(s) => (s, s + 1),
                    None => (i, i),
                };
                let seg = text[line_start..break_at].trim_end();
                if !seg.is_empty() {
                    let _ = lines.push(seg);
                }
                line_start = after;
                line_len = i + 1 - line_start;
                last_space = None;
                i += 1;
                continue;
            }
            line_len += 1;
        }
        i += 1;
    }
    if line_start < bytes.len() {
        let seg = text[line_start..].trim_end();
        if !seg.is_empty() {
            let _ = lines.push(seg);
        }
    }
    lines
}

/// Length in pixels of a base-stat bar (at least 1 px for nonzero stats).
pub fn stat_bar_len(value: u8, max_px: u16) -> u16 {
    if value == 0 {
        return 0;
    }
    let len = (u16::from(value) * max_px) / 255;
    len.max(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec::Vec;

    #[test]
    fn classify_matches_bsp_windows() {
        assert_eq!(classify_mv(0), Some(Button::Up));
        assert_eq!(classify_mv(149), Some(Button::Up));
        assert_eq!(classify_mv(150), Some(Button::Down));
        assert_eq!(classify_mv(446), Some(Button::Down));
        assert_eq!(classify_mv(447), Some(Button::Ok));
        assert_eq!(classify_mv(1899), Some(Button::Ok));
        assert_eq!(classify_mv(1900), None);
        assert_eq!(classify_mv(3300), None);
    }

    fn feed_seq(d: &mut Debouncer, mv: u16, n: u32) -> Vec<ButtonEvent> {
        let mut out = Vec::new();
        for _ in 0..n {
            if let Some(ev) = d.feed(mv) {
                out.push(ev);
            }
        }
        out
    }

    #[test]
    fn short_press_emits_single_press() {
        let mut d = Debouncer::new();
        // tap: 5 samples down, 5 released
        let evs = feed_seq(&mut d, 300, 5);
        assert_eq!(evs, vec![ButtonEvent::Press(Button::Down)]);
        let evs = feed_seq(&mut d, 3300, 5);
        assert!(evs.is_empty());
    }

    #[test]
    fn bounce_below_stable_threshold_is_ignored() {
        let mut d = Debouncer::new();
        // noise: 2 samples Up, 1 sample idle, 2 samples Up, 3 samples Down
        let evs = feed_seq(&mut d, 0, 2);
        assert!(evs.is_empty());
        let evs = feed_seq(&mut d, 3300, 1);
        assert!(evs.is_empty());
        let evs = feed_seq(&mut d, 0, 2);
        assert!(evs.is_empty());
        let evs = feed_seq(&mut d, 300, 3);
        assert_eq!(evs, vec![ButtonEvent::Press(Button::Down)]);
    }

    #[test]
    fn hold_generates_repeats_at_fixed_cadence() {
        let mut d = Debouncer::new();
        let mut presses = Vec::new();
        // hold Up for 1000 ms = 50 ticks
        for _ in 0..50 {
            if let Some(ev) = d.feed(10) {
                presses.push(ev);
            }
        }
        assert_eq!(presses[0], ButtonEvent::Press(Button::Up));
        let repeats: Vec<ButtonEvent> = presses[1..].to_vec();
        // 1s hold @ 20ms ticks: first repeat at ~360ms, then every ~150ms
        // (quantized to tick boundaries) => 4 repeats within 50 ticks.
        assert_eq!(repeats.len(), 4);
        assert!(repeats
            .iter()
            .all(|e| matches!(e, ButtonEvent::Repeat(Button::Up))));
    }

    #[test]
    fn model_navigates_and_wraps() {
        let mut m = DexModel::new(3, 1);
        assert_eq!(m.no(), 1);
        assert_eq!(m.page(), Page::Card);
        assert!(m.handle(ButtonEvent::Press(Button::Down)));
        assert_eq!(m.no(), 2);
        assert!(m.handle(ButtonEvent::Press(Button::Down)));
        assert_eq!(m.no(), 3);
        assert!(m.handle(ButtonEvent::Press(Button::Down)));
        assert_eq!(m.no(), 1, "must wrap to first");
        assert!(m.handle(ButtonEvent::Press(Button::Up)));
        assert_eq!(m.no(), 3, "must wrap backwards");
    }

    #[test]
    fn model_toggles_page_and_resets_on_nav() {
        let mut m = DexModel::new(10, 5);
        assert!(m.handle(ButtonEvent::Press(Button::Ok)));
        assert_eq!(m.page(), Page::Detail);
        assert!(!m.handle(ButtonEvent::Repeat(Button::Ok)));
        assert!(m.handle(ButtonEvent::Press(Button::Down)));
        assert_eq!(m.page(), Page::Card, "navigating returns to card page");
        assert_eq!(m.no(), 6);
    }

    #[test]
    fn type_tables_cover_all_ids() {
        for id in 0u8..=17 {
            assert_ne!(type_name(id), "???");
            let (r, g, b) = type_rgb(id);
            assert!(r > 40 || g > 40 || b > 40, "type {id} color too dark");
        }
        assert_eq!(type_name(TYPE_NONE), "???");
    }

    #[test]
    fn wrap_splits_at_spaces() {
        let lines = wrap_text("There is a plant seed on its back right", 12);
        assert_eq!(lines.len(), 4);
        for l in lines.iter() {
            assert!(l.len() <= 12, "line too long: {l:?}");
        }
        assert_eq!(lines[0], "There is a");
    }

    #[test]
    fn wrap_hard_splits_long_words() {
        let lines = wrap_text("AAAAAAAAAAAAAAAABBBB", 10);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], "AAAAAAAAAA");
        assert_eq!(lines[1], "AAAAAABBBB");
    }

    #[test]
    fn wrap_collapses_whitespace() {
        let lines = wrap_text("  hello   world  ", 10);
        assert_eq!(lines.len(), 2, "\"hello world\" is 11 chars, must wrap");
        assert_eq!(lines[0], "hello");
        assert_eq!(lines[1], "world");
    }

    #[test]
    fn stat_bar_scales_and_has_minimum() {
        assert_eq!(stat_bar_len(0, 100), 0);
        assert_eq!(stat_bar_len(1, 100), 1);
        assert_eq!(stat_bar_len(255, 100), 100);
        assert!(stat_bar_len(100, 100) > stat_bar_len(50, 100));
    }
}
