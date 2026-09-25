//! Pokedex UI — 中文卡片式设计（官方简体中文译名，点阵字库渲染）。
//!
//! 两页：卡片页（名字/分类/属性/身高体重）与详情页（种族值 + 图鉴说明）。
//! 上/下切换宝可梦（保持当前页），OK 切换页面。

use embedded_graphics::{
    pixelcolor::Rgb565,
    prelude::*,
    primitives::{Circle, Line, PrimitiveStyle, PrimitiveStyleBuilder, Rectangle},
};

use pokedex_core::{
    stat_bar_len, type_dark_text, type_name_zh, type_rgb, wrap_zh, DexModel, Page, TYPE_NONE,
};

use crate::dex_data::{DexEntry, DEX, DEX_LEN};
use crate::font_zh;
use crate::sprites::Sprite;
use crate::st7789::{FrameBuffer, HEIGHT, WIDTH};

/// 8-bit RGB -> RGB565 (Rgb565::new only masks, it does not scale; feed it
/// pre-reduced 5/6/5 channel values).
const fn c8(r: u8, g: u8, b: u8) -> Rgb565 {
    Rgb565::new(r >> 3, g >> 2, b >> 3)
}

const BG: Rgb565 = c8(0x10, 0x16, 0x2B);
const HEADER_RED: Rgb565 = c8(0xE3, 0x35, 0x0D);
const HEADER_RED_DARK: Rgb565 = c8(0xB0, 0x28, 0x0A);
const STRIP: Rgb565 = c8(0x0B, 0x10, 0x20);
const CARD_BORDER: Rgb565 = c8(0x26, 0x30, 0x4F);
const WHITE: Rgb565 = c8(0xFF, 0xFF, 0xFF);
const SUB: Rgb565 = c8(0x9A, 0xA5, 0xC0);
const DIM: Rgb565 = c8(0x8A, 0x93, 0xAD);
const DIVIDER: Rgb565 = c8(0x2A, 0x33, 0x52);
const BAR_BG: Rgb565 = c8(0x23, 0x2C, 0x48);
const PANEL: Rgb565 = c8(0x1A, 0x23, 0x40);
const LENS: Rgb565 = c8(0x12, 0x31, 0x5C);
const FLAVOR: Rgb565 = c8(0xC7, 0xCE, 0xE0);
/// Materialize flash color for the entrance animation (dark shadow on the
/// white card) and dark text on light type badges.
const INK: Rgb565 = c8(0x1A, 0x23, 0x40);

const STAT_COLORS: [Rgb565; 6] = [
    c8(0x78, 0xC8, 0x50), // 体力 green
    c8(0xF0, 0x80, 0x30), // 攻击 orange
    c8(0xF8, 0xD0, 0x30), // 防御 yellow
    c8(0x68, 0x90, 0xF0), // 特攻 blue
    c8(0x98, 0xD8, 0xD8), // 特防 cyan
    c8(0xF8, 0x58, 0x88), // 速度 pink
];

const STAT_LABELS: [&str; 6] = ["HP", "攻击", "防御", "特攻", "特防", "速度"];

/// Flavor text: x=6, 220px wide => 44 halfwidth units (5px each) per line.
const FLAVOR_UNITS: usize = 44;
const FLAVOR_LINES: usize = 6;

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

fn draw_sprite(
    fb: &mut FrameBuffer,
    s: &Sprite,
    x0: i32,
    y0: i32,
    scale: u32,
    silhouette: Option<Rgb565>,
) {
    let scale = scale as i32;
    for y in 0..64i32 {
        for x in 0..64i32 {
            if s.opaque(x as u32, y as u32) {
                let color = silhouette.unwrap_or_else(|| s.color(x as u32, y as u32));
                for dy in 0..scale {
                    for dx in 0..scale {
                        let px = x0 + x * scale + dx;
                        let py = y0 + y * scale + dy;
                        if px >= 0 && py >= 0 {
                            fb.set(px as u32, py as u32, color);
                        }
                    }
                }
            }
        }
    }
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

    font_zh::draw_text(fb, "宝可梦图鉴", 34, 3, WHITE, 2);

    let mut num = heapless::String::<12>::new();
    if core::fmt::Write::write_fmt(&mut num, format_args!("{:03}/{}", no, DEX_LEN)).is_ok() {
        font_zh::draw_right(fb, &num, WIDTH as i32 - 8, 9, WHITE, 1);
    }
}

fn render_hint_bar(fb: &mut FrameBuffer) {
    fill_rect(fb, 0, HEIGHT as i32 - 16, WIDTH as u32, 16, STRIP);
    font_zh::draw_centered(fb, "上/下：切换  确：详情", HEIGHT as i32 - 14, DIM, 1);
}

fn pill_row(fb: &mut FrameBuffer, e: &DexEntry, cx: i32, y: i32, h: u32, pad: i32, gap: i32) {
    let mut ids = [e.types[0], e.types[1]];
    if ids[0] > ids[1] {
        ids.swap(0, 1);
    }
    let count = if e.types[1] == TYPE_NONE { 1 } else { 2 };
    let widths: [i32; 2] = [
        font_zh::width(type_name_zh(ids[0]), 1) + 2 * pad,
        font_zh::width(type_name_zh(ids[1]), 1) + 2 * pad,
    ];
    let total: i32 = widths[..count].iter().sum::<i32>() + gap * (count as i32 - 1);
    let mut x = cx - total / 2;
    for i in 0..count {
        let (r, g, b) = type_rgb(ids[i]);
        fill_rect(fb, x, y, widths[i] as u32, h, c8(r, g, b));
        let text_color = if type_dark_text(ids[i]) { INK } else { WHITE };
        font_zh::draw_vcentered(fb, type_name_zh(ids[i]), x + pad, y, h as i32, text_color, 1);
        x += widths[i] + gap;
    }
}

fn render_card(fb: &mut FrameBuffer, e: &DexEntry, no: usize, sprite_dy: i32, silhouette: bool) {
    fb.clear(BG);
    render_header(fb, no);

    card_at(fb, 44, 36, 152, 152);
    draw_sprite(
        fb,
        &Sprite::new(no as u16),
        56,
        48 + sprite_dy,
        2,
        silhouette.then_some(INK),
    );

    font_zh::draw_centered(fb, e.name, 192, WHITE, 2);
    font_zh::draw_centered(fb, e.category, 220, SUB, 1);

    pill_row(fb, e, WIDTH as i32 / 2, 240, 20, 6, 8);

    let (hm, hd) = (e.height_dm as u32 / 10, e.height_dm as u32 % 10);
    let (wm, wd) = (e.weight_hg as u32 / 10, e.weight_hg as u32 % 10);
    let mut ht = heapless::String::<16>::new();
    let mut wt = heapless::String::<16>::new();
    let _ = core::fmt::Write::write_fmt(&mut ht, format_args!("{}.{}m", hm, hd));
    let _ = core::fmt::Write::write_fmt(&mut wt, format_args!("{}.{}kg", wm, wd));

    for (x, label, value) in [(28i32, "身高", &ht), (124i32, "体重", &wt)] {
        fill_rect(fb, x, 268, 88, 24, PANEL);
        let total = font_zh::width(label, 1) + 6 + font_zh::width(value, 1);
        let vx = x + (88 - total) / 2;
        font_zh::draw_vcentered(fb, label, vx, 268, 24, SUB, 1);
        font_zh::draw_vcentered(fb, value, vx + font_zh::width(label, 1) + 6, 268, 24, WHITE, 1);
    }

    render_hint_bar(fb);
}

fn render_flavor(fb: &mut FrameBuffer, e: &DexEntry) {
    let lines = wrap_zh(e.desc, FLAVOR_UNITS);
    let shown = lines.len().min(FLAVOR_LINES);
    for i in 0..shown {
        let y = 220 + i as i32 * font_zh::LINE_H;
        if i == FLAVOR_LINES - 1 && lines.len() > FLAVOR_LINES {
            let mut buf = heapless::String::<96>::new();
            let mut units = 0usize;
            for ch in lines[i].chars() {
                let cu = if ch.is_ascii() { 1 } else { 2 };
                if units + cu + 2 > FLAVOR_UNITS {
                    break;
                }
                let _ = buf.push(ch);
                units += cu;
            }
            let _ = buf.push('…');
            font_zh::draw_text(fb, &buf, 6, y, FLAVOR, 1);
        } else {
            font_zh::draw_text(fb, lines[i], 6, y, FLAVOR, 1);
        }
    }
}

fn render_detail(fb: &mut FrameBuffer, e: &DexEntry, no: usize, sprite_dy: i32, silhouette: bool) {
    fb.clear(BG);
    render_header(fb, no);

    card_at(fb, 12, 38, 68, 68);
    draw_sprite(
        fb,
        &Sprite::new(no as u16),
        14,
        40 + sprite_dy,
        1,
        silhouette.then_some(INK),
    );

    font_zh::draw_text(fb, e.name, 92, 38, WHITE, 2);
    font_zh::draw_text(fb, e.category, 92, 64, SUB, 1);

    let mut y = 80;
    for &id in e.types.iter() {
        if id == TYPE_NONE {
            continue;
        }
        let (r, g, b) = type_rgb(id);
        let w = font_zh::width(type_name_zh(id), 1) + 10;
        fill_rect(fb, 92, y, w as u32, 14, c8(r, g, b));
        let text_color = if type_dark_text(id) { INK } else { WHITE };
        font_zh::draw_vcentered(fb, type_name_zh(id), 97, y, 14, text_color, 1);
        y += 18;
    }

    Line::new(Point::new(12, 118), Point::new(228, 118))
        .into_styled(PrimitiveStyle::with_stroke(DIVIDER, 1))
        .draw(fb)
        .ok();

    for (i, (&label, &v)) in STAT_LABELS.iter().zip(e.base.iter()).enumerate() {
        let y = 122 + i as i32 * 15;
        font_zh::draw_text(fb, label, 12, y, SUB, 1);
        let mut val = heapless::String::<4>::new();
        let _ = core::fmt::Write::write_fmt(&mut val, format_args!("{}", v));
        font_zh::draw_right(fb, &val, 56, y, WHITE, 1);
        fill_rect(fb, 64, y + 2, 164, 6, BAR_BG);
        fill_rect(fb, 64, y + 2, stat_bar_len(v, 164) as u32, 6, STAT_COLORS[i]);
    }

    Line::new(Point::new(12, 214), Point::new(228, 214))
        .into_styled(PrimitiveStyle::with_stroke(DIVIDER, 1))
        .draw(fb)
        .ok();

    render_flavor(fb, e);

    render_hint_bar(fb);
}

/// Full-screen render of the current model state.
pub fn render(fb: &mut FrameBuffer, model: &DexModel) {
    render_anim(fb, &DEX[model.no() - 1], model.no(), 0, false, model.page());
}

/// Full-screen render with the front sprite shifted vertically by
/// `sprite_dy` and optionally drawn as a flat silhouette (entrance
/// animation frames; dy = 0, silhouette = false is the resting state).
pub fn render_anim(
    fb: &mut FrameBuffer,
    e: &DexEntry,
    no: usize,
    sprite_dy: i32,
    silhouette: bool,
    page: Page,
) {
    match page {
        Page::Card => render_card(fb, e, no, sprite_dy, silhouette),
        Page::Detail => render_detail(fb, e, no, sprite_dy, silhouette),
    }
}
