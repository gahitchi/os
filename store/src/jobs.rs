// SPDX-License-Identifier: GPL-3.0-only
//! Jobs with rules (milestone 7, step 3; protected outcomes,
//! `commitments.md` section 3): a program you don't fully trust works on
//! a side copy of one folder, under rules the person saw on one screen.
//!
//! - **May change: one folder** (and what is in it). The store server
//!   sends the program's requests to the side branch and asks
//!   [`Store::job_may`] first, so a write anywhere else is refused. Rules
//!   follow files, not names: a renamed path changes nothing.
//! - **Must stay unchanged**: a few files inside that folder, refused for
//!   writing like everything outside it.
//! - **Checks before keeping**, run by Alaya ([`Store::job_check`]), never
//!   by the program, after it stopped, on the side copy, read only:
//!   nothing lost, nothing emptied, and the lines of a checklist file, read
//!   from the version the person approved (the moment the job started), so
//!   a program that edits the checklist changes nothing.
//! - **Still up to date?** A file the job read that the person changed
//!   since makes the work "needs a look", like a failed check.
//! - **Keep, hold, or keep anyway.** [`Store::job_keep`] keeps the work in
//!   one commit only when every check passed; otherwise it is held with
//!   the reasons. The person (never the program: the store server lets
//!   only the desktop ask) can keep it anyway, and the history says so
//!   ([`crate::what::ANYWAY`]).
//!
//! A job's rules are kept with the person's app files, keyed by its side
//! branch, so a job survives a restart.

use alloc::vec::Vec;

use crate::disk::Disk;
use crate::format::{BLOCK, Key, word, words};
use crate::grants::RECORDS;
use crate::side::{Keep, SIDE_FLAG, is_side};
use crate::{App, Class, Error, FileId, H_READ, Person, ROOT_OBJ, Store, what};

/// The checks a job can ask for (bits).
pub mod check {
    /// Nothing outside the folder changed (enforced; checked again).
    pub const IN_FOLDER: u64 = 1;
    /// The files that must stay unchanged did (enforced; checked again).
    pub const KEPT_SAME: u64 = 2;
    /// No file that was there at the start is gone.
    pub const NOTHING_LOST: u64 = 4;
    /// No file that had something in it is empty now.
    pub const NONE_EMPTIED: u64 = 8;
    /// Every line of the checklist file holds.
    pub const CHECKLIST: u64 = 16;
    /// Always on: what Alaya enforces is checked again anyway.
    pub const ALWAYS: u64 = IN_FOLDER | KEPT_SAME;
    pub const ALL: u64 = 31;
}

/// At most this many files must stay unchanged.
pub const KEEP_SAME_MAX: usize = 8;
/// The checklist is at most this long.
pub const CHECKLIST_MAX: usize = 2048;
/// And has at most this many lines that check something.
pub const CHECKLIST_LINES: usize = 24;

const K_JOB: u64 = 10;
const K_JOB_SAME: u64 = 11;

fn k_job(side: u64) -> Key {
    Key(RECORDS | K_JOB, side)
}

fn k_job_same(side: u64, i: u64) -> Key {
    Key(RECORDS | K_JOB_SAME, side << 8 | i)
}

/// The rules of one job, as the person approved them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rules {
    /// What it may change: this folder and everything in it.
    pub folder: FileId,
    /// What must stay unchanged (inside the folder).
    pub keep_same: Vec<FileId>,
    /// Which checks run before keeping ([`check`] bits).
    pub checks: u64,
    /// The checklist file, read as it was when the job started.
    pub checklist: Option<FileId>,
}

/// A job, running or waiting for the person.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Job {
    /// Its side branch (also the handle its program's files go to).
    pub side: Person,
    pub owner: Person,
    pub app: App,
    pub rules: Rules,
    pub started: u64,
    /// The program said it is done: it can change nothing more.
    pub done: bool,
    /// The moment it started from (its checklist's version).
    pub fork: u64,
}

/// One check's result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Checked {
    /// Which check ([`check`] bit).
    pub check: u64,
    /// For the checklist: which line (from 1).
    pub line: u32,
    pub ok: bool,
    /// The first file it failed on, when there is one.
    pub file: Option<FileId>,
}

/// What Alaya's checks found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub checks: Vec<Checked>,
    /// Files the job read that the person changed (or removed) since it
    /// started: its work may rest on an old version ("needs a look").
    pub stale: Vec<FileId>,
}

impl Report {
    pub fn passed(&self) -> bool {
        self.stale.is_empty() && self.checks.iter().all(|c| c.ok)
    }
}

/// What [`Store::job_keep`] did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kept {
    /// The work is in the person's files (one commit from now).
    Kept(Vec<FileId>),
    /// Nothing was done: a check failed, or the job is out of date.
    Held(Report),
    /// Nothing was done: the person changed some of the same files
    /// (rule B6). Keep again saying whose version wins.
    Conflicts(Vec<FileId>),
}

/// A checklist line, after reading it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Line<'a> {
    /// `has <path>`: the file is there.
    Has(&'a [u8]),
    /// `says <path> <words>`: the file holds the words (either case).
    Says(&'a [u8], &'a [u8]),
    /// `not <path> <words>`: the file does not hold them.
    Not(&'a [u8], &'a [u8]),
    /// `under <path> <bytes>`: the file is smaller than that.
    Under(&'a [u8], u64),
    /// A line Alaya cannot read: it fails, so a typo never passes.
    Unknown,
}

fn split1(s: &[u8]) -> (&[u8], &[u8]) {
    let s = s.trim_ascii();
    match s.iter().position(|&b| b == b' ' || b == b'\t') {
        Some(i) => (&s[..i], s[i + 1..].trim_ascii()),
        None => (s, &[]),
    }
}

fn parse(line: &[u8]) -> Option<Line<'_>> {
    let line = line.trim_ascii();
    if line.is_empty() || line[0] == b'#' {
        return None;
    }
    let (verb, rest) = split1(line);
    let (path, more) = split1(rest);
    if path.is_empty() {
        return Some(Line::Unknown);
    }
    let verb: Vec<u8> = verb.iter().map(|b| b.to_ascii_lowercase()).collect();
    Some(match (&verb[..], more.is_empty()) {
        (b"has", true) => Line::Has(path),
        (b"says", false) if more.len() <= 64 => Line::Says(path, more),
        (b"not", false) if more.len() <= 64 => Line::Not(path, more),
        (b"under", false) => match core::str::from_utf8(more).ok().and_then(|t| t.parse().ok()) {
            Some(n) => Line::Under(path, n),
            None => Line::Unknown,
        },
        _ => Line::Unknown,
    })
}

impl<D: Disk> Store<D> {
    /// Start a job for `app` on person `p`'s documents under `rules`.
    /// The work happens on a new side branch, from the last saved moment
    /// (so whatever the rules name must be saved). Returns its handle. One
    /// job per app at a time.
    pub fn job_start(
        &mut self,
        p: Person,
        app: App,
        label: &[u8],
        rules: &Rules,
        now: u64,
    ) -> Result<Person, Error> {
        self.ok()?;
        if is_side(p)
            || rules.keep_same.len() > KEEP_SAME_MAX
            || rules.checks & !check::ALL != 0
            || rules.folder.class() != Class::Documents
            || (rules.checks & check::CHECKLIST != 0) != rules.checklist.is_some()
        {
            return Err(Error::BadRequest);
        }
        if self.job_of(p, app)?.is_some() {
            return Err(Error::Exists);
        }
        let side = self.side_open(p, label, now)?;
        let ok = self.rules_hold(side, rules);
        if !matches!(ok, Ok(true)) {
            self.side_discard(side)?;
            return Err(ok.err().unwrap_or(Error::BadRequest));
        }
        self.room(4, false)?;
        let checks = rules.checks | check::ALWAYS;
        let Store { dev, branches, .. } = self;
        let b = branches.get_mut(&p).ok_or(Error::Locked)?;
        let t = &mut b.trees[Class::Apps as usize];
        let id = side & !SIDE_FLAG;
        t.insert(
            dev,
            &b.keys,
            k_job(id),
            words(&[
                app,
                rules.folder.0,
                checks,
                rules.checklist.map_or(0, |f| f.0),
                now,
            ]),
        )?;
        for (i, f) in rules.keep_same.iter().enumerate() {
            t.insert(dev, &b.keys, k_job_same(id, i as u64), words(&[f.0]))?;
        }
        b.touched = true;
        Ok(side)
    }

    /// The rules name what is there when the job starts: a folder, files
    /// inside it, a checklist file.
    fn rules_hold(&mut self, side: Person, r: &Rules) -> Result<bool, Error> {
        if !self.stat(side, r.folder)?.is_dir {
            return Ok(false);
        }
        for f in &r.keep_same {
            if f.class() != Class::Documents || !self.inside(side, *f, r.folder)? {
                return Ok(false);
            }
        }
        if let Some(c) = r.checklist {
            let st = self.stat(side, c)?;
            if st.is_dir || st.size > CHECKLIST_MAX as u64 {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Whether `f` is `folder` or somewhere inside it, in branch `b`.
    pub(crate) fn inside(&mut self, b: Person, f: FileId, folder: FileId) -> Result<bool, Error> {
        let mut up = f.obj();
        for _ in 0..256 {
            if up == folder.obj() {
                return Ok(true);
            }
            if up == ROOT_OBJ {
                return Ok(false);
            }
            up = match self.stat(b, FileId::new(up, Class::Documents)) {
                Ok(st) => st.parent,
                Err(Error::NotFound) => return Ok(false),
                Err(e) => return Err(e),
            };
        }
        Ok(false)
    }

    /// The same, in a past view.
    fn inside_past(&mut self, v: &crate::Past, f: FileId, folder: FileId) -> Result<bool, Error> {
        let mut up = f.obj();
        for _ in 0..256 {
            if up == folder.obj() {
                return Ok(true);
            }
            if up == ROOT_OBJ {
                return Ok(false);
            }
            up = match self.past_stat(v, FileId::new(up, Class::Documents)) {
                Ok(st) => st.parent,
                Err(Error::NotFound) => return Ok(false),
                Err(e) => return Err(e),
            };
        }
        Ok(false)
    }

    /// Every job of person `p`.
    pub fn jobs(&mut self, p: Person) -> Result<Vec<Job>, Error> {
        let recs = {
            let Store { dev, branches, .. } = self;
            let b = branches.get_mut(&p).ok_or(Error::Locked)?;
            b.trees[Class::Apps as usize].collect(dev, &b.keys, k_job(0), k_job(u64::MAX))?
        };
        let sides = self.sides(p)?;
        let mut out = Vec::new();
        for (k, v) in recs {
            if k.0 != RECORDS | K_JOB {
                continue;
            }
            let side = SIDE_FLAG | k.1;
            let Some((_, info)) = sides.iter().find(|(s, _)| *s == side) else {
                continue;
            };
            let same = {
                let Store { dev, branches, .. } = self;
                let b = branches.get_mut(&p).ok_or(Error::Locked)?;
                b.trees[Class::Apps as usize]
                    .collect(dev, &b.keys, k_job_same(k.1, 0), k_job_same(k.1, 0xff))?
                    .into_iter()
                    .map(|(_, v)| FileId(word(&v, 0)))
                    .collect()
            };
            let checks = word(&v, 2);
            let cl = word(&v, 3);
            out.push(Job {
                side,
                owner: p,
                app: word(&v, 0),
                rules: Rules {
                    folder: FileId(word(&v, 1)),
                    keep_same: same,
                    checks: checks & check::ALL,
                    checklist: (cl != 0).then_some(FileId(cl)),
                },
                started: word(&v, 4),
                done: checks & DONE != 0,
                fork: info.fork,
            });
        }
        Ok(out)
    }

    /// The job `app` runs for `p`, if any.
    pub fn job_of(&mut self, p: Person, app: App) -> Result<Option<Job>, Error> {
        Ok(self.jobs(p)?.into_iter().find(|j| j.app == app))
    }

    /// A job by its side branch.
    pub fn job(&mut self, side: Person) -> Result<Job, Error> {
        let owner = self.branch(side)?.side.ok_or(Error::BadRequest)?.owner;
        self.jobs(owner)?
            .into_iter()
            .find(|j| j.side == side)
            .ok_or(Error::NotFound)
    }

    /// May the job's program read (or, with `write`, change) `f`? For a
    /// new file, a removal or a rename, `f` is the folder it happens in,
    /// and then also the file itself. Never once it said it is done.
    pub fn job_may(&mut self, job: &Job, f: FileId, write: bool) -> Result<bool, Error> {
        if f.class() != Class::Documents {
            return Ok(false);
        }
        if write && (job.done || job.rules.keep_same.contains(&f)) {
            return Ok(false);
        }
        self.inside(job.side, f, job.rules.folder)
    }

    /// The program says it is done: from now on it can change nothing,
    /// and the person is asked.
    pub fn job_done(&mut self, side: Person) -> Result<(), Error> {
        let j = self.job(side)?;
        if j.done {
            return Ok(());
        }
        self.room(2, false)?;
        let Store { dev, branches, .. } = self;
        let b = branches.get_mut(&j.owner).ok_or(Error::Locked)?;
        let k = k_job(side & !SIDE_FLAG);
        let mut v = b.trees[Class::Apps as usize]
            .get(dev, &b.keys, k)?
            .ok_or(Error::NotFound)?;
        let w = word(&v, 2) | DONE;
        crate::format::set_word(&mut v, 2, w);
        b.trees[Class::Apps as usize].insert(dev, &b.keys, k, v)?;
        b.touched = true;
        Ok(())
    }

    /// Run the job's checks, as Alaya, on its side copy (read only: no
    /// read is noted for anyone), and see whether it is still up to date.
    pub fn job_check(&mut self, side: Person) -> Result<Report, Error> {
        let j = self.job(side)?;
        let fork = self.past(j.owner, Class::Documents, j.fork)?;
        let changed = self.side_changes(side)?;
        let mut r = Report::default();
        let fail = |r: &mut Report, check: u64, file: FileId| {
            if let Some(c) = r
                .checks
                .iter_mut()
                .find(|c| c.check == check && c.line == 0 && c.ok)
            {
                c.ok = false;
                c.file = Some(file);
            }
        };
        for bit in [
            check::IN_FOLDER,
            check::KEPT_SAME,
            check::NOTHING_LOST,
            check::NONE_EMPTIED,
        ] {
            if j.rules.checks & bit != 0 {
                r.checks.push(Checked {
                    check: bit,
                    line: 0,
                    ok: true,
                    file: None,
                });
            }
        }
        for &(f, _) in &changed {
            if f.class() != Class::Documents {
                continue;
            }
            let now = match self.stat(side, f) {
                Ok(st) => Some(st),
                Err(Error::NotFound) => None,
                Err(e) => return Err(e),
            };
            let then = match self.past_stat(&fork, f) {
                Ok(st) => Some(st),
                Err(Error::NotFound) => None,
                Err(e) => return Err(e),
            };
            let in_now = now.is_some() && self.inside(side, f, j.rules.folder)?;
            let in_then = then.is_some() && self.inside_past(&fork, f, j.rules.folder)?;
            // Where it is now, and where it was, must both be the folder
            // (a file that is in neither was never the job's to change).
            if (now.is_some() && !in_now) || (then.is_some() && !in_then) {
                fail(&mut r, check::IN_FOLDER, f);
            }
            if j.rules.keep_same.contains(&f) {
                fail(&mut r, check::KEPT_SAME, f);
            }
            if let Some(t) = then {
                if t.is_dir {
                    continue;
                }
                match now {
                    None => fail(&mut r, check::NOTHING_LOST, f),
                    Some(n) if t.size > 0 && n.size == 0 => fail(&mut r, check::NONE_EMPTIED, f),
                    _ => {}
                }
            }
        }
        if let Some(cl) = j.rules.checklist {
            self.run_checklist(&j, &fork, cl, &mut r)?;
        }
        r.stale = self.job_stale(&j, &changed)?;
        Ok(r)
    }

    /// The checklist, read from the approved version; each line checked
    /// on the side copy.
    fn run_checklist(
        &mut self,
        j: &Job,
        fork: &crate::Past,
        cl: FileId,
        r: &mut Report,
    ) -> Result<(), Error> {
        let mut text = alloc::vec![0u8; CHECKLIST_MAX];
        let n = match self.past_read(fork, cl, 0, &mut text) {
            Ok(n) => n,
            Err(Error::NotFound | Error::Forgotten) => {
                r.checks.push(Checked {
                    check: check::CHECKLIST,
                    line: 1,
                    ok: false,
                    file: Some(cl),
                });
                return Ok(());
            }
            Err(e) => return Err(e),
        };
        text.truncate(n);
        let mut count = 0;
        for (i, raw) in text.split(|&b| b == b'\n').enumerate() {
            let Some(line) = parse(raw) else { continue };
            count += 1;
            let line_no = i as u32 + 1;
            if count > CHECKLIST_LINES {
                r.checks.push(Checked {
                    check: check::CHECKLIST,
                    line: line_no,
                    ok: false,
                    file: None,
                });
                break;
            }
            let path = match line {
                Line::Has(p) | Line::Says(p, _) | Line::Not(p, _) | Line::Under(p, _) => p,
                Line::Unknown => &[][..],
            };
            let file = if path.is_empty() {
                None
            } else {
                self.walk_path(j.side, j.rules.folder, path)?
            };
            let ok = match (line, file) {
                (Line::Unknown, _) => false,
                (Line::Has(_), f) => f.is_some(),
                (Line::Says(_, w) | Line::Not(_, w), Some(f)) => {
                    let found = self.side_holds(j.side, f, w)?;
                    found == matches!(line, Line::Says(..))
                }
                (Line::Under(_, max), Some(f)) => {
                    let st = self.stat(j.side, f)?;
                    !st.is_dir && st.size < max
                }
                (_, None) => false,
            };
            r.checks.push(Checked {
                check: check::CHECKLIST,
                line: line_no,
                ok,
                file,
            });
        }
        Ok(())
    }

    /// `a/b/c` below `folder` in branch `b`: never above the folder.
    fn walk_path(
        &mut self,
        b: Person,
        folder: FileId,
        path: &[u8],
    ) -> Result<Option<FileId>, Error> {
        let mut at = folder;
        for part in path.split(|&c| c == b'/') {
            if part.is_empty() || part == b"." {
                continue;
            }
            if part == b".." {
                return Ok(None);
            }
            at = match self.lookup(b, at, part) {
                Ok(f) => f,
                Err(Error::NotFound | Error::BadName | Error::NotADirectory) => return Ok(None),
                Err(e) => return Err(e),
            };
        }
        Ok(Some(at))
    }

    /// Whether a side branch's file holds `words` (either case).
    fn side_holds(&mut self, side: Person, f: FileId, words: &[u8]) -> Result<bool, Error> {
        let st = self.stat(side, f)?;
        if st.is_dir {
            return Ok(false);
        }
        let q: Vec<u8> = words.iter().map(|b| b.to_ascii_lowercase()).collect();
        let mut blk = [0u8; BLOCK];
        let hit = crate::search::scan(st.size, &q, |off, buf| {
            self.peek(side, f, off, buf, &mut blk)
        })?;
        Ok(hit.is_some())
    }

    /// Files the job read, did not change itself, and the person changed
    /// or removed since it started.
    fn job_stale(&mut self, j: &Job, changed: &[(FileId, u64)]) -> Result<Vec<FileId>, Error> {
        let mut read: Vec<FileId> = Vec::new();
        {
            let Store { dev, branches, .. } = self;
            let b = branches.get_mut(&j.side).ok_or(Error::Locked)?;
            b.hist.range(
                dev,
                &b.keys,
                crate::format::Key::MIN,
                Key(crate::H_ACT - 1, u64::MAX),
                &mut |k, v| {
                    if k.0 & 0xff == H_READ {
                        read.push(FileId(word(v, 1)));
                    }
                    true
                },
            )?;
            read.extend(b.reads.iter().map(|r| r.file));
        }
        read.sort();
        read.dedup();
        let mut out = Vec::new();
        for f in read {
            if changed.iter().any(|(g, _)| *g == f) {
                continue;
            }
            match self.stat(j.owner, f) {
                Ok(st) if st.version <= j.fork => {}
                Ok(_) | Err(Error::NotFound) => out.push(f),
                Err(e) => return Err(e),
            }
        }
        Ok(out)
    }

    /// Keep the job's work, as its program, in one commit: only when every
    /// check passed and it is up to date, unless the person keeps it
    /// `anyway` (on record). Conflicts with the person's own changes are
    /// never decided here: see [`Kept::Conflicts`]. The side branch must
    /// be saved first.
    pub fn job_keep(
        &mut self,
        side: Person,
        anyway: bool,
        side_wins: &[FileId],
        owner_wins: &[FileId],
    ) -> Result<Kept, Error> {
        let j = self.job(side)?;
        let report = self.job_check(side)?;
        let passed = report.passed();
        if !passed && !anyway {
            return Ok(Kept::Held(report));
        }
        let extra = if passed { 0 } else { what::ANYWAY };
        match self.side_keep_as(side, j.app, extra, side_wins, owner_wins)? {
            Keep::Conflicts(c) => Ok(Kept::Conflicts(c)),
            Keep::Kept(f) => {
                self.job_forget_rules(j.owner, side)?;
                Ok(Kept::Kept(f))
            }
        }
    }

    /// Throw the job's work away.
    pub fn job_discard(&mut self, side: Person) -> Result<(), Error> {
        let j = self.job(side)?;
        self.side_discard(side)?;
        self.job_forget_rules(j.owner, side)
    }

    fn job_forget_rules(&mut self, p: Person, side: Person) -> Result<(), Error> {
        let id = side & !SIDE_FLAG;
        let Store { dev, branches, .. } = self;
        let b = branches.get_mut(&p).ok_or(Error::Locked)?;
        let t = &mut b.trees[Class::Apps as usize];
        t.remove(dev, &b.keys, k_job(id))?;
        for i in 0..KEEP_SAME_MAX as u64 {
            t.remove(dev, &b.keys, k_job_same(id, i))?;
        }
        b.touched = true;
        Ok(())
    }
}

/// Kept in the checks word of a job's record: the program said it is done.
const DONE: u64 = 1 << 32;

#[cfg(test)]
mod t {
    use super::*;

    #[test]
    fn reads_checklist_lines() {
        assert_eq!(parse(b"  # a note"), None);
        assert_eq!(parse(b""), None);
        assert_eq!(parse(b"has notes/a.txt"), Some(Line::Has(b"notes/a.txt")));
        assert_eq!(
            parse(b"SAYS a.txt  hello world "),
            Some(Line::Says(b"a.txt", b"hello world"))
        );
        assert_eq!(parse(b"not a.txt x"), Some(Line::Not(b"a.txt", b"x")));
        assert_eq!(parse(b"under a.txt 100"), Some(Line::Under(b"a.txt", 100)));
        assert_eq!(parse(b"under a.txt lots"), Some(Line::Unknown));
        assert_eq!(parse(b"has"), Some(Line::Unknown));
        assert_eq!(parse(b"says a.txt"), Some(Line::Unknown));
        assert_eq!(parse(b"maybe a.txt"), Some(Line::Unknown));
    }
}
