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
//!
//! Updates (milestone 8, step 2): a later version of the same app (the
//! same maker and name) asking for nothing new is taken at once; one that
//! asks for more waits on a difference screen, with the new lines off
//! until the person ticks them (R11). The version before is kept, so one
//! undo swaps back. A new version is on trial for its first starts: Alaya
//! notes what it tries beyond what the person allowed (shown as what it
//! tried, never as proof that it is safe, R12), and the Experience
//! Guardian times its start: clearly slower, and it goes back to the
//! version before by itself, unless the maker marked it a security fix.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use alaya_apps::store::label;
use alaya_bundle::{Bundle, flags, promise};
use alaya_programs::desktop::{self as dk, installed as ins, role};
use alaya_programs::files::{self as fs, APPS, act};
use alaya_sys::say;
use alaya_ui::widgets::Key;

use super::{Desk, JobView};

/// One installed place of the person logged in.
#[derive(Clone, Default)]
pub(crate) struct Place {
    pub state: u8,
    /// What it may: its promise, less what the person unticked.
    pub ticked: u32,
    pub version: u32,
    pub promise: u32,
    pub id: u64,
    pub name: String,
    /// Starts left on trial (a new version).
    pub trial: u8,
    pub flags: u32,
    /// Its usual start time, from starts off trial (ms).
    pub usual_ms: u32,
    /// The version kept for undo.
    pub kept: Kept,
    /// What it tried beyond what it may, during its trial.
    pub tried: u32,
    /// The version the Experience Guardian went back from (0: none).
    pub went_back_from: u32,
    pub last_ms: u32,
}

/// The other version of a place, kept for undo.
#[derive(Clone, Copy, Default)]
pub(crate) struct Kept {
    pub ticked: u32,
    pub version: u32,
    pub promise: u32,
    pub flags: u32,
}

/// Clearly slower: more than twice the usual start time, and a second
/// more (so a busy moment is not taken for a slow version).
fn clearly_slower(ms: u32, usual: u32) -> bool {
    usual > 0 && ms > usual.saturating_mul(2).saturating_add(1000)
}

pub(crate) enum Stage {
    /// The promise, line by line, to tick or untick.
    Review,
    /// Kept on the side in this place, waiting for Approve.
    Aside(u64),
    /// Already installed in this place: keep it, or remove it.
    Installed(u64),
    /// A later version asks for more: the new lines, to tick or not.
    Update(u64),
    /// Running in a rehearsal in this place, on a side copy.
    Rehearsing(u64),
    /// What the rehearsal showed: lines to read, then back to Review.
    Rehearsed(Vec<String>),
    /// Refused (why), or a removal's receipt: lines to read.
    Read(Vec<String>),
}

pub(crate) struct Install {
    bytes: Vec<u8>,
    name: String,
    version: u32,
    promise: u32,
    flags: u32,
    /// What a later version asks for beyond the one installed.
    more: u32,
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
        let mut put = |at: usize, v: u32| r[at..at + 4].copy_from_slice(&v.to_le_bytes());
        put(4, place.ticked);
        put(8, place.version);
        put(12, place.promise);
        put(60, place.usual_ms);
        put(64, place.kept.ticked);
        put(68, place.kept.version);
        put(72, place.kept.promise);
        put(76, place.kept.flags);
        put(80, place.tried);
        put(84, place.went_back_from);
        put(88, place.last_ms);
        r[0] = place.state;
        r[56] = place.trial;
        r[57] = place.flags as u8;
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
            let get = |at: usize| u32::from_le_bytes(r[at..at + 4].try_into().unwrap());
            self.installed[slot as usize] = Place {
                state: r[0],
                ticked: get(4),
                version: get(8),
                promise: get(12),
                id: u64::from_le_bytes(r[16..24].try_into().unwrap()),
                name: String::from_utf8_lossy(&r[24..24 + n]).into_owned(),
                trial: r[56],
                flags: u32::from(r[57]),
                usual_ms: get(60),
                kept: Kept {
                    ticked: get(64),
                    version: get(68),
                    promise: get(72),
                    flags: get(76),
                },
                tried: get(80),
                went_back_from: get(84),
                last_ms: get(88),
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
            flags: 0,
            more: 0,
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
                let found = self
                    .installed
                    .iter()
                    .position(|p| p.id == b.id() && p.state != ins::EMPTY);
                let stage = match found {
                    Some(i)
                        if self.installed[i].state == ins::ON
                            && b.version > self.installed[i].version =>
                    {
                        Stage::Update(i as u64)
                    }
                    Some(i) if self.installed[i].state == ins::ON => Stage::Installed(i as u64),
                    Some(i) => Stage::Aside(i as u64),
                    None => Stage::Review,
                };
                let maker = String::from_utf8_lossy(&b.maker_code()).into_owned();
                let update = matches!(stage, Stage::Update(_));
                Install {
                    name: String::from(b.name()),
                    version: b.version,
                    promise: b.promise,
                    flags: b.flags,
                    more: found.map_or(0, |i| b.promise & !self.installed[i].promise),
                    id: b.id(),
                    maker,
                    // New lines of an update are off until ticked (R11).
                    ticked: if update { 0 } else { b.promise },
                    chosen: 0,
                    stage,
                    title: format!(
                        "{} {}?",
                        if update { "Update" } else { "Install" },
                        b.name()
                    ),
                    bytes: Vec::new(),
                }
            }
            Err(e) => {
                say!("install: refused ({e:?})");
                refused(e.words())
            }
        };
        let x = if matches!(x.stage, Stage::Review | Stage::Update(_)) {
            Install { bytes, ..x }
        } else {
            x
        };
        say!("install: offered {} version {}", x.name, x.version);
        // Asking for nothing new, it is never held back.
        if let Stage::Update(slot) = x.stage
            && x.more == 0
        {
            let granted = self.installed[slot as usize].ticked & x.promise;
            self.update(x, slot, granted);
        } else {
            self.install = Some(x);
        }
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
                     changes until you approve) · R: rehearse it first on a copy of your files",
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
            Stage::Update(slot) => {
                let place = &self.installed[*slot as usize];
                v.hint = format!(
                    "{} version {} from the maker {}. Its signature checks out. You have version {}; \
                     the new version also wants what is below, off unless you tick it.",
                    x.name, x.version, x.maker, place.version
                );
                v.rows = lines(x.more)
                    .iter()
                    .map(|(bit, words)| {
                        format!(
                            "{}   {words}",
                            if x.ticked & bit != 0 { "On " } else { "Off" }
                        )
                    })
                    .collect();
                v.ok = "Update";
                v.no = "Not now";
                v.foot = format!(
                    "Space: tick or untick the line · Enter: update · Escape: not now, keep version {}",
                    place.version
                );
            }
            Stage::Installed(slot) => {
                let place = &self.installed[*slot as usize];
                v.title = format!("{} is installed", x.name);
                v.rows = alloc::vec![format!("It may: {}", self.may_words(place.ticked))];
                if x.version < place.version {
                    v.rows.push(format!(
                        "This file is version {}, older than yours ({}): Alaya does not go back \
                         this way. Undo does.",
                        x.version, place.version
                    ));
                }
                v.rows.extend(trial_lines(place));
                v.mark = false;
                v.ok = "Keep";
                v.no = "Remove";
                v.foot = String::from("Enter: keep it · Delete: remove it, with a receipt");
            }
            Stage::Rehearsing(_) => {
                v.title = format!("Rehearsing {}", x.name);
                v.rows = alloc::vec![
                    String::from(
                        "It runs now on a side copy of your documents, with what you left ticked.",
                    ),
                    String::from(
                        "Nothing it does reaches your real files. Try it, then come back."
                    ),
                ];
                v.mark = false;
                v.ok = "End the rehearsal";
                v.no = "End the rehearsal";
                v.foot = String::from("Alaya key + R: end the rehearsal and see what it did");
            }
            Stage::Rehearsed(l) => {
                v.title = format!("What {} did in the rehearsal", x.name);
                v.hint = String::new();
                v.rows = l.clone();
                v.mark = false;
                v.ok = "Back to installing";
                v.no = "Cancel";
                v.foot = String::from("Enter: back to the install sheet · Escape: cancel");
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
            (Stage::Update(_), Key::Up) => x.chosen = x.chosen.saturating_sub(1),
            (Stage::Update(_), Key::Down) => {
                x.chosen = (x.chosen + 1).min(lines(x.more).len().saturating_sub(1));
            }
            (Stage::Update(_), Key::Char(' ')) => {
                if let Some((bit, _)) = lines(x.more).get(x.chosen) {
                    x.ticked ^= bit;
                }
            }
            (Stage::Update(slot), Key::Enter) => {
                let slot = *slot;
                let x = self.install.take().unwrap();
                let had = self.installed[slot as usize].ticked;
                let granted = (had & x.promise) | (x.ticked & x.more);
                self.update(x, slot, granted);
            }
            (Stage::Update(slot), Key::Escape) => {
                say!(
                    "install: not now; {} stays at version {}",
                    x.name,
                    self.installed[*slot as usize].version
                );
                self.install = None;
            }
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
            (Stage::Review, Key::Char('r' | 'R')) => self.rehearse(),
            (Stage::Rehearsing(slot), Key::Enter | Key::Escape) => {
                let slot = *slot;
                self.end_rehearsal(slot);
            }
            (Stage::Rehearsed(_), Key::Enter) => {
                x.stage = Stage::Review;
                x.chosen = 0;
            }
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
            promise: x.promise,
            flags: x.flags,
            id: x.id,
            name: x.name.clone(),
            ..Place::default()
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
        self.end_app(r);
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
            let (mut nb, mut nk) = ([0u8; 8], [0u8; 8]);
            let file = ins::bundle_file(p, slot, &mut nb);
            let kept = ins::kept_file(p, slot, &mut nk);
            let m = &self.machine;
            if let Ok(a) = m.root(APPS) {
                let _ = m.remove(a, file);
                let _ = m.remove(a, kept);
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
            flags: 0,
            more: 0,
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
            self.end_app(role::INSTALLED + slot);
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
                Stage::Update(_) => 5,
                Stage::Rehearsing(_) => 6,
                Stage::Rehearsed(_) => 7,
            })),
            gone | left << 16,
            taken | made << 32,
        ]
    }

    /// Whether the app of role `owner` may do what promise line `bit`
    /// covers: Alaya's own apps may; an installed one only when approved,
    /// its promise says so and the person left the line ticked. A refused
    /// ask during a trial is noted as what it tried.
    pub(crate) fn may(&mut self, owner: u64, bit: u32) -> bool {
        if !role::installed(owner) {
            return true;
        }
        let slot = owner - role::INSTALLED;
        let place = &self.installed[slot as usize];
        if (place.state == ins::ON || place.state == ins::REHEARSAL) && place.ticked & bit != 0 {
            return true;
        }
        say!("{} asked for what it may not ({bit:#x})", place.name);
        // Only the version on trial counts, not an older copy still
        // running.
        if place.trial > 0
            && self.running_version[slot as usize] == place.version
            && let Some(p) = self.person_id()
        {
            let mut place = place.clone();
            place.tried += 1;
            let _ = self.write_record(p, slot, &place);
        }
        false
    }

    /// Take version `x` for `slot`, with `granted`; the version before is
    /// kept for one undo, and the new one is on trial.
    fn update(&mut self, x: Install, slot: u64, granted: u32) {
        let Some(p) = self.person_id() else { return };
        let old = self.installed[slot as usize].clone();
        let (mut nb, mut nk) = ([0u8; 8], [0u8; 8]);
        let file = ins::bundle_file(p, slot, &mut nb);
        let kept = ins::kept_file(p, slot, &mut nk);
        let m = &self.machine;
        let done = m.root(APPS).and_then(|a| {
            let now = m.lookup(a, file).and_then(|f| m.read_all(f))?;
            m.open(a, kept).and_then(|f| m.write_all(f, &now))?;
            m.open(a, file).and_then(|f| m.write_all(f, &x.bytes))
        });
        let place = Place {
            ticked: granted & x.promise,
            version: x.version,
            promise: x.promise,
            flags: x.flags,
            trial: ins::TRIAL_STARTS,
            kept: Kept {
                ticked: old.ticked,
                version: old.version,
                promise: old.promise,
                flags: old.flags,
            },
            tried: 0,
            went_back_from: 0,
            last_ms: 0,
            ..old.clone()
        };
        if done.is_err() || self.write_record(p, slot, &place).is_err() {
            say!("update: {} could not be kept", x.name);
            self.install = Some(Install {
                stage: Stage::Read(alloc::vec![String::from(
                    "Not updated: the disk did not take it. You keep the version you had."
                )]),
                bytes: Vec::new(),
                ..x
            });
            return;
        }
        self.record(
            act::SETTINGS,
            &self.app_label(label::UPDATE, slot, &place.name),
        );
        say!(
            "update: {} {} to {}, may {:#x}, on trial for {} starts{}",
            place.name,
            old.version,
            place.version,
            place.ticked,
            place.trial,
            if place.flags & flags::SECURITY_FIX != 0 {
                " (a security fix)"
            } else {
                ""
            }
        );
        let mut read = alloc::vec![
            format!(
                "Updated from version {} to {}. {}",
                old.version,
                place.version,
                if x.more == 0 {
                    String::from("It asks for nothing new.")
                } else {
                    format!("It may: {}.", self.may_words(place.ticked))
                }
            ),
            format!(
                "On trial for its next {} starts: Alaya notes what it tries beyond what you \
                 allowed.",
                place.trial
            ),
        ];
        read.push(if place.flags & flags::SECURITY_FIX != 0 {
            String::from("It is a security fix, so it is never held back or taken back.")
        } else {
            String::from("If it is clearly slower to start, Alaya goes back to the version before by itself.")
        });
        read.push(String::from(
            "The new version starts the next time you open it. Undo takes you back.",
        ));
        self.install = Some(Install {
            stage: Stage::Read(read),
            title: format!("{} is updated", x.name),
            bytes: Vec::new(),
            ..x
        });
    }

    /// Swap a place's two versions (undo, redo, and the Experience
    /// Guardian going back). The version swapped in is not on trial.
    fn swap_versions(&mut self, slot: u64) -> bool {
        let Some(p) = self.person_id() else {
            return false;
        };
        let mut place = self.installed[slot as usize].clone();
        if place.kept.version == 0 {
            return false;
        }
        let (mut nb, mut nk) = ([0u8; 8], [0u8; 8]);
        let file = ins::bundle_file(p, slot, &mut nb);
        let kept = ins::kept_file(p, slot, &mut nk);
        let m = &self.machine;
        let done = m.root(APPS).and_then(|a| {
            let now = m.lookup(a, file).and_then(|f| m.read_all(f))?;
            let before = m.lookup(a, kept).and_then(|f| m.read_all(f))?;
            m.open(a, file).and_then(|f| m.write_all(f, &before))?;
            m.open(a, kept).and_then(|f| m.write_all(f, &now))
        });
        if done.is_err() {
            return false;
        }
        let k = place.kept;
        place.kept = Kept {
            ticked: place.ticked,
            version: place.version,
            promise: place.promise,
            flags: place.flags,
        };
        place.ticked = k.ticked;
        place.version = k.version;
        place.promise = k.promise;
        place.flags = k.flags;
        place.trial = 0;
        place.tried = 0;
        self.write_record(p, slot, &place).is_ok()
    }

    /// An undo or redo of an update: the two versions swap.
    pub(crate) fn undo_update(&mut self, l: &[u8]) {
        let slot = u64::from(l[1]);
        if slot >= role::INSTALLED_MAX || !self.swap_versions(slot) {
            return;
        }
        let place = &self.installed[slot as usize];
        say!(
            "update: {} is at version {} again",
            place.name,
            place.version
        );
    }

    /// The window of installed place `slot` opened: how long its start
    /// took, and, on trial, whether it is clearly slower than usual.
    pub(crate) fn started(&mut self, slot: u64) {
        let i = slot as usize;
        let t0 = core::mem::take(&mut self.starting[i]);
        let Some(p) = self.person_id() else { return };
        if t0 == 0 || self.installed[i].state != ins::ON {
            return;
        }
        let ms = alaya_sys::rt::now_ms().saturating_sub(t0) as u32;
        let mut place = self.installed[i].clone();
        place.last_ms = ms;
        say!(
            "{} {} started in {ms} ms (usually {} ms){}",
            place.name,
            place.version,
            place.usual_ms,
            if place.trial > 0 { ", on trial" } else { "" }
        );
        if place.trial == 0 {
            place.usual_ms = if place.usual_ms == 0 {
                ms
            } else {
                (place.usual_ms * 3 + ms) / 4
            };
            let _ = self.write_record(p, slot, &place);
            return;
        }
        place.trial -= 1;
        let slower = clearly_slower(ms, place.usual_ms);
        let _ = self.write_record(p, slot, &place);
        if !slower {
            return;
        }
        if place.flags & flags::SECURITY_FIX != 0 {
            say!(
                "{} {} is slower, but a security fix: kept",
                place.name,
                place.version
            );
            return;
        }
        // The Experience Guardian: back to the version before.
        let from = place.version;
        self.end_app(role::INSTALLED + slot);
        if !self.swap_versions(slot) {
            return;
        }
        let mut back = self.installed[i].clone();
        back.went_back_from = from;
        let _ = self.write_record(p, slot, &back);
        self.record(
            act::SETTINGS,
            &self.app_label(label::UPDATE, slot, &back.name),
        );
        say!(
            "guardian: {} {from} took {ms} ms to start (usually {} ms): back to version {}",
            back.name,
            back.usual_ms,
            back.version
        );
        self.say_note(format!(
            "{} {from} was much slower to start, so Alaya went back to version {}",
            back.name, back.version
        ));
    }

    /// Whether the install sheet is up and takes the keys: not during a
    /// rehearsal, when the app being rehearsed has them.
    pub(crate) fn sheet_up(&self) -> bool {
        self.install
            .as_ref()
            .is_some_and(|x| !matches!(x.stage, Stage::Rehearsing(_)))
    }

    /// The app being rehearsed, if one is.
    pub(crate) fn rehearsing(&self) -> Option<&str> {
        self.install
            .as_ref()
            .filter(|x| matches!(x.stage, Stage::Rehearsing(_)))
            .map(|x| x.name.as_str())
    }

    /// End the running copy of installed role `r` for good: its windows
    /// close, and the first program ends the program itself.
    pub(crate) fn end_app(&mut self, r: u64) {
        self.forget(r);
        let _ = alaya_sys::signal(self.session_ntfn, 1 << (dk::END_SHIFT + r));
    }

    /// R on the promise: run the app for real on a side copy of the
    /// person's documents (a job on the whole documents folder), with
    /// what is ticked, before installing anything (decision 148).
    fn rehearse(&mut self) {
        let Some(p) = self.person_id() else { return };
        let Some(x) = self.install.take() else { return };
        let fail = |x: Install, why: &str| Install {
            stage: Stage::Read(alloc::vec![
                format!("No rehearsal: {why}."),
                String::from("Nothing was changed."),
            ]),
            ..x
        };
        let Some(slot) = self.installed.iter().position(|q| q.state == ins::EMPTY) else {
            self.install = Some(fail(x, "there is no free place; remove an app first"));
            return;
        };
        let slot = slot as u64;
        let r = role::INSTALLED + slot;
        let mut nb = [0u8; 8];
        let name = ins::bundle_file(p, slot, &mut nb);
        let m = &self.machine;
        let kept = m
            .root(APPS)
            .and_then(|a| m.open(a, name))
            .and_then(|f| m.write_all(f, &x.bytes));
        let place = Place {
            state: ins::REHEARSAL,
            ticked: x.ticked & x.promise,
            version: x.version,
            promise: x.promise,
            flags: x.flags,
            id: x.id,
            name: x.name.clone(),
            ..Place::default()
        };
        if kept.is_err() || self.write_record(p, slot, &place).is_err() {
            self.install = Some(fail(x, "the disk did not take it"));
            return;
        }
        let Some(s) = self.mine() else { return };
        let label = format!("rehearsal: {}", x.name);
        let started = s
            .root(fs::DOCS)
            .and_then(|docs| s.job_start(fs::app_id(r), docs, 0, 0, &[], label.as_bytes()));
        if let Err(e) = started {
            say!("rehearsal: no side copy ({e})");
            let _ = self.write_record(p, slot, &Place::default());
            self.install = Some(fail(x, "Alaya could not make a side copy"));
            return;
        }
        say!(
            "rehearsal: {} {} runs in place {slot} on a side copy, may {:#x}",
            x.name,
            x.version,
            place.ticked
        );
        self.rehearsed = None;
        let _ = alaya_sys::signal(self.session_ntfn, 1 << r);
        self.install = Some(Install {
            stage: Stage::Rehearsing(slot),
            ..x
        });
        self.say_note(format!(
            "Rehearsing {} on a copy of your files. Alaya key + R ends it",
            place.name
        ));
    }

    /// End a rehearsal: the app ends, the side copy shows what it read
    /// and changed, then is thrown away (checked on the store), and the
    /// place is free again.
    fn end_rehearsal(&mut self, slot: u64) {
        let Some(p) = self.person_id() else { return };
        let Some(x) = self.install.take() else { return };
        let r = role::INSTALLED + slot;
        let app = fs::app_id(r);
        self.end_app(r);
        let Some(s) = self.mine() else { return };
        let _ = s.save();
        let (read, changed) = s.job_trace(app).unwrap_or_default();
        let named = |f: u64| s.name_of(f, 0).ok().filter(|n| !n.is_empty());
        let list = |files: &mut dyn Iterator<Item = String>| {
            let all: Vec<String> = files.collect();
            let n = all.len();
            let mut l: Vec<String> = all.into_iter().take(4).collect();
            if n > 4 {
                l.push(format!("and {} more", n - 4));
            }
            l.join(", ")
        };
        let mut lines = alloc::vec![format!(
            "{} ran on a side copy of your documents. Nothing real was touched.",
            x.name
        )];
        lines.push(if read.is_empty() {
            String::from("It read none of your files.")
        } else {
            format!(
                "It read {} of your files: {}.",
                read.len(),
                list(
                    &mut read
                        .iter()
                        .map(|&f| named(f).unwrap_or_else(|| String::from("a file")))
                )
            )
        });
        lines.push(if changed.is_empty() {
            String::from("It would have changed nothing.")
        } else {
            format!(
                "It would have changed {}: {}.",
                changed.len(),
                list(&mut changed.iter().map(|&(f, what)| match named(f) {
                    Some(n) if what & fs::what::DELETED != 0 => format!("{n} (removed)"),
                    Some(n) if what & fs::what::CREATED != 0 => format!("{n} (made)"),
                    Some(n) => format!("{n} (changed)"),
                    None => String::from("a new file (made)"),
                }))
            )
        });
        let gone = s.job_discard(app).is_ok() && s.job_trace(app).is_err();
        lines.push(if gone {
            String::from("The side copy is thrown away: checked on the store, none of it is left.")
        } else {
            String::from("The side copy could not be thrown away: see Settings, Apps.")
        });
        let mut nb = [0u8; 8];
        let file = ins::bundle_file(p, slot, &mut nb);
        let m = &self.machine;
        if let Ok(a) = m.root(APPS) {
            let _ = m.remove(a, file);
        }
        let _ = self.write_record(p, slot, &Place::default());
        say!(
            "rehearsal: {} read {}, would have changed {}; side copy gone: {gone}",
            x.name,
            read.len(),
            changed.len()
        );
        self.rehearsed = Some((read.len() as u64, changed.len() as u64, gone));
        self.install = Some(Install {
            stage: Stage::Rehearsed(lines),
            ..x
        });
    }

    /// `dk::TRIAL` for the checks.
    pub(crate) fn trial_words(&self, slot: u64) -> [u64; 6] {
        let Some(place) = self.installed.get(slot as usize) else {
            return [dk::BAD_REQUEST, 0, 0, 0, 0, 0];
        };
        [
            dk::OK,
            u64::from(place.trial)
                | u64::from(place.tried & 0xff) << 8
                | u64::from(place.went_back_from) << 16
                | u64::from(place.flags) << 48,
            u64::from(place.usual_ms),
            u64::from(place.last_ms),
            u64::from(place.kept.version),
            self.rehearsed.map_or(0, |(read, changed, gone)| {
                read | changed << 16 | u64::from(gone) << 32
            }),
        ]
    }
}

/// What the install sheet says of a place's trial.
fn trial_lines(place: &Place) -> Vec<String> {
    let mut l = Vec::new();
    if place.trial > 0 {
        l.push(format!(
            "Version {} is on trial for {} more starts. It tried {} {} beyond what you allowed \
             (what it tried, not proof that it is safe).",
            place.version,
            place.trial,
            place.tried,
            if place.tried == 1 { "thing" } else { "things" }
        ));
    }
    if place.went_back_from != 0 {
        l.push(format!(
            "Version {} was much slower to start, so Alaya went back to version {}.",
            place.went_back_from, place.version
        ));
    }
    l
}
