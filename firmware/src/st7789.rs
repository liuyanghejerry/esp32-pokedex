//! ST7789P3 240x320 TFT driver over SPI (RGB565).
//!
//! Register sequence mirrors the ai-passport BSP (`components/bsp/src/
//! bsp_display.c`): vendor porch/power/gamma tuning, 16-bit color mode,
//! factory-required color inversion, portrait MADCTL. The panel has no
//! RST pin (hard-wired to 3V3), so reset is software-only (SWRESET).
//!
//! All transfers raise CS only after `SpiBus::flush()` — esp-hal returns
//! from `write()` while the last FIFO chunk is still clocking out.

use core::convert::Infallible;

use embedded_graphics::{
    draw_target::DrawTarget,
    geometry::{OriginDimensions, Size},
    pixelcolor::raw::{RawData, RawU16},
    pixelcolor::Rgb565,
    prelude::Pixel,
};
use embedded_hal::digital::OutputPin;
use embedded_hal::spi::SpiBus;

use esp_hal::delay::Delay;

pub const WIDTH: usize = 240;
pub const HEIGHT: usize = 320;
/// RGB565, big-endian, row-major — the exact wire format for RAMWR.
pub const FB_LEN: usize = WIDTH * HEIGHT * 2;

/// Full-screen RGB565 framebuffer usable as an embedded-graphics DrawTarget.
pub struct FrameBuffer {
    pub buf: [u8; FB_LEN],
}

impl FrameBuffer {
    pub const fn new() -> Self {
        Self { buf: [0; FB_LEN] }
    }

    #[allow(dead_code)] // kept for callers that want an explicit fill
    pub fn clear(&mut self, c: Rgb565) {
        let raw = RawU16::from(c).into_inner();
        for pair in self.buf.chunks_exact_mut(2) {
            pair[0] = (raw >> 8) as u8;
            pair[1] = (raw & 0xFF) as u8;
        }
    }

    /// Fill an axis-aligned rectangle.
    ///
    /// This is the hot path of every screen (a full-screen fill is 76 800
    /// pixels), so it writes whole rows directly instead of going through the
    /// draw-target iterator one pixel at a time — several times faster, and it
    /// clips negative or oversized rectangles.
    pub fn fill_rect(&mut self, x: i32, y: i32, w: u32, h: u32, c: Rgb565) {
        if w == 0 || h == 0 {
            return;
        }
        let x0 = x.max(0) as usize;
        let y0 = y.max(0) as usize;
        let x1 = ((x.max(0) as u32).saturating_add(w) as usize).min(WIDTH);
        let y1 = ((y.max(0) as u32).saturating_add(h) as usize).min(HEIGHT);
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        let raw = RawU16::from(c).into_inner();
        let (hi, lo) = ((raw >> 8) as u8, raw as u8);
        for row in y0..y1 {
            let start = (row * WIDTH + x0) * 2;
            let end = (row * WIDTH + x1) * 2;
            for px in self.buf[start..end].chunks_exact_mut(2) {
                px[0] = hi;
                px[1] = lo;
            }
        }
    }

    #[inline]
    pub fn set(&mut self, x: u32, y: u32, c: Rgb565) {
        if x >= WIDTH as u32 || y >= HEIGHT as u32 {
            return;
        }
        let i = ((y as usize * WIDTH + x as usize) * 2) as usize;
        let raw = RawU16::from(c).into_inner();
        self.buf[i] = (raw >> 8) as u8;
        self.buf[i + 1] = (raw & 0xFF) as u8;
    }
}

impl OriginDimensions for FrameBuffer {
    fn size(&self) -> Size {
        Size::new(WIDTH as u32, HEIGHT as u32)
    }
}

impl DrawTarget for FrameBuffer {
    type Color = Rgb565;
    type Error = Infallible;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        for Pixel(coord, color) in pixels {
            self.set(coord.x as u32, coord.y as u32, color);
        }
        Ok(())
    }
}

/// ST7789 over any SPI bus + CS/DC control pins.
pub struct St7789<SPI, CS, DC> {
    spi: SPI,
    cs: CS,
    dc: DC,
}

impl<SPI, CS, DC> St7789<SPI, CS, DC>
where
    SPI: SpiBus<u8>,
    CS: OutputPin,
    DC: OutputPin,
{
    pub fn new(spi: SPI, cs: CS, dc: DC) -> Self {
        Self { spi, cs, dc }
    }

    fn cmd(&mut self, reg: u8) {
        let _ = self.dc.set_low();
        let _ = self.cs.set_low();
        let _ = self.spi.write(&[reg]);
        let _ = self.spi.flush();
        let _ = self.cs.set_high();
    }

    fn data(&mut self, bytes: &[u8]) {
        let _ = self.dc.set_high();
        let _ = self.cs.set_low();
        let _ = self.spi.write(bytes);
        let _ = self.spi.flush();
        let _ = self.cs.set_high();
    }

    fn cmd_data(&mut self, reg: u8, bytes: &[u8]) {
        self.cmd(reg);
        self.data(bytes);
    }

    /// Bring the panel up from power-on using the BSP's vendor sequence.
    pub fn init(&mut self, delay: &mut Delay) {
        self.cmd(0x01); // SWRESET (no RST pin wired)
        delay.delay_millis(150);
        self.cmd(0x11); // SLPOUT
        delay.delay_millis(120);
        self.cmd_data(0x3A, &[0x55]); // COLMOD: 16-bit RGB565
        self.cmd_data(0xB0, &[0x00, 0xE0]); // RAMCTRL (esp_lcd st7789 default)
        self.cmd_data(0xB2, &[0x05, 0x05, 0x00, 0x33, 0x33]); // PORCTRL
        self.cmd_data(0xB7, &[0x35]); // GCTRL
        self.cmd_data(0xBB, &[0x21]); // VCOMS
        self.cmd_data(0xC0, &[0x2C]); // LCMCTRL
        self.cmd_data(0xC2, &[0x01]); // VDVVRHEN
        self.cmd_data(0xC3, &[0x0B]); // VRHS
        self.cmd_data(0xC4, &[0x20]); // VDVSET
        self.cmd_data(0xC6, &[0x0F]); // FRCTRL2: 60Hz
        self.cmd_data(0xD0, &[0xA4, 0xA1]); // PWCTRL1
        self.cmd_data(0xD6, &[0xA1]);
        self.cmd_data(
            0xE0,
            &[0xD0, 0x04, 0x08, 0x0A, 0x09, 0x05, 0x2D, 0x43, 0x49, 0x09, 0x16, 0x15, 0x26, 0x2B],
        ); // PVGAMCTRL
        self.cmd_data(
            0xE1,
            &[0xD0, 0x03, 0x09, 0x0A, 0x0A, 0x06, 0x2E, 0x44, 0x40, 0x3A, 0x15, 0x15, 0x26, 0x2A],
        ); // NVGAMCTRL
        self.cmd(0x21); // INVON: this panel ships inverted
        self.cmd_data(0x36, &[0x00]); // MADCTL: portrait, no mirror
        self.cmd(0x29); // DISPON
        delay.delay_millis(20);
    }

    /// Push one full frame to GRAM (full-window RAMWR burst).
    ///
    /// `keep_alive` runs between SPI chunks: a frame is ~31 ms of wire time at
    /// 40 MHz, which is long enough to starve anything the caller services
    /// from its main loop — in this firmware, the DMA-fed audio ring.
    pub fn push_frame(&mut self, fb: &FrameBuffer, keep_alive: &mut dyn FnMut()) {
        self.cmd_data(0x2A, &[0x00, 0x00, 0x00, (WIDTH - 1) as u8]); // CASET
        self.cmd_data(0x2B, &[0x00, 0x00, ((HEIGHT - 1) >> 8) as u8, ((HEIGHT - 1) & 0xFF) as u8]); // RASET
        self.cmd(0x2C); // RAMWR
        let _ = self.dc.set_high();
        let _ = self.cs.set_low();
        for chunk in fb.buf.chunks(4096) {
            let _ = self.spi.write(chunk);
            keep_alive();
        }
        let _ = self.spi.flush();
        let _ = self.cs.set_high();
    }

    /// Push just the `w`x`h` window at (`x`,`y`) — everything that is animating
    /// costs its own area instead of a full 153 KB frame. The controller wraps
    /// from the window's last column to the next row on its own, so the rows
    /// are simply fed one after another.
    pub fn push_rect(
        &mut self,
        fb: &FrameBuffer,
        x: usize,
        y: usize,
        w: usize,
        h: usize,
        keep_alive: &mut dyn FnMut(),
    ) {
        if w == 0 || h == 0 || x + w > WIDTH || y + h > HEIGHT {
            return;
        }
        let (x0, x1) = (x as u16, (x + w - 1) as u16);
        let (y0, y1) = (y as u16, (y + h - 1) as u16);
        self.cmd_data(0x2A, &[(x0 >> 8) as u8, x0 as u8, (x1 >> 8) as u8, x1 as u8]); // CASET
        self.cmd_data(0x2B, &[(y0 >> 8) as u8, y0 as u8, (y1 >> 8) as u8, y1 as u8]); // RASET
        self.cmd(0x2C); // RAMWR
        let _ = self.dc.set_high();
        let _ = self.cs.set_low();
        for row in y..y + h {
            let start = (row * WIDTH + x) * 2;
            for chunk in fb.buf[start..start + w * 2].chunks(2048) {
                let _ = self.spi.write(chunk);
            }
            keep_alive();
        }
        let _ = self.spi.flush();
        let _ = self.cs.set_high();
    }
}
