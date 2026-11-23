// SPDX-License-Identifier: GPL-3.0-only
//! The window rules of the compositor (decisions 34 and 87): which window
//! is in front, what the mouse grabbed, moving and resizing, snapping to
//! a half or the whole screen, and tiling.
//!
//! This is only the logic: it says which parts of the screen changed and
//! what to tell apps, and the compositor draws. That way every rule is
//! tested on the build machine with made-up mouse moves.

use alloc::vec::Vec;

use crate::Rect;

/// Height of the strip at the top of the screen that only Alaya draws.
pub const STRIP: i32 = 34;
/// Height of a window's title bar.
pub const TITLE: i32 = 32;
/// How far outside a window's edge the mouse can still grab it to resize.
pub const EDGE: i32 = 6;
/// The smallest a window gets (its content).
pub const MIN_W: i32 = 220;
pub const MIN_H: i32 = 120;
/// The gap between tiled windows and around them.
pub const GAP: i32 = 8;
/// What a window's shadow adds around it, for damage.
pub const SHADOW: i32 = 18;
/// Size of the close button, at the right of the title bar.
pub const CLOSE: i32 = 22;

/// Where a window sits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    Free,
    Left,
    Right,
    Full,
    Tiled,
}

#[derive(Clone, Debug)]
pub struct Window {
    pub id: u32,
    /// The whole window: title bar and content.
    pub frame: Rect,
    pub place: Place,
    /// Where it goes back to when it is no longer snapped or tiled.
    pub free: Rect,
    /// The loose group it belongs to (a label only, decision 85).
    pub group: Option<u32>,
}

impl Window {
    pub fn content(&self) -> Rect {
        Rect::new(
            self.frame.x,
            self.frame.y + TITLE,
            self.frame.w,
            self.frame.h - TITLE,
        )
    }

    pub fn title_bar(&self) -> Rect {
        Rect::new(self.frame.x, self.frame.y, self.frame.w, TITLE)
    }

    pub fn close_button(&self) -> Rect {
        Rect::new(
            self.frame.right() - CLOSE - 6,
            self.frame.y + (TITLE - CLOSE) / 2,
            CLOSE,
            CLOSE,
        )
    }
}

/// Which edges a resize moves.
pub mod edge {
    pub const LEFT: u8 = 1;
    pub const RIGHT: u8 = 2;
    pub const TOP: u8 = 4;
    pub const BOTTOM: u8 = 8;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    Strip,
    Desktop,
    Title(u32),
    Close(u32),
    Edge(u32, u8),
    /// Inside the content, in the content's own coordinates.
    Content(u32, i32, i32),
}

#[derive(Clone, Copy, Debug)]
enum Grab {
    Move {
        id: u32,
        /// Where in the window the mouse holds it.
        dx: i32,
        dy: i32,
        /// Where the mouse went down: a snapped window comes loose only
        /// after a real move.
        x0: i32,
        y0: i32,
        moved: bool,
    },
    Resize {
        id: u32,
        edges: u8,
        start: Rect,
        x0: i32,
        y0: i32,
    },
    /// The press went to an app: it gets the moves and the release.
    App(u32),
}

/// What the compositor must do after an event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Out {
    /// Draw this part of the screen again.
    Damage(Rect),
    /// The window in front changed (keys now go to it).
    Focus(u32),
    /// The window's content has a new size: tell its app.
    Resized(u32, i32, i32),
    /// The close button was pressed: ask the app to close.
    Close(u32),
    /// Pointer event for an app, in its content's coordinates.
    Pointer {
        id: u32,
        x: i32,
        y: i32,
        button: u8,
        down: bool,
        moved: bool,
    },
}

pub struct Wm {
    /// The screen without the top strip.
    pub area: Rect,
    /// Back to front: the last one is in front and has the keyboard.
    pub wins: Vec<Window>,
    grab: Option<Grab>,
    /// Where a dragged window would snap if let go now.
    pub preview: Option<Rect>,
    pub tiling: bool,
    pub pointer: (i32, i32),
    next_cascade: i32,
    /// The order windows take tiles in (not the front-to-back order).
    tiles: Vec<u32>,
}

impl Wm {
    pub fn new(width: i32, height: i32) -> Wm {
        Wm {
            area: Rect::new(0, STRIP, width, height - STRIP),
            wins: Vec::new(),
            grab: None,
            preview: None,
            tiling: false,
            pointer: (width / 2, height / 2),
            next_cascade: 0,
            tiles: Vec::new(),
        }
    }

    pub fn focused(&self) -> Option<u32> {
        self.wins.last().map(|w| w.id)
    }

    pub fn get(&self, id: u32) -> Option<&Window> {
        self.wins.iter().find(|w| w.id == id)
    }

    fn index(&self, id: u32) -> Option<usize> {
        self.wins.iter().position(|w| w.id == id)
    }

    fn damage(r: Rect) -> Out {
        Out::Damage(r.grow(SHADOW))
    }

    /// Keep a frame usable: within the area, at least the minimum size,
    /// its title bar reachable.
    fn fit(&self, mut r: Rect) -> Rect {
        let a = self.area;
        r.w = r.w.clamp(MIN_W, a.w);
        r.h = r.h.clamp(MIN_H + TITLE, a.h);
        r.x = r.x.clamp(a.x - r.w + 80, a.right() - 80);
        r.y = r.y.clamp(a.y, a.bottom() - TITLE);
        r
    }

    /// A new window with content `w` by `h`, in front.
    pub fn add(&mut self, id: u32, w: i32, h: i32) -> Vec<Out> {
        let a = self.area;
        let step = 28;
        let x = a.x + 60 + self.next_cascade * step;
        let y = a.y + 30 + self.next_cascade * step;
        self.next_cascade = (self.next_cascade + 1) % 8;
        let frame = self.fit(Rect::new(x, y, w, h + TITLE));
        self.wins.push(Window {
            id,
            frame,
            place: Place::Free,
            free: frame,
            group: None,
        });
        self.tiles.push(id);
        let mut out = Vec::new();
        if self.tiling {
            self.retile(&mut out);
        } else {
            out.push(Self::damage(frame));
            let c = self.wins.last().unwrap().content();
            if (c.w, c.h) != (w, h) {
                out.push(Out::Resized(id, c.w, c.h));
            }
        }
        out.push(Out::Focus(id));
        out
    }

    pub fn remove(&mut self, id: u32) -> Vec<Out> {
        let mut out = Vec::new();
        if let Some(i) = self.index(id) {
            let w = self.wins.remove(i);
            self.tiles.retain(|&t| t != id);
            out.push(Self::damage(w.frame));
            if matches!(self.grab, Some(Grab::Move { id: g, .. } | Grab::Resize { id: g, .. } | Grab::App(g)) if g == id)
            {
                self.grab = None;
                self.preview = None;
            }
            if self.tiling {
                self.retile(&mut out);
            }
            if let Some(f) = self.focused() {
                out.push(Out::Focus(f));
            }
        }
        out
    }

    /// Bring a window to the front.
    pub fn raise(&mut self, id: u32, out: &mut Vec<Out>) {
        if let Some(i) = self.index(id)
            && i + 1 != self.wins.len()
        {
            let w = self.wins.remove(i);
            out.push(Self::damage(w.frame));
            self.wins.push(w);
            out.push(Out::Focus(id));
        }
    }

    /// What is at (`x`, `y`), front first.
    pub fn hit(&self, x: i32, y: i32) -> Hit {
        if y < self.area.y {
            return Hit::Strip;
        }
        for w in self.wins.iter().rev() {
            let f = w.frame;
            if f.contains(x, y) {
                if w.close_button().contains(x, y) {
                    return Hit::Close(w.id);
                }
                if y < f.y + TITLE {
                    // The top edge of a free window still resizes.
                    if !self.tiling && y < f.y + 3 && w.place == Place::Free {
                        return Hit::Edge(w.id, edge::TOP | edges_x(f, x, 3));
                    }
                    return Hit::Title(w.id);
                }
                let e = if self.tiling {
                    0
                } else {
                    edges_x(f, x, 3) | if y >= f.bottom() - 3 { edge::BOTTOM } else { 0 }
                };
                if e != 0 {
                    return Hit::Edge(w.id, e);
                }
                return Hit::Content(w.id, x - f.x, y - f.y - TITLE);
            }
            if !self.tiling && f.grow(EDGE).contains(x, y) {
                let mut e = edges_x(f.grow(EDGE), x, EDGE);
                if y >= f.bottom() {
                    e |= edge::BOTTOM;
                } else if y < f.y {
                    e |= edge::TOP;
                }
                if e != 0 {
                    return Hit::Edge(w.id, e);
                }
            }
        }
        Hit::Desktop
    }

    /// Where a window dragged with the mouse at (`x`, `y`) would snap.
    fn snap_target(&self, x: i32, y: i32) -> Option<(Place, Rect)> {
        let a = self.area;
        if y <= a.y {
            Some((Place::Full, a))
        } else if x <= a.x + 1 {
            Some((Place::Left, Rect::new(a.x, a.y, a.w / 2, a.h)))
        } else if x >= a.right() - 2 {
            Some((
                Place::Right,
                Rect::new(a.x + a.w / 2, a.y, a.w - a.w / 2, a.h),
            ))
        } else {
            None
        }
    }

    fn set_frame(&mut self, i: usize, r: Rect, out: &mut Vec<Out>) {
        let old = self.wins[i].frame;
        if old == r {
            return;
        }
        self.wins[i].frame = r;
        out.push(Self::damage(old));
        out.push(Self::damage(r));
        if (old.w, old.h) != (r.w, r.h) {
            let c = self.wins[i].content();
            out.push(Out::Resized(self.wins[i].id, c.w, c.h));
        }
    }

    /// Put window `id` in a place (also used by keyboard shortcuts).
    pub fn place(&mut self, id: u32, place: Place) -> Vec<Out> {
        let mut out = Vec::new();
        let Some(i) = self.index(id) else {
            return out;
        };
        if self.tiling {
            return out;
        }
        let a = self.area;
        if self.wins[i].place == Place::Free {
            self.wins[i].free = self.wins[i].frame;
        }
        let r = match place {
            Place::Full => a,
            Place::Left => Rect::new(a.x, a.y, a.w / 2, a.h),
            Place::Right => Rect::new(a.x + a.w / 2, a.y, a.w - a.w / 2, a.h),
            Place::Free | Place::Tiled => self.wins[i].free,
        };
        self.wins[i].place = if place == Place::Tiled {
            Place::Free
        } else {
            place
        };
        self.set_frame(i, r, &mut out);
        out
    }

    /// Switch the screen between floating and tiled windows.
    pub fn set_tiling(&mut self, on: bool) -> Vec<Out> {
        let mut out = Vec::new();
        if on == self.tiling {
            return out;
        }
        self.grab = None;
        self.preview = None;
        self.tiling = on;
        if on {
            for w in &mut self.wins {
                if w.place == Place::Free {
                    w.free = w.frame;
                }
            }
            self.retile(&mut out);
        } else {
            for i in 0..self.wins.len() {
                let r = self.wins[i].free;
                self.wins[i].place = Place::Free;
                self.set_frame(i, r, &mut out);
            }
        }
        out
    }

    /// Share the area between all windows, side by side, without overlap:
    /// up to three in columns, more in a grid, in tile order.
    fn retile(&mut self, out: &mut Vec<Out>) {
        let n = self.tiles.len() as i32;
        if n == 0 {
            return;
        }
        let a = self.area;
        let cols = if n <= 3 { n } else { isqrt_ceil(n) };
        let rows = (n + cols - 1) / cols;
        let rh = (a.h - GAP * (rows + 1)) / rows;
        let order = self.tiles.clone();
        for (k, id) in order.into_iter().enumerate() {
            let k = k as i32;
            let (c, r) = (k % cols, k / cols);
            // The last row's windows share its width.
            let in_row = if r == rows - 1 { n - r * cols } else { cols };
            let w = (a.w - GAP * (in_row + 1)) / in_row;
            let rect = Rect::new(a.x + GAP + c * (w + GAP), a.y + GAP + r * (rh + GAP), w, rh);
            if let Some(i) = self.index(id) {
                self.wins[i].place = Place::Tiled;
                self.set_frame(i, rect, out);
            }
        }
    }

    /// The mouse moved to (`x`, `y`).
    pub fn pointer_move(&mut self, x: i32, y: i32) -> Vec<Out> {
        let mut out = Vec::new();
        let old = self.pointer;
        self.pointer = (x, y);
        match self.grab {
            Some(Grab::Move {
                id,
                dx,
                dy,
                x0,
                y0,
                moved,
            }) => {
                let Some(i) = self.index(id) else {
                    return out;
                };
                let moved = moved || (x - x0).abs() + (y - y0).abs() > 4;
                if !moved {
                    return out;
                }
                let mut dx = dx;
                if self.wins[i].place != Place::Free && !self.tiling {
                    // Coming loose from a snap: back to its own size,
                    // still under the mouse at the same share of its width.
                    let f = self.wins[i].frame;
                    let free = self.wins[i].free;
                    dx = (dx * free.w / f.w.max(1)).clamp(0, free.w - 1);
                    self.wins[i].place = Place::Free;
                    self.set_frame(i, Rect::new(x - dx, y - dy, free.w, free.h), &mut out);
                }
                self.grab = Some(Grab::Move {
                    id,
                    dx,
                    dy,
                    x0,
                    y0,
                    moved: true,
                });
                let f = self.wins[i].frame;
                let r = self.fit(Rect::new(x - dx, y - dy, f.w, f.h));
                self.set_frame(i, r, &mut out);
                let preview = if self.tiling {
                    None
                } else {
                    self.snap_target(x, y).map(|(_, r)| r)
                };
                if preview != self.preview {
                    if let Some(p) = self.preview {
                        out.push(Self::damage(p));
                    }
                    if let Some(p) = preview {
                        out.push(Self::damage(p));
                    }
                    self.preview = preview;
                }
            }
            Some(Grab::Resize {
                id,
                edges,
                start,
                x0,
                y0,
            }) => {
                let Some(i) = self.index(id) else {
                    return out;
                };
                let (ddx, ddy) = (x - x0, y - y0);
                let mut r = start;
                if edges & edge::LEFT != 0 {
                    let nx = (start.x + ddx).min(start.right() - MIN_W);
                    r.w = start.right() - nx;
                    r.x = nx;
                }
                if edges & edge::RIGHT != 0 {
                    r.w = (start.w + ddx).max(MIN_W);
                }
                if edges & edge::TOP != 0 {
                    let ny = (start.y + ddy)
                        .min(start.bottom() - MIN_H - TITLE)
                        .max(self.area.y);
                    r.h = start.bottom() - ny;
                    r.y = ny;
                }
                if edges & edge::BOTTOM != 0 {
                    r.h = (start.h + ddy).max(MIN_H + TITLE);
                }
                self.wins[i].place = Place::Free;
                self.set_frame(i, r, &mut out);
                self.wins[i].free = self.wins[i].frame;
            }
            Some(Grab::App(id)) => {
                if let Some(w) = self.get(id) {
                    let c = w.content();
                    out.push(Out::Pointer {
                        id,
                        x: x - c.x,
                        y: y - c.y,
                        button: 0,
                        down: false,
                        moved: true,
                    });
                }
            }
            None => {
                if let Hit::Content(id, cx, cy) = self.hit(x, y) {
                    out.push(Out::Pointer {
                        id,
                        x: cx,
                        y: cy,
                        button: 0,
                        down: false,
                        moved: true,
                    });
                }
            }
        }
        // The pointer itself is drawn by the compositor; its old and new
        // places need drawing.
        out.push(Out::Damage(Rect::new(old.0 - 2, old.1 - 2, 24, 28)));
        out.push(Out::Damage(Rect::new(x - 2, y - 2, 24, 28)));
        out
    }

    /// A mouse button went down or up (1: left, 2: right).
    pub fn button(&mut self, button: u8, down: bool) -> Vec<Out> {
        let mut out = Vec::new();
        let (x, y) = self.pointer;
        if !down {
            match self.grab.take() {
                Some(Grab::Move { id, moved, .. }) => {
                    if let Some(p) = self.preview.take() {
                        out.push(Self::damage(p));
                    }
                    if !moved {
                        return out;
                    }
                    if self.tiling {
                        self.drop_tile(id, x, y, &mut out);
                    } else if let Some((place, _)) = self.snap_target(x, y) {
                        // `free` keeps the size from before the drag.
                        let keep = self.get(id).map(|w| w.free);
                        out.extend(self.place(id, place));
                        if let (Some(i), Some(k)) = (self.index(id), keep) {
                            self.wins[i].free = Rect::new(k.x, k.y.max(self.area.y), k.w, k.h);
                        }
                    } else if let Some(i) = self.index(id) {
                        self.wins[i].free = self.wins[i].frame;
                    }
                }
                Some(Grab::App(id)) => {
                    if let Some(w) = self.get(id) {
                        let c = w.content();
                        out.push(Out::Pointer {
                            id,
                            x: x - c.x,
                            y: y - c.y,
                            button,
                            down: false,
                            moved: false,
                        });
                    }
                }
                _ => {}
            }
            return out;
        }
        if self.grab.is_some() {
            return out;
        }
        match self.hit(x, y) {
            Hit::Title(id) => {
                self.raise(id, &mut out);
                let f = self.get(id).unwrap().frame;
                if button == 1 {
                    self.grab = Some(Grab::Move {
                        id,
                        dx: x - f.x,
                        dy: y - f.y,
                        x0: x,
                        y0: y,
                        moved: false,
                    });
                }
            }
            Hit::Close(id) => {
                self.raise(id, &mut out);
                if button == 1 {
                    out.push(Out::Close(id));
                }
            }
            Hit::Edge(id, edges) => {
                self.raise(id, &mut out);
                let f = self.get(id).unwrap().frame;
                if button == 1 {
                    self.grab = Some(Grab::Resize {
                        id,
                        edges,
                        start: f,
                        x0: x,
                        y0: y,
                    });
                }
            }
            Hit::Content(id, cx, cy) => {
                self.raise(id, &mut out);
                self.grab = Some(Grab::App(id));
                out.push(Out::Pointer {
                    id,
                    x: cx,
                    y: cy,
                    button,
                    down: true,
                    moved: false,
                });
            }
            Hit::Desktop | Hit::Strip => {}
        }
        out
    }

    /// In tiling mode, a window dropped onto another swaps tiles with it.
    fn drop_tile(&mut self, id: u32, x: i32, y: i32, out: &mut Vec<Out>) {
        let target = self
            .wins
            .iter()
            .rev()
            .find(|w| w.id != id && w.frame.contains(x, y))
            .map(|w| w.id);
        if let Some(t) = target {
            let a = self.tiles.iter().position(|&w| w == id);
            let b = self.tiles.iter().position(|&w| w == t);
            if let (Some(a), Some(b)) = (a, b) {
                self.tiles.swap(a, b);
            }
        }
        self.retile(out);
    }

    /// Put windows in a loose group (a label: it changes no permissions).
    pub fn set_group(&mut self, id: u32, group: Option<u32>) {
        if let Some(i) = self.index(id) {
            self.wins[i].group = group;
        }
    }
}

fn edges_x(f: Rect, x: i32, margin: i32) -> u8 {
    if x < f.x + margin {
        edge::LEFT
    } else if x >= f.right() - margin {
        edge::RIGHT
    } else {
        0
    }
}

fn isqrt_ceil(n: i32) -> i32 {
    let mut c = 1;
    while c * c < n {
        c += 1;
    }
    c
}
