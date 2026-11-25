// SPDX-License-Identifier: GPL-3.0-only WITH AdditionRef-Alaya-system-library-exception
//! Alaya's own reader for TrueType font files, and the rasteriser that
//! turns a letter's outline into soft-edged pixels.
//!
//! It reads only what drawing left-to-right text needs: which glyph a
//! character uses (`cmap`), its outline (`glyf`, `loca`, including glyphs
//! built from other glyphs), how far to move after it (`hmtx`) and the
//! line height (`hhea`). Every read is bounds-checked: a broken or hostile
//! font file gives missing letters, never a crash.
//!
//! The rasteriser adds up, for every pixel, how much of it each edge of
//! the outline covers (the method of the `font-rs` and `ab_glyph`
//! rasterisers, written again here), so letters get smooth edges without
//! any super-sampling.

use alloc::vec;
use alloc::vec::Vec;

fn u16_at(d: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_be_bytes([*d.get(o)?, *d.get(o + 1)?]))
}
fn i16_at(d: &[u8], o: usize) -> Option<i16> {
    u16_at(d, o).map(|v| v as i16)
}
fn u32_at(d: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_be_bytes([
        *d.get(o)?,
        *d.get(o + 1)?,
        *d.get(o + 2)?,
        *d.get(o + 3)?,
    ]))
}

/// `floor` without the standard library (and without `libm`).
pub fn floor(x: f32) -> f32 {
    let i = x as i32 as f32;
    if i > x { i - 1.0 } else { i }
}
pub fn ceil(x: f32) -> f32 {
    let i = x as i32 as f32;
    if i < x { i + 1.0 } else { i }
}
fn abs(x: f32) -> f32 {
    if x < 0.0 { -x } else { x }
}

/// A parsed font file. It borrows the file's bytes.
#[derive(Clone, Copy)]
pub struct Font<'a> {
    data: &'a [u8],
    glyf: (usize, usize),
    loca: (usize, usize),
    hmtx: (usize, usize),
    cmap: usize,
    cmap_format: u16,
    long_loca: bool,
    glyphs: u16,
    h_metrics: u16,
    pub units_per_em: u16,
    pub ascender: i16,
    pub descender: i16,
    pub line_gap: i16,
}

/// A line or a curve of an outline, in font units, y going up.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Segment {
    Line((f32, f32), (f32, f32)),
    Quad((f32, f32), (f32, f32), (f32, f32)),
}

impl<'a> Font<'a> {
    pub fn parse(data: &'a [u8]) -> Option<Font<'a>> {
        let tag = u32_at(data, 0)?;
        // TrueType outlines only ("\0\1\0\0" or "true"), not CFF ("OTTO").
        if tag != 0x0001_0000 && tag != 0x7472_7565 {
            return None;
        }
        let n = u16_at(data, 4)? as usize;
        let table = |name: &[u8; 4]| -> Option<(usize, usize)> {
            let want = u32::from_be_bytes(*name);
            (0..n).find_map(|i| {
                let r = 12 + 16 * i;
                (u32_at(data, r)? == want).then(|| {
                    let off = u32_at(data, r + 8)? as usize;
                    let len = u32_at(data, r + 12)? as usize;
                    (off.checked_add(len)? <= data.len()).then_some((off, len))
                })?
            })
        };
        let head = table(b"head")?;
        let maxp = table(b"maxp")?;
        let hhea = table(b"hhea")?;
        let hmtx = table(b"hmtx")?;
        let loca = table(b"loca")?;
        let glyf = table(b"glyf")?;
        let cmap_t = table(b"cmap")?;
        let units_per_em = u16_at(data, head.0 + 18)?.max(16);
        let long_loca = i16_at(data, head.0 + 50)? != 0;
        let glyphs = u16_at(data, maxp.0 + 4)?;
        let ascender = i16_at(data, hhea.0 + 4)?;
        let descender = i16_at(data, hhea.0 + 6)?;
        let line_gap = i16_at(data, hhea.0 + 8)?;
        let h_metrics = u16_at(data, hhea.0 + 34)?.max(1);

        // Prefer the full Unicode map (format 12), else the basic one
        // (format 4).
        let records = u16_at(data, cmap_t.0 + 2)? as usize;
        let mut best: Option<(usize, u16)> = None;
        for i in 0..records {
            let r = cmap_t.0 + 4 + 8 * i;
            let platform = u16_at(data, r)?;
            let encoding = u16_at(data, r + 2)?;
            let off = cmap_t.0 + u32_at(data, r + 4)? as usize;
            let format = u16_at(data, off)?;
            let unicode = platform == 0 || (platform == 3 && (encoding == 1 || encoding == 10));
            if !unicode || (format != 4 && format != 12) {
                continue;
            }
            if best.is_none_or(|(_, f)| f == 4 && format == 12) {
                best = Some((off, format));
            }
        }
        let (cmap, cmap_format) = best?;
        Some(Font {
            data,
            glyf,
            loca,
            hmtx,
            cmap,
            cmap_format,
            long_loca,
            glyphs,
            h_metrics,
            units_per_em,
            ascender,
            descender,
            line_gap,
        })
    }

    /// The glyph for a character, if the font has one.
    pub fn glyph(&self, c: char) -> Option<u16> {
        let c = c as u32;
        let d = self.data;
        let t = self.cmap;
        let g = if self.cmap_format == 12 {
            let groups = u32_at(d, t + 12)? as usize;
            // Binary search over the groups.
            let (mut lo, mut hi) = (0usize, groups);
            let mut found = None;
            while lo < hi {
                let mid = (lo + hi) / 2;
                let r = t + 16 + 12 * mid;
                let (start, end) = (u32_at(d, r)?, u32_at(d, r + 4)?);
                if c < start {
                    hi = mid;
                } else if c > end {
                    lo = mid + 1;
                } else {
                    found = Some(u32_at(d, r + 8)? + (c - start));
                    break;
                }
            }
            found?
        } else {
            if c > 0xffff {
                return None;
            }
            let segs = (u16_at(d, t + 6)? / 2) as usize;
            let ends = t + 14;
            let starts = ends + 2 * segs + 2;
            let deltas = starts + 2 * segs;
            let ranges = deltas + 2 * segs;
            let i =
                (0..segs).find(|&i| u16_at(d, ends + 2 * i).is_some_and(|e| u32::from(e) >= c))?;
            let start = u32::from(u16_at(d, starts + 2 * i)?);
            if c < start {
                return None;
            }
            let delta = u16_at(d, deltas + 2 * i)?;
            let range = u16_at(d, ranges + 2 * i)? as usize;
            if range == 0 {
                u32::from((c as u16).wrapping_add(delta))
            } else {
                let at = ranges + 2 * i + range + 2 * (c - start) as usize;
                let g = u16_at(d, at)?;
                if g == 0 {
                    return None;
                }
                u32::from(g.wrapping_add(delta))
            }
        };
        (g != 0 && g < u32::from(self.glyphs)).then_some(g as u16)
    }

    /// How far to move after a glyph, in font units.
    pub fn advance(&self, g: u16) -> u16 {
        let i = g.min(self.h_metrics - 1) as usize;
        u16_at(self.data, self.hmtx.0 + 4 * i).unwrap_or(0)
    }

    fn glyph_range(&self, g: u16) -> Option<(usize, usize)> {
        if g >= self.glyphs {
            return None;
        }
        let i = g as usize;
        let (a, b) = if self.long_loca {
            (
                u32_at(self.data, self.loca.0 + 4 * i)? as usize,
                u32_at(self.data, self.loca.0 + 4 * i + 4)? as usize,
            )
        } else {
            (
                u16_at(self.data, self.loca.0 + 2 * i)? as usize * 2,
                u16_at(self.data, self.loca.0 + 2 * i + 2)? as usize * 2,
            )
        };
        if a >= b || b > self.glyf.1 {
            return None;
        }
        Some((self.glyf.0 + a, self.glyf.0 + b))
    }

    /// The glyph's outline, as lines and curves in font units.
    pub fn outline(&self, g: u16, out: &mut Vec<Segment>) {
        self.outline_at(g, (1.0, 0.0, 0.0, 1.0), (0.0, 0.0), out, 0);
    }

    fn outline_at(
        &self,
        g: u16,
        m: (f32, f32, f32, f32),
        off: (f32, f32),
        out: &mut Vec<Segment>,
        depth: u32,
    ) -> Option<()> {
        if depth > 6 {
            return None;
        }
        let (start, end) = self.glyph_range(g)?;
        let d = &self.data[..end];
        let contours = i16_at(d, start)?;
        let tf = |x: f32, y: f32| (m.0 * x + m.2 * y + off.0, m.1 * x + m.3 * y + off.1);
        if contours < 0 {
            // Built from other glyphs.
            let mut at = start + 10;
            loop {
                let flags = u16_at(d, at)?;
                let child = u16_at(d, at + 2)?;
                at += 4;
                let (dx, dy) = if flags & 1 != 0 {
                    let v = (i16_at(d, at)?, i16_at(d, at + 2)?);
                    at += 4;
                    (f32::from(v.0), f32::from(v.1))
                } else {
                    let v = (*d.get(at)? as i8, *d.get(at + 1)? as i8);
                    at += 2;
                    (f32::from(v.0), f32::from(v.1))
                };
                let f2 = |o: usize| i16_at(d, o).map(|v| f32::from(v) / 16384.0);
                let mut cm = (1.0, 0.0, 0.0, 1.0);
                if flags & 0x08 != 0 {
                    let s = f2(at)?;
                    cm = (s, 0.0, 0.0, s);
                    at += 2;
                } else if flags & 0x40 != 0 {
                    cm = (f2(at)?, 0.0, 0.0, f2(at + 2)?);
                    at += 4;
                } else if flags & 0x80 != 0 {
                    cm = (f2(at)?, f2(at + 2)?, f2(at + 4)?, f2(at + 6)?);
                    at += 8;
                }
                // Offsets only (point matching is not supported).
                let (dx, dy) = if flags & 2 != 0 { (dx, dy) } else { (0.0, 0.0) };
                let nm = (
                    m.0 * cm.0 + m.2 * cm.1,
                    m.1 * cm.0 + m.3 * cm.1,
                    m.0 * cm.2 + m.2 * cm.3,
                    m.1 * cm.2 + m.3 * cm.3,
                );
                let no = tf(dx, dy);
                self.outline_at(child, nm, no, out, depth + 1);
                if flags & 0x20 == 0 {
                    break;
                }
            }
            return Some(());
        }
        let n = contours as usize;
        let mut ends = Vec::with_capacity(n);
        for i in 0..n {
            ends.push(u16_at(d, start + 10 + 2 * i)? as usize);
        }
        let points = ends.last().map_or(0, |e| e + 1);
        if points == 0 || ends.windows(2).any(|w| w[1] < w[0]) || points > 10_000 {
            return Some(());
        }
        let ins = u16_at(d, start + 10 + 2 * n)? as usize;
        let mut at = start + 12 + 2 * n + ins;
        let mut flags = Vec::with_capacity(points);
        while flags.len() < points {
            let f = *d.get(at)?;
            at += 1;
            flags.push(f);
            if f & 8 != 0 {
                let r = *d.get(at)?;
                at += 1;
                for _ in 0..r {
                    if flags.len() < points {
                        flags.push(f);
                    }
                }
            }
        }
        let mut xs = Vec::with_capacity(points);
        let mut v = 0i32;
        for &f in &flags {
            if f & 2 != 0 {
                let b = i32::from(*d.get(at)?);
                at += 1;
                v += if f & 16 != 0 { b } else { -b };
            } else if f & 16 == 0 {
                v += i32::from(i16_at(d, at)?);
                at += 2;
            }
            xs.push(v);
        }
        let mut v = 0i32;
        let mut pts = Vec::with_capacity(points);
        for (i, &f) in flags.iter().enumerate() {
            if f & 4 != 0 {
                let b = i32::from(*d.get(at)?);
                at += 1;
                v += if f & 32 != 0 { b } else { -b };
            } else if f & 32 == 0 {
                v += i32::from(i16_at(d, at)?);
                at += 2;
            }
            pts.push((tf(xs[i] as f32, v as f32), f & 1 != 0));
        }
        let mut first = 0;
        for &e in &ends {
            if e < first || e >= pts.len() {
                break;
            }
            contour(&pts[first..=e], out);
            first = e + 1;
        }
        Some(())
    }
}

fn mid(a: (f32, f32), b: (f32, f32)) -> (f32, f32) {
    ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0)
}

/// One closed contour of on- and off-curve points into segments. Two
/// off-curve points in a row have an on-curve point between them.
fn contour(p: &[((f32, f32), bool)], out: &mut Vec<Segment>) {
    let n = p.len();
    if n < 2 {
        return;
    }
    // Start at an on-curve point (or between two off-curve ones).
    let (start, first) = match p.iter().position(|q| q.1) {
        Some(i) => (i, p[i].0),
        None => (0, mid(p[0].0, p[1].0)),
    };
    let mut cur = first;
    let mut ctrl: Option<(f32, f32)> = None;
    for k in 1..=n {
        let (pt, on) = p[(start + k) % n];
        match (on, ctrl) {
            (true, None) => {
                out.push(Segment::Line(cur, pt));
                cur = pt;
            }
            (true, Some(c)) => {
                out.push(Segment::Quad(cur, c, pt));
                cur = pt;
                ctrl = None;
            }
            (false, None) => ctrl = Some(pt),
            (false, Some(c)) => {
                let m = mid(c, pt);
                out.push(Segment::Quad(cur, c, m));
                cur = m;
                ctrl = Some(pt);
            }
        }
    }
    match ctrl {
        Some(c) => out.push(Segment::Quad(cur, c, first)),
        None if cur != first => out.push(Segment::Line(cur, first)),
        None => {}
    }
}

/// A glyph drawn at one size: coverage 0..=255 per pixel, and where its
/// top-left pixel goes relative to the pen (x right, y down from the
/// baseline).
#[derive(Clone, Debug, Default)]
pub struct Bitmap {
    pub width: usize,
    pub height: usize,
    pub left: i32,
    pub top: i32,
    pub coverage: Vec<u8>,
}

/// Adds up how much of each pixel the outline covers.
pub struct Raster {
    w: usize,
    h: usize,
    a: Vec<f32>,
}

impl Raster {
    pub fn new(w: usize, h: usize) -> Raster {
        Raster {
            w,
            h,
            a: vec![0.0; w * h + 4],
        }
    }

    /// A straight edge from `p0` to `p1`, in pixels (y down).
    pub fn line(&mut self, p0: (f32, f32), p1: (f32, f32)) {
        if abs(p0.1 - p1.1) <= 1e-4 {
            return;
        }
        let (dir, p0, p1) = if p0.1 < p1.1 {
            (1.0, p0, p1)
        } else {
            (-1.0, p1, p0)
        };
        let dxdy = (p1.0 - p0.0) / (p1.1 - p0.1);
        let mut x = p0.0;
        let y0 = if p0.1 < 0.0 {
            x -= p0.1 * dxdy;
            0
        } else {
            p0.1 as usize
        };
        let yend = (ceil(p1.1).max(0.0) as usize).min(self.h);
        let wmax = self.w as f32 - 1.001;
        for y in y0..yend {
            let row = y * self.w;
            let dy = ((y + 1) as f32).min(p1.1) - (y as f32).max(p0.1);
            let xnext = x + dxdy * dy;
            let d = dy * dir;
            let (x0, x1) = if x < xnext { (x, xnext) } else { (xnext, x) };
            let (x0, x1) = (x0.clamp(0.0, wmax), x1.clamp(0.0, wmax));
            let x0f = floor(x0);
            let x0i = x0f as usize;
            let x1c = ceil(x1);
            let x1i = x1c as usize;
            if x1i <= x0i + 1 {
                let xm = 0.5 * (x0 + x1) - x0f;
                self.a[row + x0i] += d - d * xm;
                self.a[row + x0i + 1] += d * xm;
            } else {
                let s = 1.0 / (x1 - x0);
                let fx0 = x0 - x0f;
                let a0 = 0.5 * s * (1.0 - fx0) * (1.0 - fx0);
                let fx1 = x1 - x1c + 1.0;
                let am = 0.5 * s * fx1 * fx1;
                self.a[row + x0i] += d * a0;
                if x1i == x0i + 2 {
                    self.a[row + x0i + 1] += d * (1.0 - a0 - am);
                } else {
                    let a1 = s * (1.5 - fx0);
                    self.a[row + x0i + 1] += d * (a1 - a0);
                    for xi in x0i + 2..x1i - 1 {
                        self.a[row + xi] += d * s;
                    }
                    let a2 = a1 + (x1i - x0i - 3) as f32 * s;
                    self.a[row + x1i - 1] += d * (1.0 - a2 - am);
                }
                self.a[row + x1i] += d * am;
            }
            x = xnext;
        }
    }

    /// A curve, flattened into short lines.
    pub fn quad(&mut self, p0: (f32, f32), c: (f32, f32), p1: (f32, f32)) {
        let dev = abs(p0.0 - 2.0 * c.0 + p1.0) + abs(p0.1 - 2.0 * c.1 + p1.1);
        // About one line per third of a pixel of bend, at least one.
        let mut n = 1usize;
        let mut d2 = dev * 3.0;
        while d2 > 1.0 && n < 32 {
            d2 /= 4.0;
            n *= 2;
        }
        let mut prev = p0;
        for i in 1..=n {
            let t = i as f32 / n as f32;
            let u = 1.0 - t;
            let p = (
                u * u * p0.0 + 2.0 * u * t * c.0 + t * t * p1.0,
                u * u * p0.1 + 2.0 * u * t * c.1 + t * t * p1.1,
            );
            self.line(prev, p);
            prev = p;
        }
    }

    /// Coverage per pixel, 0 to 255.
    pub fn finish(self) -> Vec<u8> {
        let mut acc = 0.0f32;
        let mut out = Vec::with_capacity(self.w * self.h);
        for &v in &self.a[..self.w * self.h] {
            acc += v;
            let c = abs(acc).min(1.0);
            out.push((c * 255.0 + 0.5) as u8);
        }
        out
    }
}

impl Font<'_> {
    /// Draw glyph `g` at `px` pixels per em.
    pub fn rasterize(&self, g: u16, px: f32) -> Bitmap {
        let mut segs = Vec::new();
        self.outline(g, &mut segs);
        if segs.is_empty() {
            return Bitmap::default();
        }
        let s = px / f32::from(self.units_per_em);
        let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        let mut grow = |p: (f32, f32)| {
            x0 = x0.min(p.0);
            x1 = x1.max(p.0);
            y0 = y0.min(p.1);
            y1 = y1.max(p.1);
        };
        for sg in &segs {
            match *sg {
                Segment::Line(a, b) => {
                    grow(a);
                    grow(b);
                }
                Segment::Quad(a, c, b) => {
                    grow(a);
                    grow(c);
                    grow(b);
                }
            }
        }
        // Pixel box (y down), one spare pixel on each side.
        let left = floor(x0 * s) as i32 - 1;
        let top = floor(-y1 * s) as i32 - 1;
        let w = (ceil(x1 * s) as i32 - left + 2).clamp(1, 1024) as usize;
        let h = (ceil(-y0 * s) as i32 - top + 2).clamp(1, 1024) as usize;
        let tf = |p: (f32, f32)| (p.0 * s - left as f32, -p.1 * s - top as f32);
        let mut r = Raster::new(w, h);
        for sg in &segs {
            match *sg {
                Segment::Line(a, b) => r.line(tf(a), tf(b)),
                Segment::Quad(a, c, b) => r.quad(tf(a), tf(c), tf(b)),
            }
        }
        Bitmap {
            width: w,
            height: h,
            left,
            top,
            coverage: r.finish(),
        }
    }
}
