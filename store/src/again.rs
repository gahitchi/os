// SPDX-License-Identifier: GPL-3.0-only
//! "Run it again" (milestone 7, step 3; rule B2): what an app had read by
//! a past moment, each file at exactly the version it read, so the app
//! can be opened again on those versions (read only) and show what it
//! showed then.
//!
//! Exact while history keeps a moment holding that version. After
//! thinning, the nearest kept moment that has the file stands in, and
//! the answer says so: it is never passed off as exact.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use crate::disk::Disk;
use crate::format::{Key, word};
use crate::{App, Class, Error, FileId, H_READ, Person, Store, k_hist};

/// One file an app read, and where to find that version.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReadThen {
    pub file: FileId,
    /// The version it read (the commit that made it).
    pub version: u64,
    /// The kept moment to read it from.
    pub moment: u64,
    /// That moment holds exactly the version read.
    pub exact: bool,
}

impl<D: Disk> Store<D> {
    /// Every document `app` had read by commit `cn` (the newest read of
    /// each), with the kept moment that holds that version. Files history
    /// no longer has (forgotten, or older than every kept moment of them)
    /// are left out.
    pub fn read_then(&mut self, p: Person, app: App, cn: u64) -> Result<Vec<ReadThen>, Error> {
        let mut newest: BTreeMap<FileId, (u64, u64)> = BTreeMap::new();
        {
            let Store { dev, branches, .. } = self;
            let b = branches.get_mut(&p).ok_or(Error::Locked)?;
            b.hist.range(
                dev,
                &b.keys,
                Key::MIN,
                k_hist(cn, H_READ, u64::MAX),
                &mut |k, v| {
                    if k.0 & 0xff == H_READ && word(v, 0) == app {
                        let at = k.0 >> 8;
                        let f = FileId(word(v, 1));
                        let e = newest.entry(f).or_insert((0, 0));
                        if at >= e.0 {
                            *e = (at, word(v, 2));
                        }
                    }
                    true
                },
            )?;
        }
        let moments = self.history(p)?;
        let mut out = Vec::new();
        for (f, (_, version)) in newest {
            if f.class() != Class::Documents || self.file_keys(p, f.obj()).is_err() {
                continue;
            }
            if let Some((moment, exact)) = self.moment_of(p, f, version, &moments)? {
                out.push(ReadThen {
                    file: f,
                    version,
                    moment,
                    exact,
                });
            }
        }
        Ok(out)
    }

    /// The kept moment that holds version `version` of `f` (exact), or the
    /// nearest kept one that has the file at all (not exact).
    pub(crate) fn moment_of(
        &mut self,
        p: Person,
        f: FileId,
        version: u64,
        moments: &[crate::Moment],
    ) -> Result<Option<(u64, bool)>, Error> {
        let mut best: Option<(u64, u64)> = None;
        for m in moments {
            let Ok(view) = self.past(p, Class::Documents, m.cn) else {
                continue;
            };
            let Ok(st) = self.past_stat(&view, f) else {
                continue;
            };
            if st.is_dir {
                return Ok(None);
            }
            if st.version == version {
                return Ok(Some((m.cn, true)));
            }
            let d = m.cn.abs_diff(version);
            if best.is_none_or(|(bd, _)| d < bd) {
                best = Some((d, m.cn));
            }
        }
        Ok(best.map(|(_, m)| (m, false)))
    }
}
