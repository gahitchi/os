// SPDX-License-Identifier: GPL-3.0-only
//! Search (milestone 7, step 2): a person's documents by name and text,
//! and, new here, by what they *used to* say.
//!
//! - **No index.** The store looks through the files when asked, so there
//!   is no second copy of anyone's words to keep locked, to leak, or to
//!   go out of date. At desktop sizes that is fast enough; a locked
//!   index can come later without changing what search answers.
//! - **Search remembers.** When a file no longer holds the words, search
//!   looks back through the moments history keeps: "this was in
//!   letter.txt on Tuesday" (or in a file removed since). Forgotten
//!   files are never found: their key is gone.
//! - **Only the desktop asks.** An app never searches; the person picks a
//!   result and the desktop hands that one file over (a grant), so a
//!   search never shows an app more than the person gives it.
//! - Searching reads nothing on any app's behalf: it leaves no "read by"
//!   notes.

use alloc::collections::BTreeSet;
use alloc::vec::Vec;

use crate::disk::Disk;
use crate::format::{BLOCK, Block};
use crate::{Class, Error, FileId, Moment, Person, Store, what};

/// How much of one file search looks through.
pub const SCAN_MAX: u64 = 8 << 20;
/// How deep into folders search goes.
const DEPTH: usize = 16;
/// How long a found line may be.
pub const SNIPPET: usize = 60;

/// One file search found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    pub file: FileId,
    /// Its name and folder (at the moment it was found in).
    pub name: Vec<u8>,
    pub dir: FileId,
    /// Where the words start, or `None` when its name matched.
    pub at: Option<u64>,
    /// The line around the words.
    pub line: Vec<u8>,
    /// Found only in an older version: the moment that version is from.
    pub past: Option<Moment>,
    /// The file is no longer there (removed since; it can be put back).
    pub gone: bool,
}

/// Lower-case ASCII.
fn low(b: u8) -> u8 {
    b.to_ascii_lowercase()
}

fn matches(hay: &[u8], q: &[u8]) -> Option<usize> {
    if q.is_empty() || hay.len() < q.len() {
        return None;
    }
    (0..=hay.len() - q.len()).find(|&i| {
        hay[i..i + q.len()]
            .iter()
            .zip(q)
            .all(|(&a, &b)| low(a) == b)
    })
}

/// Look for `q` (already lower case) in a file of `size` bytes, reading
/// with `read(offset, buf)`. Returns where it starts and its line.
fn scan(
    size: u64,
    q: &[u8],
    mut read: impl FnMut(u64, &mut [u8]) -> Result<usize, Error>,
) -> Result<Option<(u64, Vec<u8>)>, Error> {
    const CHUNK: usize = 16 * BLOCK;
    let size = size.min(SCAN_MAX);
    let keep = q.len().saturating_sub(1);
    let mut buf = alloc::vec![0u8; CHUNK + keep];
    let mut have = 0usize; // bytes carried over from the last chunk
    let mut base = 0u64; // file offset of buf[0]
    while base + (have as u64) < size {
        let want = (CHUNK as u64).min(size - base - have as u64) as usize;
        let n = read(base + have as u64, &mut buf[have..have + want])?;
        if n == 0 {
            break;
        }
        let len = have + n;
        if let Some(i) = matches(&buf[..len], q) {
            let at = base + i as u64;
            // The line: back to the last newline (at most a little), then
            // forward to the next one.
            let from = buf[..i]
                .iter()
                .rposition(|&b| b == b'\n')
                .map_or(0, |p| p + 1)
                .max(i.saturating_sub(SNIPPET / 3));
            let mut to = (from + SNIPPET).min(len);
            if let Some(nl) = buf[i..to].iter().position(|&b| b == b'\n') {
                to = i + nl;
            }
            let line = buf[from..to]
                .iter()
                .map(|&b| {
                    if b.is_ascii_graphic() || b == b' ' {
                        b
                    } else {
                        b' '
                    }
                })
                .collect();
            return Ok(Some((at, line)));
        }
        let carry = keep.min(len);
        buf.copy_within(len - carry..len, 0);
        base += (len - carry) as u64;
        have = carry;
    }
    Ok(None)
}

impl<D: Disk> Store<D> {
    /// Search person `p`'s documents for `words` (letters match in either
    /// case): names first, then text, then (when `moments` is not zero)
    /// what older versions said, looking back through at most `moments`
    /// kept moments. At most `max` results.
    pub fn search(
        &mut self,
        p: Person,
        words: &[u8],
        max: usize,
        moments: usize,
    ) -> Result<Vec<Found>, Error> {
        let q: Vec<u8> = words.iter().map(|&b| low(b)).collect();
        if q.is_empty() || q.len() > 64 {
            return Err(Error::BadRequest);
        }
        // Every document now, with its folder and name.
        let mut files = Vec::new();
        let mut dirs = alloc::vec![(self.root(Class::Documents), 0usize)];
        while let Some((d, depth)) = dirs.pop() {
            for (name, f) in self.list(p, d)? {
                let st = self.stat(p, f)?;
                if st.is_dir {
                    if depth + 1 < DEPTH {
                        dirs.push((f, depth + 1));
                    }
                } else {
                    files.push((f, d, name, st.size));
                }
            }
        }
        files.sort_by(|a, b| a.2.cmp(&b.2));
        let mut out = Vec::new();
        let mut seen = BTreeSet::new();
        for (f, d, name, _) in &files {
            if out.len() < max && matches(name, &q).is_some() {
                seen.insert(f.0);
                out.push(Found {
                    file: *f,
                    name: name.clone(),
                    dir: *d,
                    at: None,
                    line: Vec::new(),
                    past: None,
                    gone: false,
                });
            }
        }
        for (f, d, name, size) in files {
            if out.len() >= max {
                return Ok(out);
            }
            if seen.contains(&f.0) {
                continue;
            }
            let mut blk: Block = [0; BLOCK];
            let hit = scan(size, &q, |off, buf| self.peek(p, f, off, buf, &mut blk))?;
            if let Some((at, line)) = hit {
                seen.insert(f.0);
                out.push(Found {
                    file: f,
                    name,
                    dir: d,
                    at: Some(at),
                    line,
                    past: None,
                    gone: false,
                });
            }
        }
        // What files used to say: newest moment first; each file once.
        let mut kept = self.history(p)?;
        kept.reverse();
        for m in kept.into_iter().take(moments) {
            if out.len() >= max {
                break;
            }
            let Ok(changes) = self.changes(p, m.cn) else {
                continue;
            };
            let Ok(view) = self.past(p, Class::Documents, m.cn) else {
                continue;
            };
            for c in changes {
                if out.len() >= max {
                    break;
                }
                let f = c.file;
                if f.class() != Class::Documents
                    || c.what & (what::CREATED | what::WROTE) == 0
                    || seen.contains(&f.0)
                {
                    continue;
                }
                // A forgotten file's versions cannot be read: skipped.
                let Ok(st) = self.past_stat(&view, f) else {
                    continue;
                };
                if st.is_dir {
                    continue;
                }
                let Ok(hit) = scan(st.size, &q, |off, buf| self.past_read(&view, f, off, buf))
                else {
                    continue;
                };
                let Some((at, line)) = hit else { continue };
                let Ok(Some((name, dir))) = self.name_of(p, f, Some(m.cn)) else {
                    continue;
                };
                seen.insert(f.0);
                let gone = self.name_of(p, f, None).ok().flatten().is_none();
                out.push(Found {
                    file: f,
                    name,
                    dir,
                    at: Some(at),
                    line,
                    past: Some(m),
                    gone,
                });
            }
        }
        Ok(out)
    }

    /// Read a document without noting the read for any app.
    fn peek(
        &mut self,
        p: Person,
        f: FileId,
        off: u64,
        buf: &mut [u8],
        blk: &mut Block,
    ) -> Result<usize, Error> {
        let size = self.stat(p, f)?.size;
        if off >= size {
            return Ok(0);
        }
        let n = (buf.len() as u64).min(size - off) as usize;
        let mut done = 0;
        while done < n {
            let pos = off + done as u64;
            let within = (pos % BLOCK as u64) as usize;
            let take = (BLOCK - within).min(n - done);
            self.read_block_of(p, f, pos / BLOCK as u64, blk)?;
            buf[done..done + take].copy_from_slice(&blk[within..within + take]);
            done += take;
        }
        Ok(n)
    }
}
