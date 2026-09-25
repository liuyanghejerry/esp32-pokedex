//! Runtime for the Fusion Pixel 10px Monospaced subset (tools/gen_font.py).
//!
//! Blob layout per glyph: u8 w, u8 h, i8 x_off, i8 y_off, u8 advance,
//! then ceil(w/8)*h row bytes (MSB-first). ASCII advances 5px, CJK 10px.
//! Glyphs are positioned against a baseline 10px below the line top
//! (Fusion Pixel metrics: ascent 9, glyphs may dip 2px into the descent),
//! giving a 12px visual line. `scale` = 2 doubles everything for titles
//! and names.

use embedded_graphics::pixelcolor::Rgb565;

use crate::font_zh_data::{B10, CP10, OFF10};
use crate::st7789::FrameBuffer;

/// Visual line pitch at scale 1.
pub const LINE_H: i32 = 12;
/// Nominal ink height of one line at scale 1 (CJK glyphs are 12px tall;
/// latin glyphs sit at y+1..y+11 inside the same box).
pub const INK_H: i32 = 12;
/// Baseline position relative to the line top, at scale 1.
const BASELINE: i32 = 10;

struct Glyph {
    w: i32,
    h: i32,
    x_off: i32,
    y_off: i32,
    advance: i32,
    bits: &'static [u8],
}

fn glyph(c: char) -> Option<Glyph> {
    let code = c as u32;
    if code > 0xFFFF {
        return None;
    }
    let idx = CP10.binary_search(&(code as u16)).ok()?;
    let (start, end) = (OFF10[idx] as usize, OFF10[idx + 1] as usize);
    let g = &B10[start..end];
    Some(Glyph {
        w: g[0] as i32,
        h: g[1] as i32,
        x_off: g[2] as i8 as i32,
        y_off: g[3] as i8 as i32,
        advance: g[4] as i32,
        bits: &g[5..],
    })
}

/// Advance in px at scale 1; missing glyphs fall back to half/full width.
pub fn advance(c: char) -> i32 {
    if c.is_ascii() {
        5
    } else {
        10
    }
}

pub fn width(s: &str, scale: i32) -> i32 {
    s.chars().map(|c| glyph(c).map(|g| g.advance).unwrap_or_else(|| advance(c))).sum::<i32>() * scale
}

/// Draw `s` with its line top at `y`.
pub fn draw_text(fb: &mut FrameBuffer, s: &str, x: i32, y: i32, color: Rgb565, scale: i32) {
    let baseline = y + BASELINE * scale;
    let mut cx = x;
    for c in s.chars() {
        if let Some(g) = glyph(c) {
            let row_bytes = (g.w + 7) / 8;
            let top = baseline - (g.h + g.y_off) * scale;
            for gy in 0..g.h {
                for gx in 0..g.w {
                    let byte = g.bits[(gy * row_bytes + gx / 8) as usize];
                    if byte & (0x80 >> (gx % 8)) != 0 {
                        let px = cx + (g.x_off + gx) * scale;
                        let py = top + gy * scale;
                        for sy in 0..scale {
                            for sx in 0..scale {
                                fb.set((px + sx) as u32, (py + sy) as u32, color);
                            }
                        }
                    }
                }
            }
            cx += g.advance * scale;
        } else {
            cx += advance(c) * scale;
        }
    }
}

pub fn draw_centered(fb: &mut FrameBuffer, s: &str, y: i32, color: Rgb565, scale: i32) {
    let x = (crate::st7789::WIDTH as i32 - width(s, scale)) / 2;
    draw_text(fb, s, x, y, color, scale);
}

/// Draw text vertically centered inside a box (top `box_y`, height `box_h`)
/// by placing the 12px-scale ink box symmetrically.
pub fn draw_vcentered(
    fb: &mut FrameBuffer,
    s: &str,
    x: i32,
    box_y: i32,
    box_h: i32,
    color: Rgb565,
    scale: i32,
) {
    let y = box_y + (box_h - INK_H * scale).max(0) / 2;
    draw_text(fb, s, x, y, color, scale);
}

pub fn draw_right(fb: &mut FrameBuffer, s: &str, right_x: i32, y: i32, color: Rgb565, scale: i32) {
    draw_text(fb, s, right_x - width(s, scale), y, color, scale);
}
