// SPDX-License-Identifier: GPL-3.0-only
//! Drawing into a picture of 32-bit pixels (0x00RRGGBB, row after row):
//! filled and rounded rectangles, soft shadows, frames, gradients, and
//! glyph coverage blended in a colour. Everything is clipped to the
//! picture and to a clip rectangle, so a wrong size never writes outside.

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

pub struct Canvas<'a> {
    pub px: &'a mut [u32],
    pub width: i32,
    pub height: i32,
    /// Pixels from one row to the next.
    pub stride: i32,
    clip: Rect,
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
            clip: Rect::new(0, 0, width, height),
        })
    }

    pub fn bounds(&self) -> Rect {
        Rect::new(0, 0, self.width, self.height)
    }

    /// Limit drawing to `r` (within the picture); returns the old clip.
    pub fn set_clip(&mut self, r: Rect) -> Rect {
        let old = self.clip;
        self.clip = r.intersect(self.bounds());
        old
    }

    pub fn clip(&self) -> Rect {
        self.clip
    }

    fn at(&self, x: i32, y: i32) -> usize {
        (y * self.stride + x) as usize
    }

    pub fn get(&self, x: i32, y: i32) -> Option<Color> {
        self.bounds().contains(x, y).then(|| self.px[self.at(x, y)])
    }

    /// Blend one pixel with `c` at coverage `a` (0 to 255), clipped.
    pub fn blend(&mut self, x: i32, y: i32, c: Color, a: u32) {
        if a == 0 || !self.clip.contains(x, y) {
            return;
        }
        let i = self.at(x, y);
        self.px[i] = if a >= 255 { c } else { mix(self.px[i], c, a) };
    }

    pub fn fill(&mut self, r: Rect, c: Color) {
        let r = r.intersect(self.clip);
        for y in r.y..r.y + r.h {
            let i = self.at(r.x, y);
            self.px[i..i + r.w as usize].fill(c);
        }
    }

    /// Fill with `c` at `alpha` (0 to 255) over what is there.
    pub fn fill_alpha(&mut self, r: Rect, c: Color, alpha: u32) {
        let r = r.intersect(self.clip);
        for y in r.y..r.y + r.h {
            let i = self.at(r.x, y);
            for p in &mut self.px[i..i + r.w as usize] {
                *p = mix(*p, c, alpha);
            }
        }
    }

    /// A vertical gradient from `top` to `bottom` over `r`.
    pub fn gradient(&mut self, r: Rect, top: Color, bottom: Color) {
        let full = r;
        let r = r.intersect(self.clip);
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
        let radius = radius.clamp(0, r.w.min(r.h) / 2);
        let area = r.intersect(self.clip);
        for y in area.y..area.y + area.h {
            let in_band = y >= r.y + radius && y < r.y + r.h - radius;
            if in_band {
                let row = Rect::new(area.x, y, area.w, 1);
                if alpha >= 255 {
                    self.fill(row, c);
                } else {
                    self.fill_alpha(row, c, alpha);
                }
                continue;
            }
            for x in area.x..area.x + area.w {
                let a = Self::corner_cover(r, radius, x, y);
                self.blend(x, y, c, a * alpha / 255);
            }
        }
    }

    /// A one-pixel frame around `r`, with rounded corners.
    pub fn round_frame(&mut self, r: Rect, radius: i32, c: Color) {
        let radius = radius.clamp(0, r.w.min(r.h) / 2);
        let inner = Rect::new(r.x + 1, r.y + 1, r.w - 2, r.h - 2);
        let ir = (radius - 1).max(0);
        let area = r.intersect(self.clip);
        for y in area.y..area.y + area.h {
            for x in area.x..area.x + area.w {
                let outer = Self::corner_cover(r, radius, x, y);
                let inside = if inner.contains(x, y) {
                    Self::corner_cover(inner, ir, x, y)
                } else {
                    0
                };
                if outer > inside {
                    self.blend(x, y, c, outer - inside);
                }
            }
        }
    }

    /// A soft shadow under a box: a few widening, fading rounded boxes.
    pub fn shadow(&mut self, r: Rect, radius: i32, depth: i32) {
        for i in (1..=depth).rev() {
            let g = Rect::new(r.x - i, r.y - i + depth / 2, r.w + 2 * i, r.h + 2 * i);
            let a = (40 / depth.max(1)) as u32;
            self.round_rect_alpha(g, radius + i, 0x00_00_00, a);
        }
    }

    /// Blend a glyph's coverage in colour `c`, its top-left at (`x`, `y`).
    pub fn glyph(&mut self, b: &Bitmap, x: i32, y: i32, c: Color) {
        let area = Rect::new(x, y, b.width as i32, b.height as i32).intersect(self.clip);
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

    /// Copy `src` (`w` by `h`, row stride `stride`) with its top-left at
    /// (`x`, `y`).
    pub fn blit(&mut self, src: &[u32], w: i32, h: i32, stride: i32, x: i32, y: i32) {
        if w <= 0 || h <= 0 || stride < w || src.len() < ((h - 1) * stride + w) as usize {
            return;
        }
        let area = Rect::new(x, y, w, h).intersect(self.clip);
        for py in area.y..area.y + area.h {
            let s = ((py - y) * stride + (area.x - x)) as usize;
            let d = self.at(area.x, py);
            self.px[d..d + area.w as usize].copy_from_slice(&src[s..s + area.w as usize]);
        }
    }
}
