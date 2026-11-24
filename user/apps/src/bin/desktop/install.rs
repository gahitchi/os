// SPDX-License-Identifier: GPL-3.0-only
//! Installing apps (milestone 8, step 1; decisions 146 and 147).
//!
//! An app comes as a signed bundle (`alaya-bundle`). Offered one (the
//! file browser's Enter on it, or `dk::OFFER_INSTALL`), the desktop checks
//! its signature and shows its promise, one line each, every line can be
//! unticked (decision 60). Enter installs it **on the side**: the bundle
//! is kept in the machine's app files, where no app can reach, and
//! nothing else changes. Approve puts it in the dock, and only then may it
//! run; it goes on the undo list, so one undo takes it away again.
//!
//! Removing an app ends with a receipt from the store's own records: the
//! hand-overs it had (taken back, then checked again: none left), the
//! files it made (yours, kept), its running copy (ended).

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use alaya_apps::store::label;
use alaya_bundle::{Bundle, promise};
use alaya_programs::desktop::{self as dk, installed as ins, role};
use alaya_programs::files::{self as fs, APPS, act};
use alaya_sys::say;
use alaya_ui::widgets::Key;

use super::{Desk, JobView};

/// One installed place of the person logged in.
#[derive(Clone, Default)]
pub(crate) struct Place {
    pub state: u8,
    pub ticked: u32,
    pub version: u32,
    pub id: u64,
    pub name: String,
}

pub(crate) enum Stage {
    /// The promise, line by line, to tick or untick.
    Review,
    /// Kept on the side in this place, waiting for Approve.
    Aside(u64),
    /// Already installed in this place: keep it, or remove it.
    Installed(u64),
    /// Refused (why), or a removal's receipt: lines to read.
    Read(Vec<String>),
}

pub(crate) struct Install {
    bytes: Vec<u8>,
    name: String,
    version: u32,
    promise: u32,
    id: u64,
    maker: String,
    ticked: u32,
    pub chosen: usize,
    pub stage: Stage,
    title: String,
}

/// The promise lines this bundle has, in order.
fn lines(promise: u32) -> Vec<(u32, &'static str)> {
    promise::LINES
        .iter()
        .copied()
        .filter(|(bit, _)| promise & bit != 0)
        .collect()
}

/// A colour for an installed app's dock button, from its number.
fn color_of(id: u64) -> u32 {
    const COLORS: [u32; 4] = [0x6a_3d_9a, 0x1f_78_5a, 0xa3_3b_3b, 0x3b_5b_a3];
    COLORS[(id % 4) as usize]
}

impl Desk {
    fn person_id(&self) -> Option<u64> {
        self.session.map(|i| self.people[i].id)
    }

    /// The machine's record of all installed places.
    fn records(&self) -> Vec<u8> {
        let mut b = self
            .machine
            .root(APPS)
            .and_then(|a| self.machine.lookup(a, ins::FILE))
            .and_then(|f| self.machine.read_all(f))
            .unwrap_or_default();
        b.resize(ins::BYTES, 0);
        b
    }

    fn write_record(&mut self, p: u64, slot: u64, place: &Place) -> Result<(), u64> {
        let mut b = self.records();
        let at = ins::at(p, slot);
        let r = &mut b[at..at + ins::RECORD];
        r.fill(0);
        r[0] = place.state;
        r[4..8].copy_from_slice(&place.ticked.to_le_bytes());
        r[8..12].copy_from_slice(&place.version.to_le_bytes());
        r[16..24].copy_from_slice(&place.id.to_le_bytes());
        let n = place.name.len().min(32);
        r[24..24 + n].copy_from_slice(&place.name.as_bytes()[..n]);
        let m = &self.machine;
        let apps = m.root(APPS)?;
        let f = m.open(apps, ins::FILE)?;
        m.write_all(f, &b)?;
        m.save()?;
        self.installed[slot as usize] = place.clone();
        Ok(())
    }

    /// Read the person's installed places (at login and after changes).
    pub(crate) fn load_installed(&mut self) {
        let Some(p) = self.person_id() else { return };
        let b = self.records();
        for slot in 0..role::INSTALLED_MAX {
            let at = ins::at(p, slot);
            let r = &b[at..at + ins::RECORD];
            let n = r[24..56].iter().position(|&c| c == 0).unwrap_or(32);
            self.installed[slot as usize] = Place {
                state: r[0],
                ticked: u32::from_le_bytes(r[4..8].try_into().unwrap()),
                version: u32::from_le_bytes(r[8..12].try_into().unwrap()),
                id: u64::from_le_bytes(r[16..24].try_into().unwrap()),
                name: String::from_utf8_lossy(&r[24..24 + n]).into_owned(),
            };
        }
    }

    /// The dock buttons of approved apps.
    pub(crate) fn installed_dock(&self) -> Vec<(String, u32, u64)> {
        self.installed
            .iter()
            .enumerate()
            .filter(|(_, p)| p.state == ins::ON)
            .map(|(i, p)| (p.name.clone(), color_of(p.id), role::INSTALLED + i as u64))
            .collect()
    }

    /// The name of an installed app's role, if it is one.
    pub(crate) fn installed_name(&self, r: u64) -> Option<&str> {
        role::installed(r)
            .then(|| &self.installed[(r - role::INSTALLED) as usize])
            .filter(|p| p.state != ins::EMPTY)
            .map(|p| p.name.as_str())
    }

    /// An app bundle is offered (a file of the person's): check it and
    /// show its promise.
    pub(crate) fn offer_install(&mut self, f: u64) -> u64 {
        let Some(s) = self.mine() else {
            return dk::BAD_REQUEST;
        };
        let Ok(bytes) = s.read_all(f) else {
            return dk::BAD_REQUEST;
        };
        let refused = |why: &str| Install {
            bytes: Vec::new(),
            name: String::new(),
            version: 0,
            promise: 0,
            id: 0,
            maker: String::new(),
            ticked: 0,
            chosen: 0,
            stage: Stage::Read(alloc::vec![
                format!("Not installed: {why}."),
                String::from("Nothing was changed."),
            ]),
            title: String::from("This app was refused"),
        };
        let x = match Bundle::read(&bytes) {
            Ok(b) => {
                let stage = match self
                    .installed
                    .iter()
                    .position(|p| p.id == b.id() && p.state != ins::EMPTY)
                {
                    Some(i) if self.installed[i].state == ins::ON => Stage::Installed(i as u64),
                    Some(i) => Stage::Aside(i as u64),
                    None => Stage::Review,
                };
                let maker = String::from_utf8_lossy(&b.maker_code()).into_owned();
                Install {
                    name: String::from(b.name()),
                    version: b.version,
                    promise: b.promise,
                    id: b.id(),
                    maker,
                    ticked: b.promise,
                    chosen: 0,
                    stage,
                    title: format!("Install {}?", b.name()),
                    bytes: Vec::new(),
                }
            }
            Err(e) => {
                say!("install: refused ({e:?})");
                refused(e.words())
            }
        };
        let x = if matches!(x.stage, Stage::Review) {
            Install { bytes, ..x }
        } else {
            x
        };
        say!("install: offered {} version {}", x.name, x.version);
        self.install = Some(x);
        self.damage_all();
        dk::OK
    }

    /// The install sheet, for the picker.
    pub(crate) fn install_view(&self, x: &Install) -> JobView {
        let mut v = JobView {
            title: x.title.clone(),
            hint: format!(
                "{} version {} from the maker {}. Its signature checks out.",
                x.name, x.version, x.maker
            ),
            rows: Vec::new(),
            mark: true,
            foot: String::new(),
            ok: "Install on the side",
            no: "Cancel",
            field: None,
            empty: "It asks for nothing.",
        };
        match &x.stage {
            Stage::Review => {
                v.rows = lines(x.promise)
                    .iter()
                    .map(|(bit, words)| {
                        format!(
                            "{}   {words}",
                            if x.ticked & bit != 0 { "On " } else { "Off" }
                        )
                    })
                    .collect();
                v.foot = String::from(
                    "Space: tick or untick the line · Enter: install it on the side (nothing \
                     changes until you approve)",
                );
            }
            Stage::Aside(_) => {
                v.title = format!("{} is installed on the side", x.name);
                v.rows = alloc::vec![
                    String::from("Nothing has changed yet: it is not in your dock and cannot run."),
                    format!("It may: {}", self.may_words(x.ticked & x.promise)),
                ];
                v.mark = false;
                v.ok = "Approve";
                v.no = "Remove";
                v.foot = String::from("Enter: approve, and it joins your dock · Escape: remove it");
            }
            Stage::Installed(_) => {
                v.title = format!("{} is installed", x.name);
                v.rows = alloc::vec![format!("It may: {}", self.may_words(x.ticked & x.promise))];
                v.mark = false;
                v.ok = "Keep";
                v.no = "Remove";
                v.foot = String::from("Enter: keep it · Delete: remove it, with a receipt");
            }
            Stage::Read(l) => {
                v.hint = String::new();
                v.rows = l.clone();
                v.mark = false;
                v.ok = "Close";
                v.no = "Close";
                v.foot = String::new();
            }
        }
        v
    }

    fn may_words(&self, granted: u32) -> String {
        let l: Vec<&str> = lines(granted).iter().map(|(_, w)| *w).collect();
        if l.is_empty() {
            String::from("nothing beyond its own window")
        } else {
            l.join("; ")
        }
    }

    pub(crate) fn install_key(&mut self, k: Key) {
        let Some(x) = &mut self.install else { return };
        match (&x.stage, k) {
            (Stage::Review, Key::Up) => x.chosen = x.chosen.saturating_sub(1),
            (Stage::Review, Key::Down) => {
                x.chosen = (x.chosen + 1).min(lines(x.promise).len().saturating_sub(1));
            }
            (Stage::Review, Key::Char(' ')) => {
                if let Some((bit, _)) = lines(x.promise).get(x.chosen) {
                    x.ticked ^= bit;
                }
            }
            (Stage::Review, Key::Enter) => self.install_aside(),
            (Stage::Aside(slot), Key::Enter) => {
                let slot = *slot;
                self.install_approve(slot);
            }
            (Stage::Aside(slot), Key::Escape) => {
                let slot = *slot;
                self.remove_app(slot, false);
            }
            (Stage::Installed(slot), Key::Delete | Key::Backspace) => {
                let slot = *slot;
                self.remove_app(slot, true);
            }
            (_, Key::Enter | Key::Escape) => self.install = None,
            _ => {}
        }
        self.damage_all();
    }

    /// Enter on the promise: keep the bundle on the side.
    fn install_aside(&mut self) {
        let Some(p) = self.person_id() else { return };
        let Some(x) = self.install.take() else { return };
        let Some(slot) = self.installed.iter().position(|q| q.state == ins::EMPTY) else {
            self.install = Some(Install {
                stage: Stage::Read(alloc::vec![
                    format!(
                        "There is room for {} installed apps; remove one first.",
                        role::INSTALLED_MAX
                    ),
                    String::from("Nothing was changed."),
                ]),
                ..x
            });
            return;
        };
        let slot = slot as u64;
        let mut nb = [0u8; 8];
        let name = ins::bundle_file(p, slot, &mut nb);
        let m = &self.machine;
        let kept = m
            .root(APPS)
            .and_then(|a| m.open(a, name))
            .and_then(|f| m.write_all(f, &x.bytes));
        let place = Place {
            state: ins::ASIDE,
            ticked: x.ticked & x.promise,
            version: x.version,
            id: x.id,
            name: x.name.clone(),
        };
        if kept.is_err() || self.write_record(p, slot, &place).is_err() {
            say!("install: {} could not be kept", x.name);
            self.install = Some(Install {
                stage: Stage::Read(alloc::vec![String::from(
                    "Not installed: the disk did not take it."
                )]),
                ..x
            });
            return;
        }
        say!(
            "install: {} kept on the side in place {slot}, may {:#x} of {:#x}",
            x.name,
            place.ticked,
            x.promise
        );
        self.install = Some(Install {
            stage: Stage::Aside(slot),
            bytes: Vec::new(),
            ..x
        });
    }

    fn install_approve(&mut self, slot: u64) {
        let Some(p) = self.person_id() else { return };
        let mut place = self.installed[slot as usize].clone();
        place.state = ins::ON;
        if self.write_record(p, slot, &place).is_err() {
            return;
        }
        self.record(
            act::SETTINGS,
            &self.app_label(label::INSTALL, slot, &place.name),
        );
        say!("install: {} approved, in the dock", place.name);
        self.say_note(format!("{} is in your dock", place.name));
        self.install = None;
    }

    fn app_label(&self, what: u8, slot: u64, name: &str) -> Vec<u8> {
        let mut l = alloc::vec![what, slot as u8];
        l.extend_from_slice(name.as_bytes());
        l.truncate(fs::LABEL);
        l
    }

    /// Remove the app in `slot`: end it, take back what it was handed,
    /// and say so in a receipt (decision 147). `undoable`: an approved
    /// app's removal goes on the undo list (its bundle is kept for that);
    /// one only on the side is simply let go.
    fn remove_app(&mut self, slot: u64, undoable: bool) {
        let Some(p) = self.person_id() else { return };
        let r = role::INSTALLED + slot;
        let app = fs::app_id(r);
        let mut place = self.installed[slot as usize].clone();
        let name = place.name.clone();
        // Its running copy ends, its windows close.
        let running = self.wins.iter().any(|w| w.owner == r);
        self.forget(r);
        let Some(s) = self.mine() else { return };
        // What it was handed goes back.
        let grants = s.grants(app).unwrap_or_default();
        let mut taken = 0;
        for g in grants.iter().filter(|g| !g.own) {
            if s.revoke(app, g.file).unwrap_or(false) {
                taken += 1;
            }
        }
        // What it made stays: those files are the person's.
        // (Saved first, so its last changes are in the history.)
        let _ = s.save();
        let mut made: Vec<u64> = s
            .happenings(0, 64)
            .unwrap_or_default()
            .iter()
            .filter(|h| h.app == app)
            .map(|h| h.file)
            .collect();
        // And the documents' own stories: which app made each.
        if let Ok(docs) = s.root(fs::DOCS)
            && let Ok(list) = s.list(docs)
        {
            for e in list.iter().filter(|e| !e.is_dir) {
                if s.story(e.file, 1).is_ok_and(|st| st.made_by == app) {
                    made.push(e.file);
                }
            }
        }
        made.sort_unstable();
        made.dedup();
        place.state = ins::EMPTY;
        if self.write_record(p, slot, &place).is_err() {
            return;
        }
        if !undoable {
            let mut nb = [0u8; 8];
            let file = ins::bundle_file(p, slot, &mut nb);
            let m = &self.machine;
            if let Ok(a) = m.root(APPS) {
                let _ = m.remove(a, file);
                let _ = m.save();
            }
        } else {
            self.record(act::SETTINGS, &self.app_label(label::REMOVE, slot, &name));
        }
        // Checked again on the store, not taken on trust.
        let left = s
            .grants(app)
            .map(|g| g.iter().filter(|g| !g.own).count())
            .unwrap_or(usize::MAX);
        let mut receipt = alloc::vec![
            format!(
                "Its running copy: {}.",
                if running { "ended" } else { "was not running" }
            ),
            format!(
                "Files you handed it: {taken} taken back; checked again on the store, {left} left."
            ),
            format!("Files it made: {} (they are yours and stay).", made.len()),
            String::from("Its place in the dock: gone."),
        ];
        receipt.push(if left == 0 {
            String::from("Nothing of this app is left that could act or see your files.")
        } else {
            String::from("Something is still left: see Settings, Apps.")
        });
        if undoable {
            receipt.push(String::from(
                "Its bundle is kept so one undo can bring it back.",
            ));
        }
        say!(
            "remove: {name}: running {running}, {taken} hand-overs taken back, {left} left, {} files it made",
            made.len()
        );
        self.removed = Some((r, left as u64, taken, made.len() as u64));
        self.install = Some(Install {
            bytes: Vec::new(),
            name: name.clone(),
            version: place.version,
            promise: 0,
            id: place.id,
            maker: String::new(),
            ticked: 0,
            chosen: 0,
            stage: Stage::Read(receipt),
            title: format!("{name} is gone"),
        });
    }

    /// An undo (or redo) of installing or removing: the store kept the
    /// action; the desktop puts the place back.
    pub(crate) fn undo_install(&mut self, l: &[u8], on: bool) {
        let Some(p) = self.person_id() else { return };
        let slot = u64::from(l[1]);
        if slot >= role::INSTALLED_MAX {
            return;
        }
        let mut place = self.installed[slot as usize].clone();
        place.state = if on { ins::ON } else { ins::EMPTY };
        if !on {
            self.forget(role::INSTALLED + slot);
        }
        let _ = self.write_record(p, slot, &place);
        say!(
            "install: {} is {}",
            place.name,
            if on { "back in the dock" } else { "taken away" }
        );
    }

    /// `dk::INSTALLED` for the checks: a place's state, what it may, its
    /// version and number; and the last removal's receipt.
    pub(crate) fn installed_words(&self, slot: u64) -> [u64; 6] {
        let Some(place) = self.installed.get(slot as usize) else {
            return [dk::BAD_REQUEST, 0, 0, 0, 0, 0];
        };
        let (gone, left, taken, made) = self.removed.unwrap_or((0, 0, 0, 0));
        [
            dk::OK,
            u64::from(place.state) | u64::from(place.ticked) << 8 | u64::from(place.version) << 32,
            place.id,
            u64::from(self.install.as_ref().map_or(0u8, |x| match x.stage {
                Stage::Review => 1,
                Stage::Aside(_) => 2,
                Stage::Installed(_) => 3,
                Stage::Read(_) => 4,
            })),
            gone | left << 16,
            taken | made << 32,
        ]
    }
}
