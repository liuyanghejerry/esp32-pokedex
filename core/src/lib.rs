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

pub mod audio;

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

/// What `DexModel::handle` did with an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Nothing changed (e.g. auto-repeat of OK).
    None,
    /// Switched to a neighbouring species (page is kept).
    Navigate,
    /// Toggled between card and detail page.
    TogglePage,
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

    /// Apply one button event. UP/DOWN move to the neighbouring species and
    /// keep the current page (so the detail view browses stat pages
    /// directly); OK toggles the page.
    pub fn handle(&mut self, ev: ButtonEvent) -> Action {
        match ev {
            ButtonEvent::Press(Button::Up) | ButtonEvent::Repeat(Button::Up) => {
                self.index = (self.index + self.count - 1) % self.count;
                Action::Navigate
            }
            ButtonEvent::Press(Button::Down) | ButtonEvent::Repeat(Button::Down) => {
                self.index = (self.index + 1) % self.count;
                Action::Navigate
            }
            ButtonEvent::Press(Button::Ok) => {
                self.page = match self.page {
                    Page::Card => Page::Detail,
                    Page::Detail => Page::Card,
                };
                Action::TogglePage
            }
            ButtonEvent::Repeat(Button::Ok) => Action::None,
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

/// Badge color (24-bit RGB) for a type id — the modern official palette
/// used by current games (Fire red, Electric yellow, ...).
pub fn type_rgb(id: u8) -> (u8, u8, u8) {
    match id {
        0 => (159, 161, 159),  // NORMAL
        1 => (255, 128, 0),    // FIGHTING
        2 => (129, 185, 239),  // FLYING
        3 => (145, 65, 203),   // POISON
        4 => (145, 81, 33),    // GROUND
        5 => (175, 169, 129),  // ROCK
        6 => (145, 161, 25),   // BUG
        7 => (112, 65, 112),   // GHOST
        8 => (96, 161, 184),   // STEEL
        9 => (104, 160, 144),  // MYSTERY
        10 => (230, 40, 41),   // FIRE
        11 => (41, 128, 239),  // WATER
        12 => (63, 161, 41),   // GRASS
        13 => (250, 192, 0),   // ELECTRIC
        14 => (239, 65, 121),  // PSYCHIC
        15 => (61, 206, 243),  // ICE
        16 => (80, 96, 225),   // DRAGON
        17 => (98, 77, 78),    // DARK
        _ => (120, 120, 120),
    }
}

/// True when a type badge background is light enough to need dark text
/// (perceived luminance > 165).
pub fn type_dark_text(id: u8) -> bool {
    let (r, g, b) = type_rgb(id);
    let lum = (299 * r as u32 + 587 * g as u32 + 114 * b as u32) / 1000;
    lum > 165
}

/// Longest flavor line count we ever expect to need room for.
pub const MAX_WRAP_LINES: usize = 12;

/// Chinese display name for a type id (official localization).
pub fn type_name_zh(id: u8) -> &'static str {
    match id {
        0 => "一般",
        1 => "格斗",
        2 => "飞行",
        3 => "毒",
        4 => "地面",
        5 => "岩石",
        6 => "虫",
        7 => "幽灵",
        8 => "钢",
        9 => "未知",
        10 => "火",
        11 => "水",
        12 => "草",
        13 => "电",
        14 => "超能力",
        15 => "冰",
        16 => "龙",
        17 => "恶",
        _ => "???",
    }
}

/// Line-start-forbidden punctuation for CJK wrapping (closing marks).
const ZH_NO_START: &str = "，。、！？；：）】》」』…·—～％%";

/// Display width of one char in halfwidth units: ASCII = 1, else 2.
fn zh_units(c: char) -> usize {
    if c.is_ascii() {
        1
    } else {
        2
    }
}

/// Total display width of a string in halfwidth units.
pub fn zh_width(text: &str) -> usize {
    text.chars().map(zh_units).sum()
}

/// Wrap CJK text to `max_units` halfwidth units per line, breaking between
/// any two characters (standard CJK behaviour), but never starting a line
/// with closing punctuation — such a mark is pulled back onto the previous
/// line even if that line then slightly overflows.
pub fn wrap_zh(text: &str, max_units: usize) -> heapless::Vec<&str, MAX_WRAP_LINES> {
    let mut lines = heapless::Vec::new();
    let mut start = 0usize;
    let mut units = 0usize;
    for (i, ch) in text.char_indices() {
        let ch_units = zh_units(ch);
        if units + ch_units > max_units && i > start {
            let (end, carry) = if ZH_NO_START.contains(ch) {
                (i + ch.len_utf8(), 0)
            } else {
                (i, ch_units)
            };
            let _ = lines.push(&text[start..end]);
            start = end;
            units = carry;
        } else {
            units += ch_units;
        }
    }
    if start < text.len() {
        let _ = lines.push(&text[start..]);
    }
    lines
}

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
        assert!(m.handle(ButtonEvent::Press(Button::Down)) != Action::None);
        assert_eq!(m.no(), 2);
        m.handle(ButtonEvent::Press(Button::Down));
        assert_eq!(m.no(), 3);
        m.handle(ButtonEvent::Press(Button::Down));
        assert_eq!(m.no(), 1, "must wrap to first");
        m.handle(ButtonEvent::Press(Button::Up));
        assert_eq!(m.no(), 3, "must wrap backwards");
    }

    #[test]
    fn model_keeps_page_when_navigating() {
        let mut m = DexModel::new(10, 5);
        assert_eq!(m.handle(ButtonEvent::Press(Button::Ok)), Action::TogglePage);
        assert_eq!(m.page(), Page::Detail);
        assert_eq!(m.handle(ButtonEvent::Repeat(Button::Ok)), Action::None);
        assert_eq!(
            m.handle(ButtonEvent::Press(Button::Down)),
            Action::Navigate
        );
        assert_eq!(
            m.page(),
            Page::Detail,
            "navigating from detail stays on detail"
        );
        assert_eq!(m.no(), 6);
        assert_eq!(m.handle(ButtonEvent::Press(Button::Up)), Action::Navigate);
        assert_eq!(m.no(), 5);
        assert_eq!(m.page(), Page::Detail);
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
    fn badge_text_contrast_matches_background() {
        // light backgrounds get dark text, dark ones white
        assert!(type_dark_text(13), "electric yellow needs dark text");
        assert!(type_dark_text(15), "ice cyan needs dark text");
        assert!(!type_dark_text(10), "fire red keeps white text");
        assert!(!type_dark_text(11), "water blue keeps white text");
        assert!(!type_dark_text(17), "dark keeps white text");
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

    #[test]
    fn zh_type_names_cover_all_ids() {
        for id in 0u8..=17 {
            assert!(!type_name_zh(id).starts_with("???"), "id {id}");
        }
        assert_eq!(type_name_zh(TYPE_NONE), "???");
    }

    #[test]
    fn zh_width_counts_halfwidth_units() {
        assert_eq!(zh_width("妙蛙种子"), 8);
        assert_eq!(zh_width("0.7m"), 4);
        assert_eq!(zh_width("妙蛙0.7m"), 8);
    }

    #[test]
    fn wrap_zh_breaks_by_units() {
        let lines = wrap_zh("妙蛙种子背上种着种子", 8);
        assert_eq!(lines.len(), 3);
        for l in lines.iter() {
            assert!(zh_width(l) <= 8, "line too wide: {l:?}");
        }
        assert_eq!(lines[0], "妙蛙种子");
        assert_eq!(lines[1], "背上种着");
    }

    #[test]
    fn wrap_zh_never_starts_line_with_closing_punct() {
        // 。 would be the overflowing char; the rule pulls it back onto
        // line 1 even though that line then exceeds the limit.
        let lines = wrap_zh("种子慢慢长大。测试", 12);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], "种子慢慢长大。");
        assert_eq!(lines[1], "测试");
    }

    #[test]
    fn wrap_zh_mixed_ascii() {
        let lines = wrap_zh("身高0.7m体重6.9kg", 8);
        assert_eq!(lines[0], "身高0.7m");
        for l in lines.iter() {
            assert!(zh_width(l) <= 8 + 2, "overflow beyond one punct: {l:?}");
        }
    }
}
