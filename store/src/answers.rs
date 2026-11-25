// SPDX-License-Identifier: GPL-3.0-only
//! Answers instead of data (milestone 8, step 3; `answers-and-contracts.md`
//! rules R8 to R10).
//!
//! An app can be given an *answer grant* on a document instead of the
//! document: it may then ask one fixed question about it ("how many
//! lines?", "the total of column 2"), and gets back one number. It never
//! holds the file: an answer grant is not a grant, so reading, listing or
//! copying the file is refused as before.
//!
//! - **R8**: the questions come only from Alaya's fixed list
//!   ([`Question`]). An app never sends code to run on the person's data.
//! - **R9**: each answer grant has a question limit (at most
//!   [`PER_DAY_MAX`] a day), and every answer is written in the person's
//!   history (the ledger), with the app, the file, the question and the
//!   number, so "What happened?" shows it.
//! - **R10**: an answer from a private document is private too: the app
//!   is marked as if it had read the file (decision 27).
//!
//! The answer helper here is the store itself reading the one file asked
//! about; its read is not recorded as the app's read, because the app did
//! not get the data, only the answer (which is recorded instead).

use alloc::vec::Vec;

use crate::disk::Disk;
use crate::format::{Key, word, words};
use crate::grants::RECORDS;
use crate::{App, Class, Error, FileId, H_ANSWER, H_INFO, Person, STORE_APP, Store};

/// An answer grant's record kind in the person's app-files tree.
const K_ANSWER: u64 = 10;

/// The most questions an answer grant may ask in a day.
pub const PER_DAY_MAX: u64 = 10;
/// The most of a document an answer reads (larger ones are answered
/// about their first part, and the answer says so: `whole` is false).
pub const READ_MAX: usize = 256 * 1024;
/// How many past moments [`Store::answers`] looks back through at most.
const SCAN_MAX: u64 = 4096;

/// Alaya's fixed list of questions (R8).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Question {
    /// How many lines.
    Lines,
    /// How many words.
    Words,
    /// The total of one column (1 to 32; columns are split by spaces,
    /// tabs, commas or semicolons), in hundredths: "12.50" counts 1250.
    /// Lines whose column is not a number (a heading) are skipped.
    Total(u8),
}

impl Question {
    pub fn code(self) -> u64 {
        match self {
            Question::Lines => 1,
            Question::Words => 2,
            Question::Total(c) => 3 | u64::from(c) << 8,
        }
    }

    pub fn from_code(c: u64) -> Option<Question> {
        Some(match (c & 0xff, c >> 8) {
            (1, 0) => Question::Lines,
            (2, 0) => Question::Words,
            (3, col @ 1..=32) => Question::Total(col as u8),
            _ => return None,
        })
    }

    /// Ask it about `text`.
    pub fn ask(self, text: &[u8]) -> i64 {
        let lines = text.split(|&b| b == b'\n').filter(|l| !l.is_empty());
        match self {
            Question::Lines => lines.count() as i64,
            Question::Words => text
                .split(|b| b.is_ascii_whitespace())
                .filter(|w| !w.is_empty())
                .count() as i64,
            Question::Total(col) => lines
                .filter_map(|l| {
                    l.split(|&b| matches!(b, b' ' | b'\t' | b',' | b';' | b'\r'))
                        .filter(|f| !f.is_empty())
                        .nth(usize::from(col) - 1)
                        .and_then(hundredths)
                })
                .fold(0i64, |a, n| a.saturating_add(n)),
        }
    }
}

/// "-12.5" → -1250; None when it is not a plain number. Digits past the
/// second decimal are dropped.
fn hundredths(f: &[u8]) -> Option<i64> {
    let (neg, f) = match f.first() {
        Some(b'-') => (true, &f[1..]),
        Some(b'+') => (false, &f[1..]),
        _ => (false, f),
    };
    let mut parts = f.splitn(2, |&b| b == b'.');
    let whole = parts.next()?;
    let frac = parts.next().unwrap_or(&[]);
    if whole.is_empty() && frac.is_empty() {
        return None;
    }
    let mut n: i64 = 0;
    for &d in whole {
        if !d.is_ascii_digit() {
            return None;
        }
        n = n.saturating_mul(10).saturating_add(i64::from(d - b'0'));
    }
    let mut cents = 0;
    for (i, &d) in frac.iter().enumerate() {
        if !d.is_ascii_digit() {
            return None;
        }
        if i < 2 {
            cents += i64::from(d - b'0') * if i == 0 { 10 } else { 1 };
        }
    }
    let v = n.saturating_mul(100).saturating_add(cents);
    Some(if neg { -v } else { v })
}

/// One answer grant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AnswerGrant {
    pub app: App,
    pub file: FileId,
    pub question: Question,
    pub per_day: u64,
    /// The day (seconds / 86400) `used` counts for.
    pub day: u64,
    pub used: u64,
    /// Every answer it was ever given.
    pub asked: u64,
}

/// One answer given, as the ledger keeps it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Answered {
    pub app: App,
    pub file: FileId,
    pub question: Question,
    pub value: i64,
}

/// What an app gets back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Answer {
    pub value: i64,
    /// Questions it may still ask today.
    pub left: u64,
    /// The whole document was read (it is not larger than [`READ_MAX`]).
    pub whole: bool,
}

/// A record is at most five words: (file, question, per day | used << 8,
/// day, asked).
fn k_answer(app: App, obj: u64) -> Key {
    Key(RECORDS | app << 8 | K_ANSWER, obj)
}

fn decode(app: App, v: &[u8]) -> Option<AnswerGrant> {
    Some(AnswerGrant {
        app,
        file: FileId(word(v, 0)),
        question: Question::from_code(word(v, 1))?,
        per_day: word(v, 2) & 0xff,
        used: word(v, 2) >> 8,
        day: word(v, 3),
        asked: word(v, 4),
    })
}

impl<D: Disk> Store<D> {
    /// Let `app` ask `question` about `file`, at most `per_day` times a
    /// day (the person chose it: the desktop only). A new answer grant
    /// for the same file replaces the old one, and its count starts again
    /// only on a new day.
    pub fn grant_answer(
        &mut self,
        p: Person,
        app: App,
        file: FileId,
        question: Question,
        per_day: u64,
    ) -> Result<(), Error> {
        self.ok()?;
        if file.class() != Class::Documents
            || app == 0
            || app >= 1 << 48
            || !(1..=PER_DAY_MAX).contains(&per_day)
        {
            return Err(Error::BadRequest);
        }
        if self.stat(p, file)?.is_dir {
            return Err(Error::IsADirectory);
        }
        let old = self.answer_grant(p, app, file)?;
        let (day, used, asked) = old.map_or((0, 0, 0), |g| (g.day, g.used, g.asked));
        self.put_answer(
            p,
            AnswerGrant {
                app,
                file,
                question,
                per_day,
                day,
                used,
                asked,
            },
        )
    }

    fn put_answer(&mut self, p: Person, g: AnswerGrant) -> Result<(), Error> {
        self.room(2, false)?;
        let Store { dev, branches, .. } = self;
        let b = branches.get_mut(&p).ok_or(Error::Locked)?;
        b.trees[Class::Apps as usize].insert(
            dev,
            &b.keys,
            k_answer(g.app, g.file.obj()),
            words(&[
                g.file.0,
                g.question.code(),
                g.per_day | g.used << 8,
                g.day,
                g.asked,
            ]),
        )?;
        b.touched = true;
        Ok(())
    }

    /// Take an answer grant back.
    pub fn revoke_answer(&mut self, p: Person, app: App, file: FileId) -> Result<bool, Error> {
        self.ok()?;
        let Store { dev, branches, .. } = self;
        let b = branches.get_mut(&p).ok_or(Error::Locked)?;
        let gone = b.trees[Class::Apps as usize]
            .remove(dev, &b.keys, k_answer(app, file.obj()))?
            .is_some();
        b.touched |= gone;
        Ok(gone)
    }

    pub fn answer_grant(
        &mut self,
        p: Person,
        app: App,
        file: FileId,
    ) -> Result<Option<AnswerGrant>, Error> {
        let Store { dev, branches, .. } = self;
        let b = branches.get_mut(&p).ok_or(Error::Locked)?;
        Ok(b.trees[Class::Apps as usize]
            .get(dev, &b.keys, k_answer(app, file.obj()))?
            .and_then(|v| decode(app, &v)))
    }

    /// The answer grants of one app, or of every app.
    pub fn answer_grants(
        &mut self,
        p: Person,
        app: Option<App>,
    ) -> Result<Vec<AnswerGrant>, Error> {
        let Store { dev, branches, .. } = self;
        let b = branches.get_mut(&p).ok_or(Error::Locked)?;
        let t = &b.trees[Class::Apps as usize];
        let (from, to) = match app {
            Some(a) => (k_answer(a, 0), k_answer(a, u64::MAX)),
            None => (Key(RECORDS, 0), Key(u64::MAX, u64::MAX)),
        };
        Ok(t.collect(dev, &b.keys, from, to)?
            .into_iter()
            .filter(|(k, _)| k.0 & 0xff == K_ANSWER)
            .filter_map(|(k, v)| decode((k.0 & !RECORDS) >> 8, &v))
            .collect())
    }

    /// `app` asks its question about `file` at time `now` (seconds).
    /// Refused with [`Error::NotFound`] without an answer grant, and with
    /// [`Error::UsedUp`] once today's questions are used up.
    pub fn answer(&mut self, p: Person, app: App, file: FileId, now: u64) -> Result<Answer, Error> {
        self.ok()?;
        let mut g = self.answer_grant(p, app, file)?.ok_or(Error::NotFound)?;
        let day = now / 86_400;
        if g.day != day {
            g.day = day;
            g.used = 0;
        }
        if g.used >= g.per_day {
            return Err(Error::UsedUp);
        }
        let size = self.stat(p, file)?.size;
        let mut buf = alloc::vec![0u8; (size as usize).min(READ_MAX)];
        // The helper's read is not the app's: the ledger records the
        // answer instead.
        let track = core::mem::replace(&mut self.track_reads, false);
        let got = self.read(p, STORE_APP, file, 0, &mut buf);
        self.track_reads = track;
        buf.truncate(got?);
        let value = g.question.ask(&buf);
        g.used += 1;
        g.asked += 1;
        self.put_answer(p, g)?;
        // R10: an answer from a private document marks the app.
        self.note_private_read(p, app, file, now)?;
        let b = self.branch(p)?;
        b.answered.push(Answered {
            app,
            file,
            question: g.question,
            value,
        });
        b.touched = true;
        Ok(Answer {
            value,
            left: g.per_day - g.used,
            whole: size as usize <= READ_MAX,
        })
    }

    /// Answers given, newest first, in moments below `before` (0: from the
    /// newest): (moment, time, the answer). Answers not committed yet are
    /// not in it.
    pub fn answers(
        &mut self,
        p: Person,
        before: u64,
        max: usize,
    ) -> Result<Vec<(u64, u64, Answered)>, Error> {
        let Store { dev, branches, .. } = self;
        let b = branches.get_mut(&p).ok_or(Error::Locked)?;
        let mut out = Vec::new();
        let mut hi = if before == 0 {
            crate::GEN_MAX
        } else {
            before - 1
        };
        for _ in 0..SCAN_MAX {
            if out.len() >= max {
                break;
            }
            let Some((k, _)) =
                b.hist
                    .last_in(dev, &b.keys, Key::MIN, Key(hi << 8 | 0xff, u64::MAX))?
            else {
                break;
            };
            let cn = k.0 >> 8;
            let mut time = 0;
            let mut here = Vec::new();
            b.hist.range(
                dev,
                &b.keys,
                Key(cn << 8 | H_ANSWER, 0),
                Key(cn << 8 | H_ANSWER, u64::MAX),
                &mut |k, v| {
                    if k.0 & 0xff == H_ANSWER
                        && let Some(q) = Question::from_code(word(v, 2))
                    {
                        here.push(Answered {
                            app: word(v, 0),
                            file: FileId(word(v, 1)),
                            question: q,
                            value: word(v, 3) as i64,
                        });
                    }
                    true
                },
            )?;
            if !here.is_empty() {
                if let Some(v) = b.hist.get(dev, &b.keys, Key(cn << 8 | H_INFO, 0))? {
                    time = word(&v, 0);
                }
                for a in here.into_iter().rev() {
                    if out.len() < max {
                        out.push((cn, time, a));
                    }
                }
            }
            if cn == 0 {
                break;
            }
            hi = cn - 1;
        }
        Ok(out)
    }
}
