// SPDX-License-Identifier: GPL-3.0-only
//! Alaya's toolkit and desktop logic (milestone 6): letters, drawing, the
//! window rules of the compositor, and the pieces apps are built from.
//!
//! Like the store, it does not care where its pixels or events come from,
//! so all of it is tested on the build machine (`cargo test -p alaya-ui`)
//! as well as inside Alaya.

#![no_std]

extern crate alloc;
#[cfg(test)]
extern crate std;

pub mod canvas;
pub mod diff;
pub mod font;
pub mod login;
pub mod scene;
pub mod text;
pub mod widgets;
pub mod wm;

#[cfg(test)]
mod tests;

/// A rectangle in pixels: left, top, width, height.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Rect {
        Rect { x, y, w, h }
    }

    pub fn is_empty(&self) -> bool {
        self.w <= 0 || self.h <= 0
    }

    pub fn right(&self) -> i32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> i32 {
        self.y + self.h
    }

    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x && y >= self.y && x < self.right() && y < self.bottom()
    }

    pub fn intersect(&self, o: Rect) -> Rect {
        let x = self.x.max(o.x);
        let y = self.y.max(o.y);
        let r = self.right().min(o.right());
        let b = self.bottom().min(o.bottom());
        if r <= x || b <= y {
            Rect::new(x, y, 0, 0)
        } else {
            Rect::new(x, y, r - x, b - y)
        }
    }

    /// The smallest rectangle holding both (an empty one adds nothing).
    pub fn union(&self, o: Rect) -> Rect {
        if self.is_empty() {
            return o;
        }
        if o.is_empty() {
            return *self;
        }
        let x = self.x.min(o.x);
        let y = self.y.min(o.y);
        Rect::new(
            x,
            y,
            self.right().max(o.right()) - x,
            self.bottom().max(o.bottom()) - y,
        )
    }

    pub fn grow(&self, by: i32) -> Rect {
        Rect::new(self.x - by, self.y - by, self.w + 2 * by, self.h + 2 * by)
    }

    /// The pixels on the rectangle's edge.
    pub fn outline_points(&self) -> impl Iterator<Item = (i32, i32)> + '_ {
        let top = (self.x..self.right()).map(move |x| (x, self.y));
        let bottom = (self.x..self.right()).map(move |x| (x, self.bottom() - 1));
        let left = (self.y..self.bottom()).map(move |y| (self.x, y));
        let right = (self.y..self.bottom()).map(move |y| (self.right() - 1, y));
        top.chain(bottom).chain(left).chain(right)
    }
}
