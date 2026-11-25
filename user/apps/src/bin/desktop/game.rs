// SPDX-License-Identifier: GPL-3.0-only
//! Game mode (decision 63; milestone 8, step 3): while a full-screen
//! window is in front, nothing in the background competes with it.
//!
//! - Installed apps running with no window open are frozen (their
//!   threads stop where they are) and go on when the game ends.
//! - The store puts off its backups by itself (at most two hours, so a
//!   backup is never put off for good).
//! - Install and update offers wait until the game ends: nothing about
//!   apps changes under a game, and no sheet covers it.
//! - Notifications were already held over full screen (milestone 7a);
//!   only "now" shows.
//!
//! Alaya decides by what is on the screen, not by asking the app: an app
//! cannot switch game mode on to starve others, because only the
//! person's own full-screen window in front turns it on.

use alaya_programs::desktop::{self as dk, role};
use alaya_sys::say;
use alaya_ui::wm::Place;

use super::Desk;

impl Desk {
    /// Is a full-screen window in front?
    fn full_in_front(&self) -> bool {
        self.wm
            .focused()
            .and_then(|w| self.wm.get(w))
            .is_some_and(|w| w.place == Place::Full)
    }

    /// Called after every event: turn game mode on or off when what is
    /// in front changed.
    pub(crate) fn game_check(&mut self) {
        let on = self.mine().is_some() && self.full_in_front();
        if on == self.game {
            return;
        }
        self.game = on;
        if on {
            // Installed apps with no window: frozen for the game.
            let mut mask = 0;
            for r in role::INSTALLED..=role::LAST {
                if self.windowless & 1 << r != 0 {
                    self.freeze(r, true);
                    mask |= 1 << r;
                }
            }
            self.game_frozen = mask;
            let quiet = self.mine().is_some_and(|s| s.quiet(true).is_ok());
            say!(
                "game mode on: {} background app(s) frozen, backups put off{}",
                mask.count_ones(),
                if quiet {
                    ""
                } else {
                    " (the store did not answer)"
                }
            );
        } else {
            for r in role::INSTALLED..=role::LAST {
                if self.game_frozen & 1 << r != 0 {
                    self.freeze(r, false);
                }
            }
            let n = self.game_frozen.count_ones();
            self.game_frozen = 0;
            if let Some(s) = self.mine() {
                let _ = s.quiet(false);
            }
            let held = core::mem::take(&mut self.held_offers);
            say!(
                "game mode off: {n} app(s) go on, {} offer(s) now",
                held.len()
            );
            for f in held {
                self.offer_install(f);
            }
        }
    }

    /// For GAME: on, the roles frozen for it (bit r), offers waiting.
    pub(crate) fn game_words(&self) -> [u64; 6] {
        [
            dk::OK,
            self.game as u64,
            self.game_frozen,
            self.held_offers.len() as u64,
            0,
            0,
        ]
    }
}
