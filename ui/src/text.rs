// SPDX-License-Identifier: GPL-3.0-only
//! Text: Alaya's letters (decision 105), with a fallback font for
//! alphabets the main one lacks, and a cache of drawn glyphs.
//!
//! Text is UTF-8 and drawn left to right (decision 67: right-to-left and
//! joined scripts come in milestone 7b). A character no font has is drawn
//! as an empty box, so missing letters are visible, never silently
//! dropped.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use crate::canvas::{Canvas, Color};
use crate::font::{Bitmap, Font, ceil};

/// Which family a piece of text uses.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Face {
    /// Body text (Figtree).
    Text,
    /// Titles and buttons (Figtree SemiBold).
    Strong,
    /// Code and numbers that line up (IBM Plex Mono).
    Mono,
}

/// The fonts, by face, with the fallback (Noto Sans) last.
pub struct Fonts<'a> {
    text: Option<Font<'a>>,
    strong: Option<Font<'a>>,
    mono: Option<Font<'a>>,
    fallback: Option<Font<'a>>,
    cache: BTreeMap<(u8, u16, u32), Bitmap>,
    /// Glyphs drawn (cache misses), for tests and statistics.
    pub drawn: u64,
}

const FALLBACK: u8 = 3;

impl<'a> Fonts<'a> {
    pub fn new(text: &'a [u8], strong: &'a [u8], mono: &'a [u8], fallback: &'a [u8]) -> Self {
        Fonts {
            text: Font::parse(text),
            strong: Font::parse(strong),
            mono: Font::parse(mono),
            fallback: Font::parse(fallback),
            cache: BTreeMap::new(),
            drawn: 0,
        }
    }

    fn font(&self, i: u8) -> Option<&Font<'a>> {
        match i {
            0 => self.text.as_ref(),
            1 => self.strong.as_ref(),
            2 => self.mono.as_ref(),
            _ => self.fallback.as_ref(),
        }
    }

    fn primary(face: Face) -> u8 {
        match face {
            Face::Text => 0,
            Face::Strong => 1,
            Face::Mono => 2,
        }
    }

    /// The font and glyph for `c`: the face's own font, else the
    /// fallback.
    fn lookup(&self, face: Face, c: char) -> Option<(u8, u16)> {
        let p = Self::primary(face);
        for i in [p, FALLBACK] {
            if let Some(g) = self.font(i).and_then(|f| f.glyph(c)) {
                return Some((i, g));
            }
        }
        None
    }

    /// Whether some font has a glyph for `c`.
    pub fn has(&self, face: Face, c: char) -> bool {
        c == ' ' || self.lookup(face, c).is_some()
    }

    fn scale(&self, i: u8, px: u32) -> f32 {
        self.font(i)
            .map_or(0.0, |f| px as f32 / f32::from(f.units_per_em))
    }

    /// The distance from the top of a line to its baseline, and the line
    /// height, in pixels, for `px`-pixel text.
    pub fn metrics(&self, face: Face, px: u32) -> (i32, i32) {
        let i = Self::primary(face);
        match self.font(i).or(self.fallback.as_ref()) {
            Some(f) => {
                let s = px as f32 / f32::from(f.units_per_em);
                let asc = ceil(f32::from(f.ascender) * s) as i32;
                let desc = ceil(f32::from(-f.descender) * s) as i32;
                let gap = ceil(f32::from(f.line_gap.max(0)) * s) as i32;
                (asc, asc + desc + gap)
            }
            None => (px as i32, px as i32 * 5 / 4),
        }
    }

    /// How far the pen moves for `c`.
    pub fn advance(&self, face: Face, px: u32, c: char) -> i32 {
        match self.lookup(face, c) {
            Some((i, g)) => {
                let f = self.font(i).unwrap();
                (f32::from(f.advance(g)) * self.scale(i, px) + 0.5) as i32
            }
            None if c == ' ' => px as i32 / 3,
            None => px as i32 * 3 / 5,
        }
    }

    /// The width of `s` in pixels.
    pub fn width(&self, face: Face, px: u32, s: &str) -> i32 {
        if crate::script::needs_work(s) {
            // Joined Arabic is narrower than its letters one by one.
            let v = crate::script::visual(s);
            return v.chars().map(|c| self.advance(face, px, c)).sum();
        }
        s.chars().map(|c| self.advance(face, px, c)).sum()
    }

    fn bitmap(&mut self, i: u8, g: u16, px: u32) -> &Bitmap {
        let key = (i, g, px);
        if !self.cache.contains_key(&key) {
            let b = self
                .font(i)
                .map(|f| f.rasterize(g, px as f32))
                .unwrap_or_default();
            self.drawn += 1;
            // Keep the cache bounded.
            if self.cache.len() > 4096 {
                self.cache.clear();
            }
            self.cache.insert(key, b);
        }
        &self.cache[&key]
    }

    /// Draw `s` with its baseline at `y`, starting at `x`; returns the x
    /// after the last character.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        cv: &mut Canvas,
        face: Face,
        px: u32,
        x: i32,
        y: i32,
        s: &str,
        c: Color,
    ) -> i32 {
        // Right-to-left text: joined, in the order it is seen.
        let seen;
        let s = if crate::script::needs_work(s) {
            seen = crate::script::visual(s);
            seen.as_str()
        } else {
            s
        };
        let mut pen = x;
        let clip = cv.clip();
        // On a scaled canvas the letters are drawn at the pixel size, each
        // where its point position falls, so text measures the same.
        let ppx = (px as i32 * cv.scale() / 100).max(1) as u32;
        let base = cv.px_of(y);
        for ch in s.chars() {
            if pen > clip.x + clip.w {
                break;
            }
            let adv = self.advance(face, px, ch);
            match self.lookup(face, ch) {
                Some((i, g)) => {
                    let at = cv.px_of(pen);
                    let b = self.bitmap(i, g, ppx);
                    cv.glyph(b, at + b.left, base + b.top, c);
                }
                None if ch == ' ' || ch.is_control() => {}
                None => {
                    // A visible box for a missing letter.
                    let h = px as i32 * 7 / 10;
                    let r = crate::Rect::new(pen + 1, y - h, adv - 2, h);
                    for (xx, yy) in r.outline_points() {
                        cv.blend(xx, yy, c, 160);
                    }
                }
            }
            pen += adv;
        }
        pen
    }

    /// Split `s` into lines no wider than `max` pixels, breaking at spaces
    /// where it can. Returns byte ranges.
    pub fn wrap(&self, face: Face, px: u32, s: &str, max: i32) -> Vec<(usize, usize)> {
        let mut lines = Vec::new();
        for (pstart, para) in split_inclusive_offsets(s) {
            let mut start = 0;
            let mut width = 0;
            let mut last_space = None;
            for (i, ch) in para.char_indices() {
                let w = self.advance(face, px, ch);
                if ch == ' ' {
                    last_space = Some(i);
                }
                if width + w > max && i > start {
                    let cut = match last_space {
                        Some(sp) if sp > start => sp + 1,
                        _ => i,
                    };
                    lines.push((pstart + start, pstart + cut));
                    start = cut;
                    last_space = None;
                    if cut > i {
                        // The space that did not fit ends the line.
                        width = 0;
                        continue;
                    }
                    width = self.width(face, px, &para[cut..i]);
                }
                width += w;
            }
            lines.push((pstart + start, pstart + para.len()));
        }
        lines
    }
}

/// Paragraphs of `s` (split at '\n', which is left out) with their byte
/// offsets.
fn split_inclusive_offsets(s: &str) -> impl Iterator<Item = (usize, &str)> {
    let mut at = 0;
    s.split('\n').map(move |p| {
        let start = at;
        at += p.len() + 1;
        (start, p)
    })
}
