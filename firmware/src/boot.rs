//! 开机动画：精灵球落入 → 白光炸开 → 标题 → 扫描线切入图鉴主界面。
//!
//! 全部按帧号驱动（每帧 = 一次整屏推送 + 固定延时），不依赖计时器：
//! 40 MHz SPI 推一帧 240x320 约 31 ms，加上渲染稳定在 ~27 fps。
//! 每一帧都会 pump 音频 DMA，所以动画期间音乐不会断。
//! 开机动画期间按任意键可跳过。

use embedded_hal::{digital::OutputPin, spi::SpiBus};

use embedded_graphics::{
    pixelcolor::raw::RawU16,
    pixelcolor::Rgb565,
    prelude::*,
    primitives::{Circle, PrimitiveStyle},
};

use pokedex_core::DexModel;

use crate::audio::Audio;
use crate::audio_data::sfx;
use crate::font_zh;
use crate::st7789::{FrameBuffer, St7789, HEIGHT, WIDTH};
use crate::ui;

use ui::c8;

const BG: Rgb565 = c8(0x10, 0x16, 0x2B);
const BG_LINE: Rgb565 = c8(0x17, 0x1E, 0x36);
const BALL_RED: Rgb565 = c8(0xE3, 0x35, 0x0D);
const BALL_RED_LIT: Rgb565 = c8(0xFF, 0x6B, 0x4A);
const BALL_RED_DARK: Rgb565 = c8(0xB0, 0x28, 0x0A);
const BALL_WHITE: Rgb565 = c8(0xF4, 0xF6, 0xFA);
const BALL_INK: Rgb565 = c8(0x1A, 0x23, 0x40);
const DIM: Rgb565 = c8(0x60, 0x6A, 0x88);

/// Ball radius in pixels.
const BALL_R: i32 = 38;
/// Ball centre y when resting on the "floor".
const FLOOR_Y: i32 = 196;
/// Sub-pixel scale for the fall simulation (16 units = 1 px).
const U: i32 = 16;
/// Gravity per frame, in `U` units: 24/16 = 1.5 px per frame squared.
const GRAVITY: i32 = 24;
/// Velocity kept through a bounce (percent).
const RESTITUTION: i32 = 30;
/// Below this speed (units/frame) the ball is considered settled.
const SETTLE_V: i32 = 60;

/// Frame budget for each phase. The whole animation is ~3.1 s.
const F_BLACK: u32 = 2;
const F_FALL: u32 = 34;
/// Frames the ball wobbles before it opens.
const F_SHAKE: usize = 8;
const F_FLASH: u32 = 11;
const F_TITLE: u32 = 22;
const F_WIPE: u32 = 8;
const FRAME_MS: u32 = 6;

fn isqrt(n: i32) -> i32 {
    if n <= 0 {
        return 0;
    }
    let mut x = n;
    let mut y = (x + 1) / 2;
    while y < x {
        x = y;
        y = (x + n / x) / 2;
    }
    x
}

/// Classic two-tone ball, drawn pixel by pixel (`fb.set` clips off-screen
/// coordinates, so a partially visible ball needs no special case).
fn draw_ball(fb: &mut FrameBuffer, cx: i32, cy: i32) {
    let r = BALL_R;
    let band = (r / 7).max(3);
    let btn = r / 3;
    for dy in -r..=r {
        let span = isqrt(r * r - dy * dy);
        for dx in -span..=span {
            let px = dx * dx + dy * dy;
            let color = if span - dx.abs() <= 1 {
                BALL_INK // outline
            } else if px <= btn * btn {
                if px >= (btn - 2) * (btn - 2) {
                    BALL_INK
                } else {
                    BALL_WHITE
                }
            } else if dy.abs() <= band {
                BALL_INK // the hinge band
            } else if dy < 0 {
                // upper half: a lit facet gives the flat ball some volume
                let hx = dx + r / 3;
                let hy = dy + r / 3;
                if hx * hx + hy * hy < (r / 4) * (r / 4) {
                    BALL_RED_LIT
                } else if dy < -band * 3 {
                    BALL_RED_DARK
                } else {
                    BALL_RED
                }
            } else {
                BALL_WHITE
            };
            fb.set((cx + dx) as u32, (cy + dy) as u32, color);
        }
    }
}

/// Dark scanline backdrop for the ball phase.
fn draw_backdrop(fb: &mut FrameBuffer) {
    ui::fill_rect(fb, 0, 0, WIDTH as u32, HEIGHT as u32, BG);
    let mut y = 2;
    while y < HEIGHT as i32 {
        ui::fill_rect(fb, 0, y, WIDTH as u32, 1, BG_LINE);
        y += 4;
    }
}

/// Expanding white burst with a red rim — the "sent out" flash.
///
/// Filled by scanline rather than with a `Circle` primitive: the burst grows
/// past the screen, and the primitive would iterate its whole bounding box
/// (over a million clipped pixel writes per frame on the last frames).
fn draw_burst(fb: &mut FrameBuffer, cx: i32, cy: i32, r: i32) {
    fill_disc(fb, cx, cy, r + 4, BALL_RED);
    fill_disc(fb, cx, cy, r, BALL_WHITE);
}

fn fill_disc(fb: &mut FrameBuffer, cx: i32, cy: i32, r: i32, color: Rgb565) {
    for dy in -r..=r {
        let y = cy + dy;
        if y < 0 || y >= HEIGHT as i32 {
            continue;
        }
        let span = isqrt(r * r - dy * dy);
        let x0 = (cx - span).max(0);
        let x1 = (cx + span).min(WIDTH as i32 - 1);
        if x1 >= x0 {
            ui::fill_rect(fb, x0, y, (x1 - x0 + 1) as u32, 1, color);
        }
    }
}

/// The title card: red band, wordmark, dex count and a loading bar that
/// fills over the phase (`t` = 0..=255 through the phase).
fn draw_title(fb: &mut FrameBuffer, t: u32) {
    ui::fill_rect(fb, 0, 0, WIDTH as u32, HEIGHT as u32, BALL_WHITE);

    // Header band with the lens, same visual language as the dex header.
    let band_h = (64 * t / 255).max(1);
    ui::fill_rect(fb, 0, 0, WIDTH as u32, band_h, BALL_RED);
    if band_h >= 64 {
        ui::fill_rect(fb, 0, 62, WIDTH as u32, 2, BALL_RED_DARK);
        Circle::new(Point::new(18, 14), 36)
            .into_styled(PrimitiveStyle::with_fill(BALL_WHITE))
            .draw(fb)
            .ok();
        Circle::new(Point::new(22, 18), 28)
            .into_styled(PrimitiveStyle::with_fill(c8(0x12, 0x31, 0x5C)))
            .draw(fb)
            .ok();
        Circle::new(Point::new(16, 12), 8)
            .into_styled(PrimitiveStyle::with_fill(BALL_WHITE))
            .draw(fb)
            .ok();
        font_zh::draw_text(fb, "宝可梦图鉴", 68, 22, BALL_WHITE, 2);
    }

    // Wordmark and count fade in from the background colour.
    if t > 40 {
        let k = ((t - 40).min(120)) as i32;
        let ink = lerp(BALL_WHITE, BALL_INK, k);
        font_zh::draw_centered(fb, "全国图鉴", 136, ink, 3);
        font_zh::draw_centered(fb, "386 只", 186, lerp(BALL_WHITE, BALL_RED, k), 3);
    }

    // Loading bar.
    let bar_w = (t * 160 / 255) as u32;
    ui::fill_rect(fb, 40, 258, 160, 8, c8(0xE2, 0xE6, 0xEF));
    if bar_w > 0 {
        ui::fill_rect(fb, 40, 258, bar_w, 8, BALL_RED);
    }
    if t > 200 {
        font_zh::draw_centered(fb, "宝可梦图鉴", 286, DIM, 1);
    }
}

/// Blend two RGB565 colours: `t` = 0 gives `a`, 255 gives `b`.
fn lerp(a: Rgb565, b: Rgb565, t: i32) -> Rgb565 {
    let t = t.clamp(0, 255);
    let mix = |x: i32, y: i32| -> u8 { ((x * (255 - t) + y * t) / 255) as u8 };
    let (ar, ag, ab) = channels(a);
    let (br, bg, bb) = channels(b);
    Rgb565::new(mix(ar, br), mix(ag, bg), mix(ab, bb))
}

fn channels(c: Rgb565) -> (i32, i32, i32) {
    let raw = RawU16::from(c).into_inner();
    (
        ((raw >> 11) & 0x1F) as i32,
        ((raw >> 5) & 0x3F) as i32,
        (raw & 0x1F) as i32,
    )
}

/// Play the whole boot sequence, ending on the first dex card with `bgm_track`
/// (already picked at random by the caller) playing underneath.
pub fn run<SPI, CS, DC>(
    lcd: &mut St7789<SPI, CS, DC>,
    fb: &mut FrameBuffer,
    delay: &mut esp_hal::delay::Delay,
    model: &DexModel,
    audio: &mut dyn Audio,
    bgm_track: usize,
    skip: &mut dyn FnMut() -> bool,
) where
    SPI: SpiBus<u8>,
    CS: OutputPin,
    DC: OutputPin,
{
    audio.jingle(sfx::STARTUP);

    // --- black ---
    let mut done = 0u32;
    for _ in 0..F_BLACK {
        ui::fill_rect(fb, 0, 0, WIDTH as u32, HEIGHT as u32, c8(0, 0, 0));
        lcd.push_frame(fb, &mut || audio.pump());
        delay.delay_millis(FRAME_MS);
        done += 1;
    }

    // --- the ball falls, bounces twice and settles ---
    let mut cy = -(BALL_R + 8) * U;
    let mut vy = 0i32;
    let mut bounces: usize = 0;
    for _ in 0..F_FALL {
        vy += GRAVITY;
        cy += vy;
        if cy >= FLOOR_Y * U {
            cy = FLOOR_Y * U;
            vy = -(vy * RESTITUTION) / 100;
            if vy.abs() > SETTLE_V {
                audio.sfx(
                    [sfx::BALL_BOUNCE_1, sfx::BALL_BOUNCE_2, sfx::BALL_BOUNCE_3][bounces.min(2)],
                );
                bounces += 1;
            }
        }
        draw_backdrop(fb);
        draw_ball(fb, WIDTH as i32 / 2, cy / U);
        lcd.push_frame(fb, &mut || audio.pump());
        delay.delay_millis(FRAME_MS);
        done += 1;
        if skip() {
            break;
        }
    }

    // --- shake, flash and title: any key press during the fall skips the lot ---
    const SHAKE: [i32; F_SHAKE] = [-7, 5, -4, 3, -2, 1, 0, 0];
    if !skip() {
        for (i, dx) in SHAKE.iter().enumerate() {
            if i == 0 {
                audio.sfx(sfx::BALL_CLICK);
            }
            draw_backdrop(fb);
            draw_ball(fb, WIDTH as i32 / 2 + dx, FLOOR_Y);
            lcd.push_frame(fb, &mut || audio.pump());
            delay.delay_millis(FRAME_MS * 2);
        }
        audio.sfx(sfx::BALL_OPEN);
        for i in 0..F_FLASH {
            let r = (8 + i * i * 5) as i32;
            draw_backdrop(fb);
            draw_ball(fb, WIDTH as i32 / 2, FLOOR_Y);
            draw_burst(fb, WIDTH as i32 / 2, FLOOR_Y, r);
            lcd.push_frame(fb, &mut || audio.pump());
            delay.delay_millis(FRAME_MS);
        }

        // --- title ---
        for i in 0..F_TITLE {
            let t = (i + 1) * 255 / F_TITLE;
            draw_title(fb, t);
            lcd.push_frame(fb, &mut || audio.pump());
            delay.delay_millis(FRAME_MS * 2);
        }
    }

    // --- wipe into the dex: the card slides in over a red field ---
    audio.sfx(sfx::DEX_PAGE);
    ui::fill_rect(fb, 0, 0, WIDTH as u32, HEIGHT as u32, BALL_RED);
    lcd.push_frame(fb, &mut || audio.pump());
    ui::render(fb, model);
    let step = HEIGHT / F_WIPE as usize;
    for i in 1..=F_WIPE {
        let h = (i as usize * step).min(HEIGHT);
        lcd.push_rect(fb, 0, 0, WIDTH, h, &mut || audio.pump());
        delay.delay_millis(FRAME_MS * 2);
    }
    audio.bgm(bgm_track);
    esp_println::println!("boot: animation done after {} frames", done);
}
