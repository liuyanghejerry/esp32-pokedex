//! Pokedex UI — original card-style design, drawn with embedded-graphics.
//!
//! Two pages: a sprite "card" (name, category, type badges, height/weight)
//! and a "detail" page (base-stat bars + FireRed flavor text). UP/DOWN
//! switch species, OK toggles the page.

use embedded_graphics::{
    mono_font::{ascii as font, MonoFont, MonoTextStyle, MonoTextStyleBuilder},
    pixelcolor::Rgb565,
    prelude::*,
    primitives::{Circle, Line, PrimitiveStyle, PrimitiveStyleBuilder, Rectangle},
    text::{Baseline, Text, TextStyleBuilder},
};

use pokedex_core::{stat_bar_len, type_name, type_rgb, wrap_text, DexModel, Page, TYPE_NONE};

use crate::dex_data::{DexEntry, DEX, DEX_LEN};
use crate::sprites::Sprite;
use crate::st7789::{FrameBuffer, HEIGHT, WIDTH};

const BG: Rgb565 = Rgb565::new(0x10, 0x16, 0x2B);
const HEADER_RED: Rgb565 = Rgb565::new(0xE3, 0x35, 0x0D);
const HEADER_RED_DARK: Rgb565 = Rgb565::new(0xB0, 0x28, 0x0A);
const STRIP: Rgb565 = Rgb565::new(0x0B, 0x10, 0x20);
const CARD_BORDER: Rgb565 = Rgb565::new(0x26, 0x30, 0x4F);
const WHITE: Rgb565 = Rgb565::new(0xFF, 0xFF, 0xFF);
const SUB: Rgb565 = Rgb565::new(0x9A, 0xA5, 0xC0);
const DIM: Rgb565 = Rgb565::new(0x8A, 0x93, 0xAD);
const DIVIDER: Rgb565 = Rgb565::new(0x2A, 0x33, 0x52);
const BAR_BG: Rgb565 = Rgb565::new(0x23, 0x2C, 0x48);
const PANEL: Rgb565 = Rgb565::new(0x1A, 0x23, 0x40);
const LENS: Rgb565 = Rgb565::new(0x12, 0x31, 0x5C);
const FLAVOR: Rgb565 = Rgb565::new(0xC7, 0xCE, 0xE0);

const STAT_COLORS: [Rgb565; 6] = [
    Rgb565::new(0x78, 0xC8, 0x50), // HP  green
    Rgb565::new(0xF0, 0x80, 0x30), // ATK orange
    Rgb565::new(0xF8, 0xD0, 0x30), // DEF yellow
    Rgb565::new(0x68, 0x90, 0xF0), // SPA blue
    Rgb565::new(0x98, 0xD8, 0xD8), // SPD cyan
    Rgb565::new(0xF8, 0x58, 0x88), // SPE pink
];

const STAT_LABELS: [&str; 6] = ["HP", "ATK", "DEF", "SPA", "SPD", "SPE"];

fn fill_rect(fb: &mut FrameBuffer, x: i32, y: i32, w: u32, h: u32, color: Rgb565) {
    Rectangle::new(Point::new(x, y), Size::new(w, h))
        .into_styled(PrimitiveStyle::with_fill(color))
        .draw(fb)
        .ok();
}

fn card_at(fb: &mut FrameBuffer, x: i32, y: i32, w: u32, h: u32) {
    Rectangle::new(Point::new(x, y), Size::new(w, h))
        .into_styled(
            PrimitiveStyleBuilder::new()
                .fill_color(WHITE)
                .stroke_color(CARD_BORDER)
                .stroke_width(2)
                .build(),
        )
        .draw(fb)
        .ok();
}

fn draw_sprite(fb: &mut FrameBuffer, s: &Sprite, x0: u32, y0: u32, scale: u32) {
    for y in 0..64u32 {
        for x in 0..64u32 {
            if s.opaque(x, y) {
                let color = s.color(x, y);
                for dy in 0..scale {
                    for dx in 0..scale {
                        fb.set(x0 + x * scale + dx, y0 + y * scale + dy, color);
                    }
                }
            }
        }
    }
}

fn text_top(fb: &mut FrameBuffer, s: &str, x: i32, y: i32, style: MonoTextStyle<'static, Rgb565>) {
    Text::with_text_style(
        s,
        Point::new(x, y),
        style,
        TextStyleBuilder::new().baseline(Baseline::Top).build(),
    )
    .draw(fb)
    .ok();
}

fn text_centered(
    fb: &mut FrameBuffer,
    s: &str,
    y: i32,
    char_w: i32,
    style: MonoTextStyle<'static, Rgb565>,
) {
    let x = (WIDTH as i32 - s.len() as i32 * char_w) / 2;
    text_top(fb, s, x, y, style);
}

fn white_on(f: &'static MonoFont<'static>) -> MonoTextStyle<'static, Rgb565> {
    MonoTextStyleBuilder::new().font(f).text_color(WHITE).build()
}

fn colored(f: &'static MonoFont<'static>, color: Rgb565) -> MonoTextStyle<'static, Rgb565> {
    MonoTextStyleBuilder::new().font(f).text_color(color).build()
}

fn render_header(fb: &mut FrameBuffer, no: usize) {
    fill_rect(fb, 0, 0, WIDTH as u32, 30, HEADER_RED);
    Line::new(Point::new(0, 29), Point::new(WIDTH as i32 - 1, 29))
        .into_styled(PrimitiveStyle::with_stroke(HEADER_RED_DARK, 1))
        .draw(fb)
        .ok();
    Circle::new(Point::new(18, 15), 9)
        .into_styled(PrimitiveStyle::with_fill(WHITE))
        .draw(fb)
        .ok();
    Circle::new(Point::new(18, 15), 7)
        .into_styled(PrimitiveStyle::with_fill(LENS))
        .draw(fb)
        .ok();
    Circle::new(Point::new(15, 12), 2)
        .into_styled(PrimitiveStyle::with_fill(WHITE))
        .draw(fb)
        .ok();

    text_top(fb, "POKeDEX", 34, 5, white_on(&font::FONT_10X20));

    let mut num = heapless::String::<12>::new();
    if core::fmt::Write::write_fmt(&mut num, format_args!("{:03}/{}", no, DEX_LEN)).is_ok() {
        let x = WIDTH as i32 - num.len() as i32 * 8 - 8;
        text_top(fb, &num, x, 9, white_on(&font::FONT_8X13_BOLD));
    }
}

fn render_hint_bar(fb: &mut FrameBuffer) {
    fill_rect(fb, 0, HEIGHT as i32 - 16, WIDTH as u32, 16, STRIP);
    text_centered(
        fb,
        "UP/DN: SWITCH   OK: INFO",
        HEIGHT as i32 - 14,
        6,
        colored(&font::FONT_6X12, DIM),
    );
}

fn pill_row(fb: &mut FrameBuffer, e: &DexEntry, cx: i32, y: i32, h: u32, pad: i32, gap: i32) {
    let mut ids = [e.types[0], e.types[1]];
    if ids[0] > ids[1] {
        ids.swap(0, 1);
    }
    let count = if e.types[1] == TYPE_NONE { 1 } else { 2 };
    let widths: [i32; 2] = [
        6 * type_name(ids[0]).len() as i32 + 2 * pad,
        6 * type_name(ids[1]).len() as i32 + 2 * pad,
    ];
    let total: i32 = widths[..count].iter().sum::<i32>() + gap * (count as i32 - 1);
    let mut x = cx - total / 2;
    for i in 0..count {
        let (r, g, b) = type_rgb(ids[i]);
        fill_rect(fb, x, y, widths[i] as u32, h, Rgb565::new(r, g, b));
        text_top(
            fb,
            type_name(ids[i]),
            x + pad,
            y + ((h as i32 - 12) / 2 + 1),
            white_on(&font::FONT_6X12),
        );
        x += widths[i] + gap;
    }
}

fn render_card(fb: &mut FrameBuffer, e: &DexEntry, no: usize) {
    render_header(fb, no);

    card_at(fb, 44, 36, 152, 152);
    draw_sprite(fb, &Sprite::new(no as u16), 56, 48, 2);

    text_centered(fb, e.name, 198, 10, white_on(&font::FONT_10X20));

    let mut cat = heapless::String::<24>::new();
    if core::fmt::Write::write_fmt(&mut cat, format_args!("{} POKEMON", e.category)).is_ok() {
        text_centered(fb, &cat, 222, 6, colored(&font::FONT_6X12, SUB));
    }

    pill_row(fb, e, WIDTH as i32 / 2, 240, 22, 8, 8);

    let (hm, hd) = (e.height_dm as u32 / 10, e.height_dm as u32 % 10);
    let (wm, wd) = (e.weight_hg as u32 / 10, e.weight_hg as u32 % 10);
    let mut ht = heapless::String::<16>::new();
    let mut wt = heapless::String::<16>::new();
    let _ = core::fmt::Write::write_fmt(&mut ht, format_args!("{}.{}m", hm, hd));
    let _ = core::fmt::Write::write_fmt(&mut wt, format_args!("{}.{}kg", wm, wd));

    for (x, label, value) in [(28i32, "HT", &ht), (124i32, "WT", &wt)] {
        fill_rect(fb, x, 272, 88, 26, PANEL);
        let total = 2 * 6 + 6 + value.len() as i32 * 8;
        let vx = x + (88 - total) / 2;
        text_top(fb, label, vx, 279, colored(&font::FONT_6X12, SUB));
        text_top(fb, value, vx + 18, 277, white_on(&font::FONT_8X13_BOLD));
    }

    render_hint_bar(fb);
}

fn render_detail(fb: &mut FrameBuffer, e: &DexEntry, no: usize) {
    render_header(fb, no);

    card_at(fb, 12, 38, 68, 68);
    draw_sprite(fb, &Sprite::new(no as u16), 14, 40, 1);

    text_top(fb, e.name, 92, 42, white_on(&font::FONT_8X13_BOLD));
    text_top(fb, e.category, 92, 58, colored(&font::FONT_6X12, SUB));

    let mut y = 74;
    for &id in e.types.iter() {
        if id == TYPE_NONE {
            continue;
        }
        let (r, g, b) = type_rgb(id);
        let w = 6 * type_name(id).len() as i32 + 12;
        fill_rect(fb, 92, y, w as u32, 16, Rgb565::new(r, g, b));
        text_top(fb, type_name(id), 98, y + 2, white_on(&font::FONT_6X12));
        y += 20;
    }

    Line::new(Point::new(12, 118), Point::new(228, 118))
        .into_styled(PrimitiveStyle::with_stroke(DIVIDER, 1))
        .draw(fb)
        .ok();

    for (i, (&label, &v)) in STAT_LABELS.iter().zip(e.base.iter()).enumerate() {
        let y = 126 + i as i32 * 18;
        text_top(fb, label, 12, y + 1, colored(&font::FONT_6X12, SUB));
        let mut val = heapless::String::<4>::new();
        let _ = core::fmt::Write::write_fmt(&mut val, format_args!("{}", v));
        text_top(fb, &val, 56 - val.len() as i32 * 8, y - 1, white_on(&font::FONT_8X13_BOLD));
        fill_rect(fb, 64, y, 164, 8, BAR_BG);
        fill_rect(fb, 64, y, stat_bar_len(v, 164) as u32, 8, STAT_COLORS[i]);
    }

    Line::new(Point::new(12, 240), Point::new(228, 240))
        .into_styled(PrimitiveStyle::with_stroke(DIVIDER, 1))
        .draw(fb)
        .ok();

    let lines = wrap_text(e.desc, 36);
    for (i, line) in lines.iter().enumerate() {
        if i >= 5 {
            break;
        }
        text_top(fb, line, 12, 248 + i as i32 * 12, colored(&font::FONT_6X12, FLAVOR));
    }

    render_hint_bar(fb);
}

/// Full-screen render of the current model state.
pub fn render(fb: &mut FrameBuffer, model: &DexModel) {
    fb.clear(BG);
    let e = &DEX[model.no() - 1];
    match model.page() {
        Page::Card => render_card(fb, e, model.no()),
        Page::Detail => render_detail(fb, e, model.no()),
    }
}
