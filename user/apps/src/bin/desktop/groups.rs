// SPDX-License-Identifier: GPL-3.0-only
//! Groups and protected tasks on the desktop (milestone 7a, step 1;
//! `continuity.md` section 4, decision 85).
//!
//! A group is a label the person makes: windows go in by dropping them
//! on its label, files from the file browser (G). It never changes what
//! an app sees. "Make it a task" shows exactly which files apps in it
//! will see, and only after the person confirms does the store keep the
//! wall: an app whose window is in the task sees those files and nothing
//! else. A paused task's apps are frozen by the first program, not
//! closed (rule B3), and their windows say so.

use alloc::string::String;
use alloc::vec::Vec;

use alaya_apps::store::{self as st, FinishRow, GroupRow, SinceRow};
use alaya_programs::desktop::{self as dk, event, role};
use alaya_programs::files as fs;
use alaya_sys::say;
use alaya_ui::scene;
use alaya_ui::widgets::Key;

use super::{Desk, JobView, act, capital, label};

/// What the group screen shows.
pub(crate) struct GroupScreen {
    pub id: u32,
    pub chosen: usize,
    pub stage: GStage,
}

pub(crate) enum GStage {
    /// What is in the group: (name, file, is a folder).
    Look(Vec<(String, u64, bool)>),
    /// "Make it a task?": what apps in it will see.
    Confirm(Vec<String>),
    /// Naming a group: the name so far, and whether it is a new group.
    Name(String, bool),
    /// Put this file (and its name) in which group?
    Choose(u64, String),
    /// "Since you left": the lines, when the person left, the apps open
    /// then (bit r), their note; and the line whose receipt is shown.
    Brief {
        lines: Vec<SinceRow>,
        left: u64,
        roles: u64,
        note: String,
        receipt: Option<usize>,
    },
    /// Writing the group's note (the person's own words).
    Note(String),
    /// Finish: each line, its name, and whether it goes to the archive.
    Finish(Vec<(FinishRow, String, bool)>),
}

/// The colours new groups take, in turn.
const COLORS: [u32; 6] = [
    0x23_68_6b, 0x9a_5b_00, 0x5b_4b_8a, 0x2f_6f_3e, 0x8a_3b_4b, 0x3b_5b_8a,
];

/// The labels the desktop starts a person with (milestone 6's two).
const STARTERS: [(&str, u32); 2] = [("Work", 0x23_68_6b), ("Home", 0x9a_5b_00)];

/// At most this many group labels next to the dock.
pub(crate) const CHIPS: usize = 4;

/// Apps that are Alaya's own carriers see everything anyway: a task's
/// wall is not for them.
fn walled(r: u64) -> bool {
    r != 0 && ![role::FILES, role::SETTINGS, role::BACK].contains(&r)
}

/// A file number in four bytes for an undo label (documents are small
/// numbers; the class is in the low bits).
fn short(f: u64) -> [u8; 4] {
    (f as u32).to_le_bytes()
}

fn long(b: &[u8]) -> u64 {
    u64::from(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

impl Desk {
    /// Read the person's groups (making the two starter labels the first
    /// time), and put each walled app's window back in its task.
    pub(crate) fn load_groups(&mut self) {
        let Some(s) = self.mine() else { return };
        let mut l = s.groups().unwrap_or_default();
        if l.is_empty() {
            for (name, color) in STARTERS {
                let _ = s.group_new(name, color);
            }
            l = s.groups().unwrap_or_default();
        }
        self.groups = l;
        for r in 1..=role::COUNT {
            if walled(r)
                && let Ok(Some(t)) = s.task_of(fs::app_id(r))
            {
                self.set_group_of(r, t);
            }
        }
        self.place_chips();
    }

    /// The labels next to the dock (in the look that shows them).
    pub(crate) fn place_chips(&mut self) {
        // A finished group has no label (Finish can be undone).
        let n = self.groups.iter().filter(|g| !g.done).count().min(CHIPS);
        self.wm.groups = if self.look.groups() {
            self.groups
                .iter()
                .filter(|g| !g.done)
                .take(n)
                .enumerate()
                .map(|(i, g)| {
                    (
                        g.id,
                        scene::group_chip(
                            self.width,
                            self.height,
                            super::dock_items(self.look).count(),
                            n,
                            i,
                        ),
                    )
                })
                .collect()
        } else {
            Vec::new()
        };
        self.damage_all();
    }

    pub(crate) fn group_row(&self, id: u32) -> Option<&GroupRow> {
        self.groups.iter().find(|g| g.id == id)
    }

    /// The label a window in group `id` shows: a task says so.
    pub(crate) fn group_label(&self, id: u32) -> Option<(String, u32)> {
        self.group_row(id).map(|g| {
            let name = if g.paused {
                alloc::format!("{} · paused", g.name)
            } else if g.task {
                alloc::format!("{} · task", g.name)
            } else {
                g.name.clone()
            };
            (name, g.color)
        })
    }

    /// The app with role `r` is in a paused task: its window is frozen.
    pub(crate) fn paused_role(&self, r: u64) -> bool {
        self.groups_of
            .iter()
            .find(|x| x.0 == r)
            .and_then(|x| self.group_row(x.1))
            .is_some_and(|g| g.paused)
    }

    /// A window of role `r` went into group `g` (0: none): its app is
    /// inside the task's wall when `g` is a task, and outside otherwise.
    pub(crate) fn wall_for(&mut self, r: u64, g: u32) {
        if !walled(r) {
            return;
        }
        let Some(s) = self.mine() else { return };
        let task = self.group_row(g).filter(|x| x.task).map(|x| x.id);
        let app = fs::app_id(r);
        let was = s.task_of(app).ok().flatten();
        if was == task {
            return;
        }
        match s.open_in_task(app, task) {
            Ok(()) => match task {
                Some(t) => say!(
                    "{} is inside the task {}: it sees the task's files, nothing else",
                    st::app_name(app),
                    self.group_row(t).map_or("?", |g| g.name.as_str())
                ),
                None => say!("{} is outside any task", st::app_name(app)),
            },
            Err(e) => say!("the task's wall did not take: {e}"),
        }
        // Into a paused task: frozen with it; out of one: going again.
        let paused_now = task
            .and_then(|t| self.group_row(t))
            .is_some_and(|g| g.paused);
        let paused_was = was
            .and_then(|t| self.group_row(t))
            .is_some_and(|g| g.paused);
        if paused_now != paused_was {
            self.freeze(r, paused_now);
        }
    }

    /// Ask the first program to stop (or start again) role `r`'s app.
    fn freeze(&mut self, r: u64, stop: bool) {
        let bit = if stop {
            dk::PAUSE_SHIFT
        } else {
            dk::GO_ON_SHIFT
        };
        let _ = alaya_sys::signal(self.session_ntfn, 1 << (bit + r));
    }

    /// Open the group screen on group `id` (0: the focused window's group,
    /// or the first).
    pub(crate) fn group_open(&mut self, id: u32) -> u64 {
        if self.mine().is_none()
            || self.pick.is_some()
            || self.ask.is_some()
            || self.job.is_some()
            || self.search.is_some()
        {
            return dk::BAD_REQUEST;
        }
        if let Some(s) = self.mine() {
            self.groups = s.groups().unwrap_or_default();
        }
        let id = match id {
            0 => self
                .wm
                .focused()
                .and_then(|w| self.wm.get(w))
                .and_then(|w| w.group)
                .or_else(|| self.groups.first().map(|g| g.id)),
            i => Some(i),
        };
        let Some(id) = id.filter(|i| self.group_row(*i).is_some()) else {
            return dk::BAD_REQUEST;
        };
        let files = self.group_contents(id);
        self.gscreen = Some(GroupScreen {
            id,
            chosen: 0,
            stage: GStage::Look(files),
        });
        let g = self.group_row(id).unwrap();
        say!(
            "group screen: {}{} with {} files",
            g.name,
            if g.task { " (a protected task)" } else { "" },
            self.gscreen.as_ref().map_or(0, |x| match &x.stage {
                GStage::Look(l) => l.len(),
                _ => 0,
            })
        );
        self.damage_all();
        dk::OK
    }

    /// The file browser hands over a file to put in a group: ask which.
    pub(crate) fn group_choose(&mut self, f: u64) -> u64 {
        let Some(s) = self.mine() else {
            return dk::BAD_REQUEST;
        };
        if self.gscreen.is_some() || self.pick.is_some() || self.job.is_some() {
            return dk::BAD_REQUEST;
        }
        self.groups = s.groups().unwrap_or_default();
        let Ok(name) = s.name_of(f, 0) else {
            return dk::BAD_REQUEST;
        };
        self.gscreen = Some(GroupScreen {
            id: 0,
            chosen: 0,
            stage: GStage::Choose(f, name),
        });
        self.damage_all();
        dk::OK
    }

    fn group_contents(&self, id: u32) -> Vec<(String, u64, bool)> {
        let Some(s) = self.mine() else {
            return Vec::new();
        };
        let mut l: Vec<(String, u64, bool)> = s
            .group_files(id)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|f| {
                let name = s.name_of(f, 0).ok()?;
                let dir = s.stat(f).is_ok_and(|x| x.2);
                Some((name, f, dir))
            })
            .collect();
        l.sort();
        l
    }

    /// For STATE: 0 no group screen, 1 a group, 2 "make it a task?",
    /// 3 naming, 4 choosing a group for a file.
    pub(crate) fn group_state(&self) -> u64 {
        match self.gscreen.as_ref().map(|g| &g.stage) {
            None => 0,
            Some(GStage::Look(_)) => 1,
            Some(GStage::Confirm(_)) => 2,
            Some(GStage::Name(..)) => 3,
            Some(GStage::Choose(..)) => 4,
            Some(GStage::Brief { receipt: None, .. }) => 5,
            Some(GStage::Brief { .. }) => 6,
            Some(GStage::Note(_)) => 7,
            Some(GStage::Finish(_)) => 8,
        }
    }

    pub(crate) fn group_key(&mut self, k: Key) {
        let current = self
            .gscreen
            .as_ref()
            .and_then(|g| self.group_row(g.id))
            .map(|x| x.name.clone())
            .unwrap_or_default();
        if self.brief_key(k) {
            self.damage_all();
            return;
        }
        let Some(g) = &mut self.gscreen else { return };
        let rows = match &g.stage {
            GStage::Look(l) => l.len(),
            GStage::Confirm(l) => l.len(),
            GStage::Finish(l) => l.len(),
            GStage::Name(..) | GStage::Brief { .. } | GStage::Note(_) => 0,
            GStage::Choose(..) => self.groups.len(),
        };
        let id = g.id;
        match (&mut g.stage, k) {
            (GStage::Name(name, _), Key::Char(c)) => {
                if name.len() < 16 && c != '/' {
                    name.push(c);
                }
            }
            (GStage::Name(name, _), Key::Backspace) => {
                name.pop();
            }
            (GStage::Name(_, new), Key::Escape) => {
                if *new && id == 0 {
                    self.gscreen = None;
                } else {
                    let files = self.group_contents(id);
                    self.gscreen = Some(GroupScreen {
                        id,
                        chosen: 0,
                        stage: GStage::Look(files),
                    });
                }
            }
            (GStage::Name(..), Key::Enter) => return self.group_ok(),
            (_, Key::Up) => g.chosen = g.chosen.saturating_sub(1),
            (_, Key::Down) => g.chosen = (g.chosen + 1).min(rows.saturating_sub(1)),
            (GStage::Confirm(_), Key::Escape | Key::Backspace) => {
                let files = self.group_contents(id);
                self.gscreen = Some(GroupScreen {
                    id,
                    chosen: 0,
                    stage: GStage::Look(files),
                });
            }
            (GStage::Look(_) | GStage::Choose(..), Key::Escape) => self.gscreen = None,
            (GStage::Look(_), Key::Char('t' | 'T')) => {
                if self.group_row(id).is_some_and(|x| x.task) {
                    self.set_task(id, false);
                    self.gscreen = None;
                } else {
                    return self.group_ok();
                }
            }
            (GStage::Look(_), Key::Char('p' | 'P')) => {
                if self.group_row(id).is_some_and(|x| x.task) {
                    return self.group_ok();
                }
            }
            (GStage::Look(_) | GStage::Choose(..), Key::Char('n' | 'N')) => {
                let keep = if let GStage::Choose(f, _) = g.stage {
                    f
                } else {
                    0
                };
                g.stage = GStage::Name(String::new(), true);
                g.id = 0;
                self.gchoose_after = keep;
            }
            (GStage::Look(_), Key::Char('w' | 'W')) => {
                let note = self
                    .mine()
                    .and_then(|s| s.return_point(id).ok().flatten())
                    .map(|r| r.3)
                    .unwrap_or_default();
                if let Some(g) = &mut self.gscreen {
                    g.stage = GStage::Note(note);
                }
            }
            (GStage::Look(_), Key::Char('f' | 'F')) => {
                let plan = self.finish_lines(id);
                if let Some(g) = &mut self.gscreen {
                    g.chosen = 0;
                    g.stage = GStage::Finish(plan);
                }
            }
            (GStage::Finish(l), Key::Char(' ')) => {
                if let Some(x) = l.get_mut(g.chosen)
                    && matches!(x.0.fate, fs::fate::KEEP | fs::fate::ARCHIVE)
                {
                    x.2 = !x.2;
                }
            }
            (GStage::Finish(_), Key::Escape | Key::Backspace) => {
                let files = self.group_contents(id);
                self.gscreen = Some(GroupScreen {
                    id,
                    chosen: 0,
                    stage: GStage::Look(files),
                });
            }
            (GStage::Finish(_), Key::Enter) => return self.group_ok(),
            (GStage::Look(_), Key::Char('r' | 'R')) => {
                g.stage = GStage::Name(current, false);
            }
            (GStage::Look(l), Key::Delete | Key::Backspace) => {
                if let Some(&(ref name, f, _)) = l.get(g.chosen) {
                    let name = name.clone();
                    self.file_in_group(id, f, &name, false, true);
                    let files = self.group_contents(id);
                    if let Some(g) = &mut self.gscreen {
                        g.chosen = g.chosen.min(files.len().saturating_sub(1));
                        g.stage = GStage::Look(files);
                    }
                }
            }
            (GStage::Look(_) | GStage::Choose(..) | GStage::Confirm(_), Key::Enter) => {
                return self.group_ok();
            }
            _ => {}
        }
        self.damage_all();
    }

    /// The screen's main button.
    pub(crate) fn group_ok(&mut self) {
        let Some(g) = self.gscreen.take() else { return };
        match g.stage {
            GStage::Look(files) => {
                let Some(row) = self.group_row(g.id).cloned() else {
                    return;
                };
                if row.task {
                    self.set_paused(g.id, !row.paused);
                } else {
                    // Show exactly what apps in it will see, first.
                    let s = self.mine();
                    let rows = files
                        .iter()
                        .map(|(name, f, dir)| {
                            let inside = match (&s, dir) {
                                (Some(s), true) => s.list(*f).map_or(0, |l| l.len()),
                                _ => 0,
                            };
                            match dir {
                                true => {
                                    alloc::format!("{name}   ·   a folder, {inside} things inside")
                                }
                                false => name.clone(),
                            }
                        })
                        .collect();
                    self.gscreen = Some(GroupScreen {
                        id: g.id,
                        chosen: 0,
                        stage: GStage::Confirm(rows),
                    });
                }
            }
            GStage::Confirm(_) => self.set_task(g.id, true),
            GStage::Name(name, new) => {
                let name = String::from(name.trim());
                let Some(s) = self.mine() else { return };
                if name.is_empty() {
                    self.gscreen = Some(GroupScreen {
                        id: g.id,
                        chosen: 0,
                        stage: GStage::Name(name, new),
                    });
                    return;
                }
                if new {
                    let color = COLORS[self.groups.len() % COLORS.len()];
                    match s.group_new(&name, color) {
                        Ok(id) => {
                            say!("a new group: {name}");
                            self.groups = s.groups().unwrap_or_default();
                            self.place_chips();
                            let f = core::mem::take(&mut self.gchoose_after);
                            if f != 0 {
                                let fname = s.name_of(f, 0).unwrap_or_default();
                                self.file_in_group(id, f, &fname, true, true);
                            }
                            self.say_note(alloc::format!("A new group: {name}"));
                        }
                        Err(e) => {
                            say!("the group was not made: {e}");
                            self.say_note(String::from(
                                "That name is taken, or there are too many groups",
                            ));
                        }
                    }
                } else if let Some(mut row) = self.group_row(g.id).cloned() {
                    let old = core::mem::replace(&mut row.name, name.clone());
                    if s.group_set(&row).is_ok() {
                        say!("group {old} is now called {name}");
                        self.groups = s.groups().unwrap_or_default();
                    }
                }
            }
            GStage::Choose(f, name) => {
                if let Some(id) = self.groups.get(g.chosen).map(|x| x.id) {
                    self.file_in_group(id, f, &name, true, true);
                }
            }
            GStage::Brief { roles, receipt, .. } => match receipt {
                None => self.resume(g.id, roles),
                Some(_) => {
                    self.gscreen = Some(g);
                    self.brief_key(Key::Backspace);
                }
            },
            GStage::Note(text) => self.save_note(g.id, &text),
            GStage::Finish(lines) => {
                let archive: Vec<u64> = lines.iter().filter(|x| x.2).map(|x| x.0.file).collect();
                self.finish(g.id, &archive, true);
            }
        }
        self.damage_all();
    }

    /// The Finish screen's lines: Alaya's suggestion, which the person
    /// changes with Space.
    fn finish_lines(&self, id: u32) -> Vec<(FinishRow, String, bool)> {
        let Some(s) = self.mine() else {
            return Vec::new();
        };
        s.finish_plan(id)
            .unwrap_or_default()
            .into_iter()
            .map(|r| {
                let name = s.name_of(r.file, 0).unwrap_or_default();
                (r, name, r.fate == fs::fate::ARCHIVE)
            })
            .collect()
    }

    /// Finish group `id` (on the undo list when `record`).
    pub(crate) fn finish(&mut self, id: u32, archive: &[u64], record: bool) {
        let Some(s) = self.mine() else { return };
        let name = self.group_row(id).map_or(String::new(), |g| g.name.clone());
        match s.finish(id, archive) {
            Ok(receipt) => {
                let rname = s.name_of(receipt, 0).unwrap_or_default();
                say!(
                    "finished {name}: {} archived, the receipt is {rname}",
                    archive.len()
                );
                if record {
                    let mut l = alloc::vec![label::FINISH, id as u8];
                    l.extend(name.bytes().take(fs::LABEL - 2));
                    self.record(act::GROUP, &l);
                }
                if self.current_group == Some(id) {
                    self.current_group = None;
                }
                self.groups = s.groups().unwrap_or_default();
                self.place_chips();
                self.say_note(alloc::format!(
                    "Finished {name}. Your receipt: {rname} · Alaya key + Z brings it all back"
                ));
            }
            Err(e) => {
                say!("{name} was not finished: {e}");
                self.say_note(alloc::format!("{} could not be finished", capital(&name)));
            }
        }
    }

    /// Undo Finish: everything comes back.
    fn unfinish(&mut self, id: u32) {
        let Some(s) = self.mine() else { return };
        match s.finish_undo(id) {
            Ok(()) => {
                say!("finish undone: group {id} is back as it was");
                self.groups = s.groups().unwrap_or_default();
                self.place_chips();
            }
            Err(e) => say!("finish was not undone: {e}"),
        }
    }

    /// Put file `f` in group `id` (or take it out); on the undo list when
    /// `record`.
    pub(crate) fn file_in_group(&mut self, id: u32, f: u64, name: &str, add: bool, record: bool) {
        let Some(s) = self.mine() else { return };
        let gname = self.group_row(id).map_or(String::new(), |g| g.name.clone());
        let done = if add {
            s.group_add(id, f)
        } else {
            s.group_take(id, f)
        };
        match done {
            Ok(true) => {
                say!(
                    "{name} {} the group {gname}",
                    if add { "is now in" } else { "was taken out of" }
                );
                if record {
                    let mut l = alloc::vec![label::IN_GROUP, id as u8, add as u8];
                    l.extend(short(f));
                    l.extend(name.bytes().take(fs::LABEL - 7));
                    self.record(act::GROUP, &l);
                }
                self.say_note(alloc::format!(
                    "{name} {} {gname}",
                    if add { "is in" } else { "is out of" }
                ));
            }
            Ok(false) => self.say_note(alloc::format!("{name} is already there")),
            Err(e) => say!("the group did not take it: {e}"),
        }
    }

    /// Make group `id` a protected task, or a loose group again; windows
    /// in it go inside (or outside) the wall. On the undo list.
    pub(crate) fn set_task(&mut self, id: u32, on: bool) {
        self.set_flags(id, on, false, true);
    }

    pub(crate) fn set_paused(&mut self, id: u32, on: bool) {
        let task = self.group_row(id).is_some_and(|g| g.task);
        if task {
            self.set_flags(id, true, on, true);
        }
    }

    /// The task and pause flags of group `id`, and what follows from them.
    pub(crate) fn set_flags(&mut self, id: u32, task: bool, paused: bool, record: bool) {
        let Some(s) = self.mine() else { return };
        let Some(mut row) = self.group_row(id).cloned() else {
            return;
        };
        let was = (row.task, row.paused);
        let paused = paused && task;
        if was == (task, paused) {
            return;
        }
        row.task = task;
        row.paused = paused;
        if let Err(e) = s.group_set(&row) {
            say!("the group did not change: {e}");
            return;
        }
        self.groups = s.groups().unwrap_or_default();
        let code = |t: bool, p: bool| u8::from(t) | u8::from(p) << 1;
        if record {
            let mut l = alloc::vec![
                label::TASK,
                id as u8,
                code(was.0, was.1),
                code(task, paused)
            ];
            l.extend(row.name.bytes().take(fs::LABEL - 4));
            self.record(act::GROUP, &l);
        }
        // The windows in it: inside or outside the wall, frozen or not.
        let roles: Vec<u64> = self
            .groups_of
            .iter()
            .filter(|x| x.1 == id)
            .map(|x| x.0)
            .collect();
        for &r in &roles {
            if was.0 != task {
                // The store let every app go when it stopped being a task.
                self.wall_for(r, id);
            }
            if was.1 != paused && walled(r) {
                self.freeze(r, paused);
            }
        }
        let text = match (was, (task, paused)) {
            (_, (true, true)) => alloc::format!("{} is paused: its apps use nothing", row.name),
            ((true, true), (true, false)) => alloc::format!("{} goes on", row.name),
            (_, (true, false)) => alloc::format!(
                "{} is a protected task: apps in it see only its files",
                capital(&row.name)
            ),
            _ => alloc::format!("{} is a loose group again", row.name),
        };
        say!("{text}");
        self.say_note(text);
        self.damage_all();
    }

    /// An undo (or redo) of a group action: true when it was one.
    pub(crate) fn group_undone(&mut self, l: &[u8], redo: bool) -> bool {
        match l.first() {
            Some(&label::IN_GROUP) if l.len() >= 7 => {
                let (id, add, f) = (u32::from(l[1]), l[2] != 0, long(&l[3..7]));
                let name = String::from_utf8_lossy(&l[7..])
                    .trim_end_matches('\0')
                    .into();
                // Undo does the opposite; a redo does it again.
                let name: String = name;
                self.file_in_group(id, f, &name, add == redo, false);
                true
            }
            Some(&label::FINISH) if l.len() >= 2 => {
                let id = u32::from(l[1]);
                if redo {
                    let archive: Vec<u64> = self
                        .finish_lines(id)
                        .into_iter()
                        .filter(|x| x.2)
                        .map(|x| x.0.file)
                        .collect();
                    self.finish(id, &archive, false);
                } else {
                    self.unfinish(id);
                }
                true
            }
            Some(&label::TASK) if l.len() >= 4 => {
                let code = if redo { l[3] } else { l[2] };
                self.set_flags(u32::from(l[1]), code & 1 != 0, code & 2 != 0, false);
                true
            }
            _ => false,
        }
    }

    /// What the group screen shows.
    pub(crate) fn group_view(&self, g: &GroupScreen) -> JobView {
        let row = self.group_row(g.id);
        let name = row.map_or("", |r| r.name.as_str());
        match &g.stage {
            GStage::Look(files) => {
                let r = row.cloned().unwrap_or(GroupRow {
                    id: 0,
                    name: String::new(),
                    color: 0,
                    task: false,
                    paused: false,
                    done: false,
                    made: 0,
                    left: 0,
                });
                let windows = self.groups_of.iter().filter(|x| x.1 == g.id).count();
                JobView {
                    title: match (r.task, r.paused) {
                        (true, true) => alloc::format!("{name}: a protected task, paused"),
                        (true, false) => alloc::format!("{name}: a protected task"),
                        _ => alloc::format!("{name}: a loose group"),
                    },
                    hint: String::from(if r.task {
                        "Apps opened in it see these files and nothing else. \
                         P: pause or go on · T: a loose group again · F: finish · Delete: take out"
                    } else {
                        "A label only: no app sees anything more. \
                         T: make it a task · N: new group · R: rename · F: finish · Delete: take out"
                    }),
                    rows: files
                        .iter()
                        .map(|(n, _, dir)| {
                            if *dir {
                                alloc::format!("{n}   ·   folder")
                            } else {
                                n.clone()
                            }
                        })
                        .collect(),
                    mark: !files.is_empty(),
                    foot: alloc::format!(
                        "{windows} window{} in it. Files go in from the file browser (G); \
                         windows by dropping them on its label.",
                        if windows == 1 { "" } else { "s" }
                    ),
                    ok: match (r.task, r.paused) {
                        (true, true) => "Go on",
                        (true, false) => "Pause",
                        _ => "Make it a task",
                    },
                    no: "Close",
                    field: None,
                    empty: "Nothing in it yet.",
                }
            }
            GStage::Finish(lines) => JobView {
                title: alloc::format!("Finish {name}?"),
                hint: alloc::format!(
                    "Temporary things go to Archive/{name}, never deleted. Space: keep or archive"
                ),
                rows: lines
                    .iter()
                    .map(|(r, n, archive)| match r.fate {
                        fs::fate::ELSEWHERE => {
                            let other = self
                                .group_row(r.other as u32)
                                .map_or("another group", |g| g.name.as_str());
                            alloc::format!("Still used in {other}   ·   {n}")
                        }
                        fs::fate::HAND_BACK => alloc::format!(
                            "Hand back   ·   {} could {} {n}",
                            st::app_name(r.other),
                            if r.write { "change" } else { "read" }
                        ),
                        _ if *archive => alloc::format!("Archive   ·   {n}"),
                        _ => alloc::format!("Keep   ·   {n}"),
                    })
                    .collect(),
                mark: !lines.is_empty(),
                foot: String::from(
                    "Alaya writes a work receipt from its own records. One undo brings it all back.",
                ),
                ok: "Finish",
                no: "Not now",
                field: None,
                empty: "Nothing in it: Finish only writes the receipt.",
            },
            GStage::Confirm(rows) => JobView {
                title: alloc::format!("Make {name} a protected task?"),
                hint: String::from(
                    "Apps you open in it will see these, and nothing else of yours:",
                ),
                rows: rows.clone(),
                mark: false,
                foot: String::from(
                    "Windows already in it go inside the wall. One undo takes it back.",
                ),
                ok: "Make it a task",
                no: "Not now",
                field: None,
                empty: "Nothing: apps in it would see none of your files.",
            },
            GStage::Name(text, new) => JobView {
                title: if *new {
                    String::from("A new group")
                } else {
                    alloc::format!("Rename {name}")
                },
                hint: String::from("A group is only a label: it changes what no app can see."),
                rows: Vec::new(),
                mark: false,
                foot: String::from("Enter: done · Escape: back"),
                ok: "Done",
                no: "Back",
                field: Some(("Name", text.clone())),
                empty: "",
            },
            GStage::Brief {
                lines,
                left,
                roles,
                note,
                receipt,
            } => match receipt.and_then(|i| lines.get(i)) {
                None => JobView {
                    title: alloc::format!("{name}, left at {}", clock(*left)),
                    hint: {
                        let apps = role_names(*roles);
                        let place = if apps.is_empty() {
                            String::from("no apps were open in it")
                        } else {
                            alloc::format!("you had {apps} open")
                        };
                        if note.is_empty() {
                            alloc::format!("Where you left off: {place}. No note.")
                        } else {
                            alloc::format!(
                                "Where you left off: {place}. Your note: \u{201c}{note}\u{201d}"
                            )
                        }
                    },
                    rows: lines.iter().map(|l| self.line_text(l)).collect(),
                    mark: !lines.is_empty(),
                    foot: String::from(
                        "Each line comes from a record: Enter shows it. W: your note. R: resume.",
                    ),
                    ok: "Resume",
                    no: "Not now",
                    field: None,
                    empty: "Nothing changed since you left. Your earlier work is kept.",
                },
                Some(l) => JobView {
                    title: String::from("Where this line comes from"),
                    hint: self.line_text(l),
                    rows: self.receipt_rows(l),
                    mark: false,
                    foot: String::from("T: see that moment on the timeline. Backspace: back."),
                    ok: "Back",
                    no: "Back",
                    field: None,
                    empty: "",
                },
            },
            GStage::Note(text) => JobView {
                title: alloc::format!("Your note for {name}"),
                hint: String::from(
                    "In your own words, for when you come back. Alaya never writes one for you.",
                ),
                rows: Vec::new(),
                mark: false,
                foot: String::from("Enter: keep it · Escape: back · an empty note clears it"),
                ok: "Keep it",
                no: "Back",
                field: Some(("Note", text.clone())),
                empty: "",
            },
            GStage::Choose(_, fname) => JobView {
                title: alloc::format!("Put {fname} in a group"),
                hint: String::from(
                    "A file can be in several groups. Only a protected task's apps see it.",
                ),
                rows: self
                    .groups
                    .iter()
                    .map(|r| match self.group_label(r.id) {
                        Some((l, _)) => l,
                        None => r.name.clone(),
                    })
                    .collect(),
                mark: true,
                foot: String::from("N: a new group for it"),
                ok: "Put it there",
                no: "Cancel",
                field: None,
                empty: "No groups yet: N makes one.",
            },
        }
    }

    /// A click while the group screen is up.
    pub(crate) fn group_click(&mut self, x: i32, y: i32) {
        let (ok, no) = scene::picker_buttons(self.width, self.height);
        let Some(g) = &mut self.gscreen else { return };
        let first = scene::picker_first(g.chosen);
        if ok.contains(x, y) {
            self.group_ok();
        } else if no.contains(x, y) {
            self.group_key(Key::Escape);
        } else if let Some(i) = scene::picker_row_at(self.width, self.height, x, y, first) {
            g.chosen = i;
            self.damage_all();
        }
    }

    /// The person goes into group `g` (its label, or the first program):
    /// the group they were in is left; one they come back to shows what
    /// changed since, when anything did.
    pub(crate) fn group_go(&mut self, g: u32) -> u64 {
        let Some(s) = self.mine() else {
            return dk::BAD_REQUEST;
        };
        if self.gscreen.is_some() || self.pick.is_some() || self.ask.is_some() {
            return dk::BAD_REQUEST;
        }
        self.groups = s.groups().unwrap_or_default();
        if self.group_row(g).is_none() {
            return dk::BAD_REQUEST;
        }
        if self.current_group != Some(g) {
            if let Some(old) = self.current_group {
                self.group_leave_now(old);
            }
            self.current_group = Some(g);
            if let Ok(Some((left, _, roles, note))) = s.return_point(g) {
                let lines = s.since_left(g).unwrap_or_default();
                let closed = roles & !self.running_roles();
                if !lines.is_empty() || !note.is_empty() || closed != 0 {
                    say!(
                        "since you left {}: {} lines, {} apps to open again{}",
                        self.group_row(g).map_or("?", |x| x.name.as_str()),
                        lines.len(),
                        closed.count_ones(),
                        if note.is_empty() { "" } else { ", your note" }
                    );
                    self.gscreen = Some(GroupScreen {
                        id: g,
                        chosen: 0,
                        stage: GStage::Brief {
                            lines,
                            left,
                            roles,
                            note,
                            receipt: None,
                        },
                    });
                    self.damage_all();
                    return dk::OK;
                }
            }
        }
        let out = self.wm.raise_group(g);
        self.apply(out);
        dk::OK
    }

    /// Roles with a window open (bit r).
    fn running_roles(&self) -> u64 {
        self.wins.iter().fold(0, |b, w| b | 1 << w.owner)
    }

    /// The person left group `id`: keep where they were.
    pub(crate) fn group_leave_now(&mut self, id: u32) {
        let Some(s) = self.mine() else { return };
        let running = self.running_roles();
        let roles = self
            .groups_of
            .iter()
            .filter(|x| x.1 == id && running & 1 << x.0 != 0)
            .fold(0u64, |b, x| b | 1 << x.0);
        match s.group_leave(id, roles) {
            Ok(()) => say!(
                "left {}: {} apps open in it",
                self.group_row(id).map_or("?", |x| x.name.as_str()),
                roles.count_ones()
            ),
            Err(e) => say!("the return point was not kept: {e}"),
        }
    }

    /// Back to where the person was: the apps open then, the group's
    /// windows in front.
    fn resume(&mut self, id: u32, roles: u64) {
        let running = self.running_roles();
        let mut started = 0;
        for r in 1..=role::COUNT {
            if roles & 1 << r != 0 && running & 1 << r == 0 {
                self.set_group_of(r, id);
                let _ = alaya_sys::signal(self.session_ntfn, 1 << r);
                started += 1;
            }
        }
        let out = self.wm.raise_group(id);
        self.apply(out);
        let name = self.group_row(id).map_or(String::new(), |x| x.name.clone());
        say!("resumed {name}: {started} apps opened again");
        self.say_note(alloc::format!("Back to {name}"));
    }

    fn save_note(&mut self, id: u32, text: &str) {
        let Some(s) = self.mine() else { return };
        let text = text.trim();
        match s.group_note(id, text) {
            Ok(()) => {
                say!(
                    "note for group {id}: {} bytes, the person's own words",
                    text.len()
                );
                self.say_note(String::from(if text.is_empty() {
                    "Note cleared"
                } else {
                    "Note kept"
                }));
            }
            Err(e) => say!("the note was not kept: {e}"),
        }
        let files = self.group_contents(id);
        self.gscreen = Some(GroupScreen {
            id,
            chosen: 0,
            stage: GStage::Look(files),
        });
    }

    /// Keys of the briefing, its receipts and the note: true when taken.
    fn brief_key(&mut self, k: Key) -> bool {
        let Some(g) = &mut self.gscreen else {
            return false;
        };
        let id = g.id;
        // Screens that need the store are opened after the match.
        let mut back_to_look = false;
        let mut to_note = false;
        let taken = match &mut g.stage {
            GStage::Note(text) => {
                match k {
                    Key::Char(c) if text.len() + c.len_utf8() <= fs::NOTE_MAX => text.push(c),
                    Key::Backspace => {
                        text.pop();
                    }
                    Key::Enter => {
                        let t = core::mem::take(text);
                        self.save_note(id, &t);
                    }
                    Key::Escape => back_to_look = true,
                    _ => {}
                }
                true
            }
            GStage::Brief {
                lines,
                roles,
                receipt,
                ..
            } => {
                let n = lines.len();
                match (receipt.is_some(), k) {
                    (false, Key::Up) => g.chosen = g.chosen.saturating_sub(1),
                    (false, Key::Down) => g.chosen = (g.chosen + 1).min(n.saturating_sub(1)),
                    (false, Key::Enter) if n > 0 => *receipt = Some(g.chosen),
                    (false, Key::Enter | Key::Char('r' | 'R')) => {
                        let roles = *roles;
                        self.gscreen = None;
                        self.resume(id, roles);
                    }
                    (false, Key::Char('w' | 'W')) => to_note = true,
                    (false, Key::Escape) => {
                        self.gscreen = None;
                        let out = self.wm.raise_group(id);
                        self.apply(out);
                    }
                    (true, Key::Backspace | Key::Escape | Key::Enter) => *receipt = None,
                    (true, Key::Char('t' | 'T')) => {
                        let cn = receipt.and_then(|i| lines.get(i)).map_or(0, |l| l.cn);
                        self.timeline_at(cn);
                    }
                    _ => {}
                }
                true
            }
            _ => false,
        };
        if back_to_look {
            let files = self.group_contents(id);
            if let Some(g) = &mut self.gscreen {
                g.stage = GStage::Look(files);
            }
        }
        if to_note {
            let note = self
                .mine()
                .and_then(|s| s.return_point(id).ok().flatten())
                .map(|r| r.3)
                .unwrap_or_default();
            if let Some(g) = &mut self.gscreen {
                g.stage = GStage::Note(note);
            }
        }
        taken
    }

    /// Show moment `cn` on the timeline ("What happened?").
    fn timeline_at(&mut self, cn: u64) {
        let e = [event::MOMENT, cn, 0, 0, 0];
        say!("receipt: the timeline at moment {cn}");
        match self
            .wins
            .iter()
            .rev()
            .find(|w| w.owner == role::BACK)
            .map(|w| w.id)
        {
            Some(id) => {
                self.push(id, e);
                let mut out = Vec::new();
                self.wm.raise(id, &mut out);
                self.apply(out);
            }
            None => {
                self.hand = Some((role::BACK, e));
                self.show_app(role::BACK, "What happened?");
            }
        }
    }

    /// One line of "since you left", in words.
    fn line_text(&self, l: &SinceRow) -> String {
        let name = self.file_name(l.file, l.cn);
        match l.kind {
            2 => alloc::format!(
                "{name} needs a look: {} changed since it was made",
                self.file_name(l.source, 0)
            ),
            _ => {
                let did = what_words(l.what);
                let times = if l.count > 1 {
                    alloc::format!(" ({} changes)", l.count)
                } else {
                    String::new()
                };
                alloc::format!(
                    "{name} {did} by {}{times}   ·   {}",
                    st::app_name(l.app),
                    clock(l.time)
                )
            }
        }
    }

    /// A line's receipt: the record it comes from, exactly.
    fn receipt_rows(&self, l: &SinceRow) -> Vec<String> {
        match l.kind {
            2 => alloc::vec![
                alloc::format!("A mark on {}: it needs a look", self.file_name(l.file, 0)),
                alloc::format!(
                    "Made by {} from {}",
                    st::app_name(l.app),
                    self.file_name(l.source, 0)
                ),
                alloc::format!("That source is now the version of moment {}", l.cn),
            ],
            _ => alloc::vec![
                alloc::format!("The record: moment {} in your history", l.cn),
                alloc::format!("When: {}", clock_s(l.time)),
                alloc::format!("Which app: {}", st::app_name(l.app)),
                alloc::format!(
                    "What: {} {}",
                    self.file_name(l.file, l.cn),
                    what_words(l.what)
                ),
                alloc::format!(
                    "Changes since you left: {}{}",
                    l.count,
                    if l.count > 1 {
                        " (this is the newest)"
                    } else {
                        ""
                    }
                ),
            ],
        }
    }

    /// A file's name now, or at the moment before `cn` when it is gone.
    fn file_name(&self, f: u64, cn: u64) -> String {
        let Some(s) = self.mine() else {
            return String::from("a file");
        };
        s.name_of(f, 0)
            .or_else(|_| s.name_of(f, cn.saturating_sub(1)))
            .unwrap_or_else(|_| String::from("a file"))
    }

    /// For BRIEF_LINE: line i of the briefing up now, and its receipt.
    pub(crate) fn brief_line(&self, i: usize) -> ([u64; 6], u64) {
        match self.gscreen.as_ref().map(|g| &g.stage) {
            Some(GStage::Brief { lines, .. }) => match lines.get(i) {
                Some(l) => ([dk::OK, l.kind, l.file, l.cn, l.app, l.what], 0),
                None => ([dk::EMPTY, 0, 0, 0, 0, 0], 0),
            },
            _ => ([dk::BAD_REQUEST, 0, 0, 0, 0, 0], 0),
        }
    }

    /// The group label under a point, if any.
    pub(crate) fn chip_at(&self, x: i32, y: i32) -> Option<u32> {
        self.wm
            .groups
            .iter()
            .find(|(_, r)| r.contains(x, y))
            .map(|&(g, _)| g)
    }
}

/// The store's clock as hours and minutes.
fn clock(t: u64) -> String {
    alloc::format!("{}:{:02}", t / 3600 % 24, t / 60 % 60)
}

fn clock_s(t: u64) -> String {
    alloc::format!("{}:{:02}:{:02}", t / 3600 % 24, t / 60 % 60, t % 60)
}

/// What a change did, in words.
fn what_words(w: u64) -> &'static str {
    use alaya_programs::files::what;
    match w {
        w if w & what::FORGOTTEN != 0 => "was forgotten",
        w if w & what::DELETED != 0 => "was deleted",
        w if w & what::CREATED != 0 => "was made",
        w if w & what::RESTORED != 0 => "was put back",
        w if w & what::KEPT != 0 => "was kept from a job",
        w if w & what::RENAMED != 0 => "was renamed",
        _ => "was changed",
    }
}

/// The apps of these roles (bit r), in words.
fn role_names(roles: u64) -> String {
    let names: Vec<&str> = (1..=role::COUNT)
        .filter(|r| roles & 1 << r != 0)
        .map(|r| st::app_name(fs::app_id(r)))
        .collect();
    match names.len() {
        0 => String::new(),
        1 => String::from(names[0]),
        n => alloc::format!("{} and {}", names[..n - 1].join(", "), names[n - 1]),
    }
}
