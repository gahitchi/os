// SPDX-License-Identifier: GPL-3.0-only
//! Drawing into a picture of 32-bit pixels (0x00RRGGBB, row after row):
//! filled and rounded rectangles, soft shadows, frames, gradients, and
//! glyph coverage blended in a colour. Everything is clipped to the
//! picture and to a clip rectangle, so a wrong size never writes outside.
//!
//! A canvas can have a scale (milestone 7b): then every place and size
//! given to it is in "points", and it draws `scale` percent pixels for
//! each (200: two by two). Apps draw in points and look the same size
//! on a dense screen; the canvas turns points into pixels in one place.

use crate::Rect;
use crate::font::Bitmap;

pub type Color = u32;

/// Mix `a` towards `b` by `t` of 255.
pub fn mix(a: Color, b: Color, t: u32) -> Color {
    let t = t.min(255);
    let ch = |s: u32| {
        let (x, y) = ((a >> s) & 0xff, (b >> s) & 0xff);
        ((x * (255 - t) + y * t + 127) / 255) << s
    };
    ch(16) | ch(8) | ch(0)
}

/// The high-contrast colour for `c` (milestone 7b): black ground, white
/// text, yellow for what is chosen or has the keyboard, and bright
/// colours for states. Alaya's own colours have their own; any other
/// colour becomes black or white by how light it is, or a bright colour
/// when it is a strong one. Only colours change, never what anything does.
pub fn high_contrast(c: Color) -> Color {
    use crate::widgets::theme as t;
    const BLACK: Color = 0x00_00_00;
    const WHITE: Color = 0xff_ff_ff;
    const YELLOW: Color = 0xff_ff_00;
    match c & 0xff_ff_ff {
        t::INK | t::MUTED | t::LINE => WHITE,
        t::ACCENT => YELLOW,
        t::ACCENT_INK => BLACK,
        t::ACCENT_SOFT => 0x33_33_00,
        t::OK => 0x80_ff_80,
        t::WARN => 0xff_d0_60,
        t::PAST => 0xa0_d0_ff,
        t::UNKNOWN => 0xd8_d8_d8,
        t::NOW => 0xff_a0_a0,
        t::PAGE
        | t::WALL_TOP
        | t::WALL_BOTTOM
        | t::BAR
        | t::WIN
        | t::CHROME
        | t::OK_BG
        | t::WARN_BG
        | t::PAST_BG
        | t::UNKNOWN_BG => BLACK,
        c => {
            let (r, g, b) = ((c >> 16) & 0xff, (c >> 8) & 0xff, c & 0xff);
            let light = (r * 2126 + g * 7152 + b * 722) / 10_000;
            let strong = r.max(g).max(b) - r.min(g).min(b) > 96;
            // Only near-white is taken for a ground: middle greys are
            // mostly lines and marks drawn on one.
            let light = if light >= 180 { 255 } else { 0 };
            match (strong, light >= 128) {
                // A strong colour keeps its hue, made bright.
                (true, _) => {
                    let m = r.max(g).max(b).max(1);
                    let up = |v: u32| (v * 255 / m).max(0x60);
                    up(r) << 16 | up(g) << 8 | up(b)
                }
                (false, true) => BLACK,
                (false, false) => WHITE,
            }
        }
    }
}

/// How far apart two colours are for the eye, times 100 (WCAG's
/// contrast ratio: 700 is 7 to 1).
pub fn contrast(a: Color, b: Color) -> u32 {
    fn lin(v: u32) -> f32 {
        let v = v as f32 / 255.0;
        if v <= 0.040_45 {
            v / 12.92
        } else {
            // (v + 0.055) / 1.055 to the power 2.4, without std.
            let x = (v + 0.055) / 1.055;
            let l = x * x;
            // x^2.4 = x^2 * x^0.4; x^0.4 by a few Newton steps of y^5 = x^2.
            let mut y = x;
            for _ in 0..30 {
                let y4 = y * y * y * y;
                y -= (y4 * y - l) / (5.0 * y4);
            }
            l * y
        }
    }
    let lum = |c: Color| {
        0.2126 * lin((c >> 16) & 0xff) + 0.7152 * lin((c >> 8) & 0xff) + 0.0722 * lin(c & 0xff)
    };
    let (x, y) = (lum(a), lum(b));
    let (hi, lo) = if x > y { (x, y) } else { (y, x) };
    ((hi + 0.05) / (lo + 0.05) * 100.0) as u32
}

pub struct Canvas<'a> {
    pub px: &'a mut [u32],
    /// The size in points (pixels when the scale is 100).
    pub width: i32,
    pub height: i32,
    /// Pixels from one row to the next.
    pub stride: i32,
    /// The clip, in pixels.
    clip_px: Rect,
    /// The clip as it was asked for, in points.
    clip: Rect,
    /// Percent: pixels per 100 points.
    scale: i32,
    /// The size in pixels.
    pw: i32,
    ph: i32,
    /// Drawn in high contrast: every colour goes through
    /// [`high_contrast`].
    contrast: bool,
}

impl<'a> Canvas<'a> {
    /// A canvas over `px`; `None` if the buffer is too small.
    pub fn new(px: &'a mut [u32], width: i32, height: i32, stride: i32) -> Option<Canvas<'a>> {
        if width < 0 || height < 0 || stride < width {
            return None;
        }
        if height > 0 && px.len() < ((height - 1) * stride + width) as usize {
            return None;
        }
        Some(Canvas {
            px,
            width,
            height,
            stride,
            clip_px: Rect::new(0, 0, width, height),
            clip: Rect::new(0, 0, width, height),
            scale: 100,
            pw: width,
            ph: height,
            contrast: false,
        })
    }

    /// The same picture in high contrast (or not).
    pub fn with_contrast(mut self, on: bool) -> Canvas<'a> {
        self.contrast = on;
        self
    }

    pub fn contrast(&self) -> bool {
        self.contrast
    }

    /// The colour as this canvas draws it.
    pub fn ink(&self, c: Color) -> Color {
        if self.contrast { high_contrast(c) } else { c }
    }

    /// The same picture drawn at `scale` percent (clamped to 50 to 400):
    /// its size becomes the pixel size in points.
    pub fn scaled(mut self, scale: i32) -> Canvas<'a> {
        let s = scale.clamp(50, 400);
        self.scale = s;
        self.width = self.pw * 100 / s;
        self.height = self.ph * 100 / s;
        self.clip = Rect::new(0, 0, self.width, self.height);
        self.clip_px = Rect::new(0, 0, self.pw, self.ph);
        self
    }

    /// Pixels per 100 points.
    pub fn scale(&self) -> i32 {
        self.scale
    }

    /// Points to pixels (rounding down, so neighbours still touch).
    pub fn px_of(&self, v: i32) -> i32 {
        if self.scale == 100 {
            v
        } else {
            (v * self.scale).div_euclid(100)
        }
    }

    /// A rectangle in points to the pixels it covers.
    pub fn px_rect(&self, r: Rect) -> Rect {
        if self.scale == 100 {
            return r;
        }
        let (x0, y0) = (self.px_of(r.x), self.px_of(r.y));
        Rect::new(
            x0,
            y0,
            self.px_of(r.x + r.w) - x0,
            self.px_of(r.y + r.h) - y0,
        )
    }

    pub fn bounds(&self) -> Rect {
        Rect::new(0, 0, self.width, self.height)
    }

    fn pixels(&self) -> Rect {
        Rect::new(0, 0, self.pw, self.ph)
    }

    /// Limit drawing to `r` (within the picture); returns the old clip.
    pub fn set_clip(&mut self, r: Rect) -> Rect {
        let old = self.clip;
        self.clip = r.intersect(self.bounds());
        self.clip_px = self.px_rect(r).intersect(self.pixels());
        old
    }

    pub fn clip(&self) -> Rect {
        self.clip
    }

    fn at(&self, x: i32, y: i32) -> usize {
        (y * self.stride + x) as usize
    }

    pub fn get(&self, x: i32, y: i32) -> Option<Color> {
        let (x, y) = (self.px_of(x), self.px_of(y));
        self.pixels().contains(x, y).then(|| self.px[self.at(x, y)])
    }

    /// Blend one point with `c` at coverage `a` (0 to 255), clipped.
    pub fn blend(&mut self, x: i32, y: i32, c: Color, a: u32) {
        let c = self.ink(c);
        if self.scale != 100 {
            let r = self.px_rect(Rect::new(x, y, 1, 1));
            self.fill_alpha_px(r, c, a);
            return;
        }
        self.blend_px(x, y, c, a);
    }

    /// Blend one pixel (not point) with `c` at coverage `a`, clipped.
    pub fn blend_px(&mut self, x: i32, y: i32, c: Color, a: u32) {
        if a == 0 || !self.clip_px.contains(x, y) {
            return;
        }
        let i = self.at(x, y);
        self.px[i] = if a >= 255 { c } else { mix(self.px[i], c, a) };
    }

    pub fn fill(&mut self, r: Rect, c: Color) {
        let (r, c) = (self.px_rect(r), self.ink(c));
        self.fill_px(r, c);
    }

    fn fill_px(&mut self, r: Rect, c: Color) {
        let r = r.intersect(self.clip_px);
        for y in r.y..r.y + r.h {
            let i = self.at(r.x, y);
            self.px[i..i + r.w as usize].fill(c);
        }
    }

    /// Fill with `c` at `alpha` (0 to 255) over what is there.
    pub fn fill_alpha(&mut self, r: Rect, c: Color, alpha: u32) {
        let (r, c) = (self.px_rect(r), self.ink(c));
        self.fill_alpha_px(r, c, alpha);
    }

    fn fill_alpha_px(&mut self, r: Rect, c: Color, alpha: u32) {
        let r = r.intersect(self.clip_px);
        for y in r.y..r.y + r.h {
            let i = self.at(r.x, y);
            for p in &mut self.px[i..i + r.w as usize] {
                *p = mix(*p, c, alpha);
            }
        }
    }

    /// A vertical gradient from `top` to `bottom` over `r`.
    pub fn gradient(&mut self, r: Rect, top: Color, bottom: Color) {
        let r = self.px_rect(r);
        let (top, bottom) = (self.ink(top), self.ink(bottom));
        let full = r;
        let r = r.intersect(self.clip_px);
        for y in r.y..r.y + r.h {
            let t = ((y - full.y) * 255 / full.h.max(1)) as u32;
            let c = mix(top, bottom, t);
            let i = self.at(r.x, y);
            self.px[i..i + r.w as usize].fill(c);
        }
    }

    /// How much of the pixel at (`x`, `y`) inside a box with corners of
    /// `radius` is covered (0 to 255), sampled 4 by 4.
    fn corner_cover(r: Rect, radius: i32, x: i32, y: i32) -> u32 {
        let (cx, cy) = (
            if x < r.x + radius {
                r.x + radius
            } else if x >= r.x + r.w - radius {
                r.x + r.w - radius
            } else {
                return 255;
            },
            if y < r.y + radius {
                r.y + radius
            } else if y >= r.y + r.h - radius {
                r.y + r.h - radius
            } else {
                return 255;
            },
        );
        // In quarter pixels.
        let rr = (radius * 4) * (radius * 4);
        let mut n = 0;
        for sy in 0..4 {
            for sx in 0..4 {
                let dx = (x * 4 + sx) * 2 + 1 - cx * 8;
                let dy = (y * 4 + sy) * 2 + 1 - cy * 8;
                if dx * dx + dy * dy <= rr * 4 {
                    n += 1;
                }
            }
        }
        n * 255 / 16
    }

    /// A filled rectangle with rounded corners and smooth edges.
    pub fn round_rect(&mut self, r: Rect, radius: i32, c: Color) {
        self.round_rect_alpha(r, radius, c, 255);
    }

    pub fn round_rect_alpha(&mut self, r: Rect, radius: i32, c: Color, alpha: u32) {
        let (r, radius, c) = (self.px_rect(r), self.px_of(radius), self.ink(c));
        self.round_rect_px(r, radius, c, alpha);
    }

    fn round_rect_px(&mut self, r: Rect, radius: i32, c: Color, alpha: u32) {
        let radius = radius.clamp(0, r.w.min(r.h) / 2);
        let area = r.intersect(self.clip_px);
        for y in area.y..area.y + area.h {
            let in_band = y >= r.y + radius && y < r.y + r.h - radius;
            if in_band {
                let row = Rect::new(area.x, y, area.w, 1);
                if alpha >= 255 {
                    self.fill_px(row, c);
                } else {
                    self.fill_alpha_px(row, c, alpha);
                }
                continue;
            }
            for x in area.x..area.x + area.w {
                let a = Self::corner_cover(r, radius, x, y);
                self.blend_px(x, y, c, a * alpha / 255);
            }
        }
    }

    /// A one-point frame around `r`, with rounded corners.
    pub fn round_frame(&mut self, r: Rect, radius: i32, c: Color) {
        let (r, radius, c) = (self.px_rect(r), self.px_of(radius), self.ink(c));
        // As thick as one point is (two in high contrast).
        let thick = (self.scale / 100).max(1) * if self.contrast { 2 } else { 1 };
        for t in 0..thick {
            let inner = Rect::new(r.x + t, r.y + t, r.w - 2 * t, r.h - 2 * t);
            self.round_frame_px(inner, radius - t, c);
        }
    }

    fn round_frame_px(&mut self, r: Rect, radius: i32, c: Color) {
        let radius = radius.clamp(0, r.w.min(r.h) / 2);
        let inner = Rect::new(r.x + 1, r.y + 1, r.w - 2, r.h - 2);
        let ir = (radius - 1).max(0);
        let area = r.intersect(self.clip_px);
        for y in area.y..area.y + area.h {
            for x in area.x..area.x + area.w {
                let outer = Self::corner_cover(r, radius, x, y);
                let inside = if inner.contains(x, y) {
                    Self::corner_cover(inner, ir, x, y)
                } else {
                    0
                };
                if outer > inside {
                    self.blend_px(x, y, c, outer - inside);
                }
            }
        }
    }

    /// A soft shadow under a box: a few widening, fading rounded boxes.
    pub fn shadow(&mut self, r: Rect, radius: i32, depth: i32) {
        let (r, radius, depth) = (self.px_rect(r), self.px_of(radius), self.px_of(depth));
        for i in (1..=depth).rev() {
            let g = Rect::new(r.x - i, r.y - i + depth / 2, r.w + 2 * i, r.h + 2 * i);
            let a = (40 / depth.max(1)) as u32;
            self.round_rect_px(g, radius + i, 0x00_00_00, a);
        }
    }

    /// Blend a glyph's coverage in colour `c`, its top-left at the pixel
    /// (`x`, `y`): glyphs are drawn at the pixel size already.
    pub fn glyph(&mut self, b: &Bitmap, x: i32, y: i32, c: Color) {
        let c = self.ink(c);
        let area = Rect::new(x, y, b.width as i32, b.height as i32).intersect(self.clip_px);
        for py in area.y..area.y + area.h {
            let row = (py - y) as usize * b.width;
            for px in area.x..area.x + area.w {
                let a = u32::from(b.coverage[row + (px - x) as usize]);
                if a != 0 {
                    let i = self.at(px, py);
                    self.px[i] = mix(self.px[i], c, a);
                }
            }
        }
    }

    /// Copy `src` (`w` by `h` pixels, row stride `stride`) with its
    /// top-left at the point (`x`, `y`); pixels are copied as they are.
    pub fn blit(&mut self, src: &[u32], w: i32, h: i32, stride: i32, x: i32, y: i32) {
        let (x, y) = (self.px_of(x), self.px_of(y));
        if w <= 0 || h <= 0 || stride < w || src.len() < ((h - 1) * stride + w) as usize {
            return;
        }
        let area = Rect::new(x, y, w, h).intersect(self.clip_px);
        for py in area.y..area.y + area.h {
            let s = ((py - y) * stride + (area.x - x)) as usize;
            let d = self.at(area.x, py);
            self.px[d..d + area.w as usize].copy_from_slice(&src[s..s + area.w as usize]);
        }
    }
}
