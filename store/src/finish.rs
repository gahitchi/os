// SPDX-License-Identifier: GPL-3.0-only
//! Finish, with a work receipt (milestone 7a, step 3; `continuity.md`
//! section 6).
//!
//! Finishing a group is one screen, and one undo:
//!
//! - **Results to keep** stay where they are.
//! - **Temporary things** go to `Archive/<group>` in the person's
//!   documents: moved, never deleted. Alaya only suggests which ones (by
//!   name: `.tmp`, `.bak`, `~`, `scratch`); the person decides each.
//! - **Still used elsewhere**: a file also in another group stays where
//!   it is, whatever its name.
//! - **Permissions to hand back**: what apps were handed of the group's
//!   files (not what they made themselves) is taken back.
//!
//! Then the store writes the **work receipt**, from its own records:
//! each result with its version, the last change the ledger has for it
//! (which app, which moment), what it was made from, what was archived
//! and what was handed back. The receipt is "made from" the exact
//! versions of the results it lists, so like any result it turns "needs
//! a look" when one of them changes: the receipt tells on itself.
//!
//! Everything Finish did is kept in one record, so one undo brings it
//! all back: the files to their folders, the grants to their apps, and
//! the receipt is taken away (the timeline still has it).

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::disk::Disk;
use crate::format::{Key, word, words};
use crate::grants::{Lasts, RECORDS};
use crate::made::{MADE_MAX, Source};
use crate::{App, Class, Error, FileId, Person, Store};

const K_FINISH: u64 = 17;

/// What Finish does with one thing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fate {
    /// A result: stays where it is.
    Keep = 0,
    /// Temporary: moved to the archive (never deleted).
    Archive = 1,
    /// Also in another group: stays where it is.
    Elsewhere = 2,
    /// A grant to hand back.
    HandBack = 3,
}

/// One line of the Finish screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Line {
    pub fate: Fate,
    pub file: FileId,
    /// Elsewhere: the other group. HandBack: the app.
    pub other: u64,
    /// HandBack: the app could change the file.
    pub write: bool,
}

// Record entries (each one 40-byte record, numbered from 0).
const E_HEAD: u64 = 0;
const E_MOVED: u64 = 1;
const E_GRANT: u64 = 2;
const E_RECEIPT: u64 = 3;

fn k_finish(id: u32, n: u64) -> Key {
    Key(RECORDS | K_FINISH, u64::from(id) << 16 | n)
}

/// Alaya's suggestion: the name looks like a temporary thing.
pub fn looks_temporary(name: &[u8]) -> bool {
    let lower: Vec<u8> = name.iter().map(u8::to_ascii_lowercase).collect();
    lower.ends_with(b".tmp")
        || lower.ends_with(b".bak")
        || lower.ends_with(b"~")
        || lower.starts_with(b"~")
        || lower.starts_with(b"scratch")
}

const ARCHIVE: &[u8] = b"Archive";

impl<D: Disk> Store<D> {
    /// The group was finished (and not undone).
    pub fn finished(&mut self, p: Person, id: u32) -> Result<bool, Error> {
        Ok(self.get_rec(p, k_finish(id, 0))?.is_some())
    }

    /// What Finish would do: keep, archive, elsewhere, then hand back.
    pub fn finish_plan(&mut self, p: Person, id: u32) -> Result<Vec<Line>, Error> {
        self.group(p, id)?;
        let files = self.group_files(p, id)?;
        let mut out = Vec::new();
        for &f in &files {
            let Some((name, _)) = self.name_of(p, f, None)? else {
                continue;
            };
            let other = self.groups_of(p, f)?.into_iter().find(|&g| g != id);
            let (fate, other) = match other {
                Some(g) => (Fate::Elsewhere, u64::from(g)),
                None if looks_temporary(&name) => (Fate::Archive, 0),
                None => (Fate::Keep, 0),
            };
            out.push(Line {
                fate,
                file: f,
                other,
                write: false,
            });
        }
        out.sort_by_key(|l| l.fate as u8);
        for g in self.grants(p, None)? {
            if !g.own && files.contains(&g.file) {
                out.push(Line {
                    fate: Fate::HandBack,
                    file: g.file,
                    other: g.app,
                    write: g.write,
                });
            }
        }
        Ok(out)
    }

    /// Finish group `id`: archive `archive` (each one of the group's own
    /// files, not used elsewhere), hand back what apps were handed, and
    /// write the work receipt as `app`. `names` gives an app's name for
    /// the receipt. Returns the receipt.
    pub fn finish(
        &mut self,
        p: Person,
        app: App,
        id: u32,
        archive: &[FileId],
        now: u64,
        names: &dyn Fn(App) -> &'static str,
    ) -> Result<FileId, Error> {
        self.ok()?;
        if self.finished(p, id)? {
            return Err(Error::Exists);
        }
        let g = self.group(p, id)?;
        let plan = self.finish_plan(p, id)?;
        for f in archive {
            if !plan
                .iter()
                .any(|l| l.file == *f && matches!(l.fate, Fate::Keep | Fate::Archive))
            {
                return Err(Error::BadRequest);
            }
        }
        let mut n = 1;
        let mut moved = Vec::new();
        // The temporary things, into Archive/<group>.
        if !archive.is_empty() {
            let docs = self.root(Class::Documents);
            let arch = match self.lookup(p, docs, ARCHIVE) {
                Ok(d) => d,
                Err(Error::NotFound) => self.create(p, app, docs, ARCHIVE, true)?,
                Err(e) => return Err(e),
            };
            let into = match self.lookup(p, arch, &g.name) {
                Ok(d) => d,
                Err(Error::NotFound) => self.create(p, app, arch, &g.name, true)?,
                Err(e) => return Err(e),
            };
            for &f in archive {
                let Some((name, dir)) = self.name_of(p, f, None)? else {
                    continue;
                };
                if self.lookup(p, into, &name).is_ok() {
                    // Something of that name is archived already: this
                    // one stays where it is.
                    continue;
                }
                self.rename(p, app, dir, &name, into, &name)?;
                self.put_rec(p, k_finish(id, n), words(&[E_MOVED, f.0, dir.0]))?;
                n += 1;
                moved.push((f, name));
            }
        }
        // The permissions, handed back.
        let mut back = Vec::new();
        for l in plan.iter().filter(|l| l.fate == Fate::HandBack) {
            let Some(gr) = self
                .grants(p, Some(l.other))?
                .into_iter()
                .find(|x| x.file == l.file && !x.own)
            else {
                continue;
            };
            if self.revoke(p, gr.app, gr.file)? {
                let (kind, arg) = gr.lasts.code();
                self.put_rec(
                    p,
                    k_finish(id, n),
                    words(&[
                        E_GRANT | u64::from(gr.write) << 8 | kind << 16,
                        gr.app,
                        gr.file.0,
                        arg,
                    ]),
                )?;
                n += 1;
                back.push(gr);
            }
        }
        // The receipt, from the records.
        let cn = self.last_commit();
        let mut text = format!(
            "Work receipt: {}\n\
             Finished at moment {cn}. Written by Alaya from its own records.\n",
            String::from_utf8_lossy(&g.name)
        );
        let mut sources = Vec::new();
        let kept: Vec<FileId> = plan
            .iter()
            .filter(|l| l.fate == Fate::Keep && !archive.contains(&l.file))
            .map(|l| l.file)
            .collect();
        text.push_str(&format!("\nKept ({}):\n", kept.len()));
        for &f in &kept {
            let name = self.name_text(p, f)?;
            let st = self.stat(p, f)?;
            if st.is_dir {
                text.push_str(&format!("- {name} (a folder)\n"));
                continue;
            }
            let story = self.story(p, f, 1)?;
            match story.changes.first() {
                Some(&(at, _, by, _)) => text.push_str(&format!(
                    "- {name}, version {}: last changed by {} at moment {at}\n",
                    st.version,
                    names(by)
                )),
                None => text.push_str(&format!("- {name}, version {}\n", st.version)),
            }
            for (s, _) in self.sources(p, f)? {
                let src = self.name_text(p, s.file)?;
                text.push_str(&format!("  made from {src}, version {}\n", s.version));
            }
            if sources.len() < MADE_MAX && f.class() == Class::Documents {
                sources.push(Source {
                    file: f,
                    version: st.version,
                    edits: st.edits,
                });
            }
        }
        if !moved.is_empty() {
            text.push_str(&format!(
                "\nArchived, not deleted ({}), in Archive/{}:\n",
                moved.len(),
                String::from_utf8_lossy(&g.name)
            ));
            for (_, name) in &moved {
                text.push_str(&format!("- {}\n", String::from_utf8_lossy(name)));
            }
        }
        let elsewhere: Vec<&Line> = plan.iter().filter(|l| l.fate == Fate::Elsewhere).collect();
        if !elsewhere.is_empty() {
            text.push_str(&format!(
                "\nStill used elsewhere, left where they are ({}):\n",
                elsewhere.len()
            ));
            for l in elsewhere {
                let name = self.name_text(p, l.file)?;
                let other = self
                    .group(p, l.other as u32)
                    .map(|o| String::from_utf8_lossy(&o.name).into_owned())
                    .unwrap_or_default();
                text.push_str(&format!("- {name}, also in {other}\n"));
            }
        }
        if !back.is_empty() {
            text.push_str(&format!("\nHanded back ({}):\n", back.len()));
            for gr in &back {
                let name = self.name_text(p, gr.file)?;
                text.push_str(&format!(
                    "- {} could {} {name}\n",
                    names(gr.app),
                    if gr.write { "change" } else { "read" }
                ));
            }
        }
        text.push_str(
            "\nThis receipt follows the versions above: if one of them changes, \
             it says \"needs a look\".\n",
        );
        let docs = self.root(Class::Documents);
        let mut i = 1;
        let receipt = loop {
            let name = if i == 1 {
                format!("{} receipt.txt", String::from_utf8_lossy(&g.name))
            } else {
                format!("{} receipt {i}.txt", String::from_utf8_lossy(&g.name))
            };
            match self.create(p, app, docs, name.as_bytes(), false) {
                Ok(f) => break f,
                Err(Error::Exists) if i < 64 => i += 1,
                Err(e) => return Err(e),
            }
        };
        self.write(p, app, receipt, 0, text.as_bytes())?;
        if !sources.is_empty() {
            self.made_from_with(p, app, receipt, &sources, true)?;
        }
        self.group_add(p, id, receipt)?;
        self.put_rec(p, k_finish(id, n), words(&[E_RECEIPT, receipt.0]))?;
        n += 1;
        self.put_rec(p, k_finish(id, 0), words(&[E_HEAD, n, now]))?;
        Ok(receipt)
    }

    /// Undo Finish: the archived files go back to their folders, the
    /// grants back to their apps, and the receipt is taken away.
    pub fn finish_undo(&mut self, p: Person, app: App, id: u32) -> Result<(), Error> {
        self.ok()?;
        let Some(head) = self.get_rec(p, k_finish(id, 0))? else {
            return Err(Error::NotFound);
        };
        let n = word(&head, 1);
        for i in (1..n).rev() {
            let Some(e) = self.get_rec(p, k_finish(id, i))? else {
                continue;
            };
            let w0 = word(&e, 0);
            match w0 & 0xff {
                E_MOVED => {
                    let f = FileId(word(&e, 1));
                    let dir = FileId(word(&e, 2));
                    if let Some((name, now_in)) = self.name_of(p, f, None)?
                        && now_in != dir
                        && self.lookup(p, dir, &name).is_err()
                    {
                        self.rename(p, app, now_in, &name, dir, &name)?;
                    }
                }
                E_GRANT => {
                    let write = w0 >> 8 & 1 != 0;
                    let lasts =
                        Lasts::from_code(w0 >> 16 & 0xff, word(&e, 3)).ok_or(Error::BadRequest)?;
                    let _ = self.grant(p, word(&e, 1), FileId(word(&e, 2)), write, lasts);
                }
                E_RECEIPT => {
                    let f = FileId(word(&e, 1));
                    self.group_take(p, id, f)?;
                    if let Some((name, dir)) = self.name_of(p, f, None)? {
                        self.remove(p, app, dir, &name)?;
                    }
                }
                _ => {}
            }
            self.del_rec(p, k_finish(id, i))?;
        }
        self.del_rec(p, k_finish(id, 0))?;
        Ok(())
    }

    fn name_text(&mut self, p: Person, f: FileId) -> Result<String, Error> {
        Ok(self
            .name_of(p, f, None)?
            .map(|(n, _)| String::from_utf8_lossy(&n).into_owned())
            .unwrap_or_else(|| String::from("(gone)")))
    }
}
