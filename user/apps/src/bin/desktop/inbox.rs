// SPDX-License-Identifier: GPL-3.0-only
//! The decision inbox (milestone 7a, step 3; `continuity.md` section 5,
//! replacing notifications that belong to tasks, B5).
//!
//! Things waiting for the person, in four kinds:
//!
//! - **Now**: shows at once, always, also over a full-screen game. Only
//!   Alaya itself (`dk::NOW`, a channel no app can use) and the person's
//!   own rules ("always let this app through") make something "now".
//! - **About what you're doing**: from an app in the group the person is
//!   in, or in front: shows at once, quietly (not over full screen).
//! - **Can wait**: held in the inbox, with a count in the strip.
//! - **Just so you know**: in the inbox, never shown at arrival.
//!
//! An app's own label is never trusted: an app asking for "now" gets
//! "about what you're doing" or "can wait", unless the person's rule
//! says otherwise. No AI decides what matters, and "show everything" is
//! the inbox itself, one key away (Alaya key + I).

use alloc::string::String;
use alloc::vec::Vec;

use alaya_apps::store as st;
use alaya_programs::desktop::{self as dk, inbox};
use alaya_programs::files::{self as fs, APPS};
use alaya_sys::say;
use alaya_ui::widgets::Key;
use alaya_ui::wm::Place;

use super::{Desk, JobView, capital};

/// The inbox keeps at most this many things.
const KEEP: usize = 64;

/// One-time hints: each shows once, the first time it helps, and never
/// again (it is kept with the person's app files).
pub(crate) const HINT_UNDO: u64 = 1;
const HINT_HELD: u64 = 2;

/// One thing waiting for the person.
#[derive(Clone, Debug)]
pub(crate) struct Item {
    pub kind: u64,
    /// The app's role (0: Alaya itself).
    pub from: u64,
    pub text: String,
    /// It showed at once, when it came.
    pub shown: bool,
}

fn kind_words(k: u64) -> &'static str {
    match k {
        inbox::NOW => "Now",
        inbox::ABOUT => "About what you're doing",
        inbox::WAIT => "Can wait",
        _ => "Just so you know",
    }
}

/// 32 bytes of text from four message words.
pub(crate) fn text_of(len: u64, w: &[u64]) -> String {
    let mut b = [0u8; 32];
    for (k, word) in w.iter().take(4).enumerate() {
        b[k * 8..k * 8 + 8].copy_from_slice(&word.to_le_bytes());
    }
    let n = (len as usize).min(32);
    String::from_utf8_lossy(&b[..n]).into_owned()
}

impl Desk {
    /// Something for the inbox: from Alaya (`from` 0, its kind trusted) or
    /// an app (its kind only a wish).
    pub(crate) fn post(&mut self, from: u64, wanted: u64, text: String) -> u64 {
        if self.mine().is_none() {
            return dk::BAD_REQUEST;
        }
        let kind = if from == 0 {
            wanted.min(inbox::KNOW)
        } else {
            match wanted {
                inbox::NOW if self.rules.contains(&from) => inbox::NOW,
                inbox::NOW | inbox::ABOUT => {
                    if self.about_now(from) {
                        inbox::ABOUT
                    } else {
                        inbox::WAIT
                    }
                }
                inbox::WAIT => inbox::WAIT,
                _ => inbox::KNOW,
            }
        };
        let full = self
            .wm
            .focused()
            .and_then(|w| self.wm.get(w))
            .is_some_and(|w| w.place == Place::Full);
        // In the Play look, and over a full-screen game, only "now" shows.
        let quiet = full || self.look == super::Look::Play;
        let shown = kind == inbox::NOW || (kind == inbox::ABOUT && !quiet);
        let who = if from == 0 {
            "Alaya"
        } else {
            st::app_name(fs::app_id(from))
        };
        say!(
            "inbox: {} from {who}: {text}{}",
            kind_words(kind),
            if shown { " (shown at once)" } else { " (held)" }
        );
        if !shown && kind != inbox::KNOW {
            self.hint_once(
                HINT_HELD,
                "Held for later, not lost: Alaya key + I shows it",
            );
        }
        if shown {
            self.say_note(if from == 0 {
                text.clone()
            } else {
                alloc::format!("{}: {text}", capital(who))
            });
            if kind == inbox::NOW {
                self.now_shown += 1;
            }
        }
        if self.inbox.len() >= KEEP {
            // Room: the oldest "just so you know" goes first, then the
            // oldest of what can wait; "now" is never dropped.
            let i = self
                .inbox
                .iter()
                .position(|x| x.kind == inbox::KNOW)
                .or_else(|| self.inbox.iter().position(|x| x.kind == inbox::WAIT));
            match i {
                Some(i) => {
                    self.inbox.remove(i);
                }
                None => return dk::NO_MEMORY,
            }
        }
        self.inbox.push(Item {
            kind,
            from,
            text,
            shown,
        });
        self.damage_all();
        dk::OK
    }

    /// The app is part of what the person is doing: in front, or in the
    /// group they are in.
    fn about_now(&self, r: u64) -> bool {
        let front = self
            .wm
            .focused()
            .and_then(|id| self.wins.iter().find(|w| w.id == id))
            .is_some_and(|w| w.owner == r);
        let in_group = self
            .current_group
            .is_some_and(|g| self.groups_of.iter().any(|x| x.0 == r && x.1 == g));
        front || in_group
    }

    /// For INBOX: shown-at-once "now" count, then each kind's count.
    pub(crate) fn inbox_counts(&self) -> [u64; 6] {
        let n = |k| self.inbox.iter().filter(|x| x.kind == k).count() as u64;
        [
            dk::OK,
            self.now_shown,
            n(inbox::NOW),
            n(inbox::ABOUT),
            n(inbox::WAIT),
            n(inbox::KNOW),
        ]
    }

    /// What the strip says when nothing else: how much is held.
    pub(crate) fn held_words(&self) -> Option<String> {
        let held = self.inbox.iter().filter(|x| !x.shown).count();
        (held > 0).then(|| {
            alloc::format!(
                "{held} thing{} waiting for you · Alaya key + I",
                if held == 1 { "" } else { "s" }
            )
        })
    }

    /// The inbox, sorted: now, about, can wait, just so you know.
    fn sorted(&self) -> Vec<usize> {
        let mut ix: Vec<usize> = (0..self.inbox.len()).collect();
        ix.sort_by_key(|&i| self.inbox[i].kind);
        ix
    }

    pub(crate) fn inbox_open(&mut self) {
        if self.mine().is_none() || self.gscreen.is_some() || self.job.is_some() {
            return;
        }
        self.inbox_at = Some(0);
        say!("the inbox: {} things", self.inbox.len());
        self.damage_all();
    }

    pub(crate) fn inbox_key(&mut self, k: Key) {
        let Some(at) = self.inbox_at else { return };
        let order = self.sorted();
        let n = order.len();
        match k {
            Key::Up => self.inbox_at = Some(at.saturating_sub(1)),
            Key::Down => self.inbox_at = Some((at + 1).min(n.saturating_sub(1))),
            Key::Escape => self.inbox_at = None,
            Key::Delete | Key::Backspace => {
                if let Some(&i) = order.get(at) {
                    self.inbox.remove(i);
                    self.inbox_at = Some(at.min(n.saturating_sub(2)));
                }
            }
            Key::Enter => {
                if let Some(&i) = order.get(at) {
                    let it = self.inbox.remove(i);
                    self.inbox_at = None;
                    if it.from != 0 {
                        self.show_app(it.from, st::app_name(fs::app_id(it.from)));
                    }
                }
            }
            Key::Char('a' | 'A') => {
                if let Some(&i) = order.get(at) {
                    let r = self.inbox[i].from;
                    if r != 0 && !self.rules.contains(&r) {
                        self.rules.push(r);
                        self.save_rules();
                        let name = st::app_name(fs::app_id(r));
                        say!("your rule: always let {name} through");
                        self.say_note(alloc::format!(
                            "Your rule: {name} can always reach you at once"
                        ));
                    }
                }
            }
            _ => {}
        }
        self.damage_all();
    }

    /// A one-time hint: shown the first time only.
    pub(crate) fn hint_once(&mut self, bit: u64, text: &str) {
        if self.hints & bit != 0 || self.mine().is_none() {
            return;
        }
        self.hints |= bit;
        say!("hint, once: {text}");
        self.say_note(String::from(text));
        let Some(s) = self.mine() else { return };
        if let Ok(a) = s.root(APPS)
            && let Ok(f) = s.open(a, "hints")
        {
            let _ = s.write_all(f, &self.hints.to_le_bytes());
        }
    }

    /// The person's rules (and which hints they have seen) live with
    /// their app files.
    pub(crate) fn load_rules(&mut self) {
        self.rules.clear();
        self.hints = 0;
        if let Some(s) = self.mine()
            && let Ok(b) = s
                .root(APPS)
                .and_then(|a| s.lookup(a, "hints"))
                .and_then(|f| s.read_all(f))
        {
            let mut w = [0u8; 8];
            let n = b.len().min(8);
            w[..n].copy_from_slice(&b[..n]);
            self.hints = u64::from_le_bytes(w);
        }
        let Some(s) = self.mine() else { return };
        if let Ok(b) = s
            .root(APPS)
            .and_then(|a| s.lookup(a, "inbox-rules"))
            .and_then(|f| s.read_all(f))
        {
            self.rules = b.iter().map(|&r| u64::from(r)).collect();
        }
    }

    fn save_rules(&mut self) {
        let Some(s) = self.mine() else { return };
        let b: Vec<u8> = self.rules.iter().map(|&r| r as u8).collect();
        if let Ok(a) = s.root(APPS)
            && let Ok(f) = s.open(a, "inbox-rules")
        {
            let _ = s.write_all(f, &b);
        }
    }

    pub(crate) fn inbox_view(&self, at: usize) -> JobView {
        let order = self.sorted();
        let held = self.inbox.iter().filter(|x| !x.shown).count();
        JobView {
            title: String::from("Waiting for you"),
            hint: String::from(
                "Everything, sorted: now, about what you're doing, can wait, just so you know. \
                 Only Alaya and your own rules make something \u{201c}now\u{201d}.",
            ),
            rows: order
                .iter()
                .map(|&i| {
                    let x = &self.inbox[i];
                    let who = if x.from == 0 {
                        "Alaya"
                    } else {
                        st::app_name(fs::app_id(x.from))
                    };
                    alloc::format!("{}   ·   {}   ·   {}", kind_words(x.kind), x.text, who)
                })
                .collect(),
            mark: !order.is_empty() && at < order.len(),
            foot: alloc::format!(
                "{held} held back for now. Enter: open its app · Delete: done with it · \
                 A: always let this app through"
            ),
            ok: "Open",
            no: "Close",
            field: None,
            empty: "Nothing is waiting for you.",
        }
    }
}
