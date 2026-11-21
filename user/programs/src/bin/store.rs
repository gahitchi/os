// SPDX-License-Identifier: GPL-3.0-only
//! The store server (milestone 5): the Alaya store on the real disk,
//! through the driver keeper, for apps that talk to it by messages.
//!
//! One thread serves every request in turn. Changes are gathered and saved
//! every five seconds (choice A), or at once when an app asks. If this
//! program crashes, the next one opens the disk at its last saved commit;
//! nothing saved is ever lost or damaged.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec::Vec;

use alaya_programs::files::{self as fs, st};
use alaya_programs::{keeper as k, status};
use alaya_store::disk::{Disk, IoError};
use alaya_store::format::Block;
use alaya_store::{Class, Error, FileId, Options, Store};
use alaya_sys::abi::{StartInfo, call, start};
use alaya_sys::rt::{self, Mem};
use alaya_sys::{Caps, Kind, Msg, MsgInfo, say};

alaya_sys::entry!(main);
alaya_sys::heap!();

/// The machine key for this milestone's tests. The real key comes from
/// the TPM and the person's PIN (decisions 29 and 76).
const MACHINE: [u8; 32] = *b"alaya test machine key, m5 only!";

const TICK_BIT: u64 = 1;

/// The disk, through the keeper: one 4 KiB block at a time, in buffer
/// slot 0 of the pages the keeper shares with this program.
struct KeeperDisk {
    keeper: u64,
    first: u64,
    blocks: u64,
    /// Writes and flushes left before stopping dead (tests only).
    countdown: Option<u64>,
    dying: u64,
    /// A notification nobody signals.
    never: u64,
}

impl KeeperDisk {
    /// A power cut at an exact point: tell the tests, then never run again.
    fn tick(&mut self) {
        match self.countdown {
            Some(0) => {
                let _ = alaya_sys::signal(self.dying, 1);
                loop {
                    let _ = alaya_sys::wait(self.never);
                }
            }
            Some(n) => self.countdown = Some(n - 1),
            None => {}
        }
    }

    fn ask(&self, label: u64, words: &[u64]) -> Result<(), IoError> {
        let m = alaya_sys::ipc_call(self.keeper, &Msg::new(label, words), Caps::default())
            .map_err(|_| IoError::Failed)?;
        match m.words[0] {
            status::OK => Ok(()),
            _ => Err(IoError::Failed),
        }
    }
}

impl Disk for KeeperDisk {
    fn blocks(&self) -> u64 {
        self.blocks
    }

    fn read(&mut self, addr: u64, buf: &mut Block) -> Result<(), IoError> {
        self.ask(k::BREAD, &[self.first + addr, 0])?;
        // SAFETY: slot 0 of the shared buffer pages, mapped here.
        unsafe { core::ptr::copy_nonoverlapping(fs::BUF as *const u8, buf.as_mut_ptr(), 4096) };
        Ok(())
    }

    fn write(&mut self, addr: u64, buf: &Block) -> Result<(), IoError> {
        self.tick();
        // SAFETY: as in `read`.
        unsafe { core::ptr::copy_nonoverlapping(buf.as_ptr(), fs::BUF as *mut u8, 4096) };
        self.ask(k::BWRITE, &[self.first + addr, 0])
    }

    fn flush(&mut self) -> Result<(), IoError> {
        self.tick();
        self.ask(k::FLUSH, &[])
    }
}

fn code(e: Error) -> u64 {
    match e {
        Error::NotFound => st::NOT_FOUND,
        Error::Exists => st::EXISTS,
        Error::Full => st::FULL,
        Error::Locked => st::LOCKED,
        Error::WrongKey => st::WRONG_KEY,
        Error::Damaged(_) => st::DAMAGED,
        Error::Io(_) | Error::MustReopen => st::IO,
        Error::NoSuchMoment => st::NO_SUCH_MOMENT,
        Error::BadRequest | Error::BadName | Error::NotADirectory | Error::IsADirectory => {
            st::BAD_REQUEST
        }
        _ => st::OTHER,
    }
}

fn xfer(channel: u64) -> &'static mut [u8] {
    let base = fs::XFER + (channel % fs::CHANNELS) * fs::XFER_PAGES * 4096;
    // SAFETY: the transfer pages are mapped here, writable; requests are
    // served one at a time.
    unsafe { core::slice::from_raw_parts_mut(base as *mut u8, fs::XFER_BYTES) }
}

fn secret(w: &[u64]) -> [u8; 32] {
    let mut s = [0u8; 32];
    for i in 0..4 {
        s[8 * i..8 * i + 8].copy_from_slice(&w[i].to_le_bytes());
    }
    s
}

fn put_words(buf: &mut [u8], at: usize, w: &[u64]) {
    for (i, x) in w.iter().enumerate() {
        buf[at + 8 * i..at + 8 * i + 8].copy_from_slice(&x.to_le_bytes());
    }
}

fn main(info: &'static StartInfo) -> ! {
    let g = start::FIRST_GIVEN;
    let mut mem = Mem::new(info);
    alaya_sys::heap::init(&mut mem, 4).expect("heap");
    let ep = g + fs::G_ENDPOINT;
    let keeper = g + fs::G_KEEPER;
    let me = mem.make(Kind::Notification, 0).expect("notification");
    alaya_sys::thread_bind(start::THREAD, me).expect("bind");
    let reply = mem.make(Kind::Reply, 0).expect("reply");
    rt::start_clock(&mut mem, me, TICK_BIT, 1, 150).expect("clock");

    let m =
        alaya_sys::ipc_call(keeper, &Msg::new(k::BLOCKS, &[]), Caps::default()).expect("keeper");
    let limit = info.args[fs::ARG_BLOCKS];
    let blocks = if limit != 0 {
        m.words[1].min(limit)
    } else {
        m.words[1]
    };
    let dying = g + fs::G_DYING;
    let never = mem.make(Kind::Notification, 0).expect("notification");
    let disk = || KeeperDisk {
        keeper,
        first: m.words[0],
        blocks,
        countdown: None,
        dying,
        never,
    };
    let now = || rt::now_ms() / 1000;
    let format = info.args[fs::ARG_FLAGS] & fs::FLAG_FORMAT != 0;
    let mut store = match format {
        true => Store::format(disk(), &MACHINE, Options::default()).expect("format"),
        false => match Store::open(disk(), &MACHINE) {
            Ok(s) => s,
            // A fresh disk.
            Err(Error::NotAStore) => {
                Store::format(disk(), &MACHINE, Options::default()).expect("format")
            }
            Err(e) => panic!("cannot open the store: {e:?}"),
        },
    };
    // About half a MiB of tree nodes kept in memory.
    store.dev.cache_nodes = 128;
    say!(
        "{} the store ({} blocks of 4 KiB) at commit {}",
        if format { "formatted" } else { "opened" },
        blocks,
        store.last_commit()
    );
    let mut last_save = rt::now_ms();
    let mut serve = Server { store: &mut store };
    let mut msg = alaya_sys::msg_call(
        call::RECV,
        ep,
        &Msg::default(),
        Caps {
            reply,
            ..Caps::default()
        },
    );
    let mut last_clean = u64::MAX;
    loop {
        let answer = match msg {
            Ok(m) if m.info.has(MsgInfo::SIGNAL) => {
                // Once a day: empty the caches nobody used for 30 days
                // (choice D). A person's own files are never cleaned.
                let day = now() / (24 * 3600);
                if m.badge & TICK_BIT != 0 && day != last_clean {
                    last_clean = day;
                    let _ = serve.store.clean_caches(now(), fs::CACHE_UNUSED_DAYS);
                }
                if m.badge & TICK_BIT != 0
                    && serve.store.has_changes()
                    && rt::now_ms() - last_save >= fs::AUTOSAVE_MS
                {
                    let _ = serve.store.commit(now());
                    last_save = rt::now_ms();
                }
                None
            }
            Ok(m) => {
                let a = serve.handle(&m, now());
                if m.label() == fs::SAVE {
                    last_save = rt::now_ms();
                }
                Some(a)
            }
            Err(_) => None,
        };
        if serve.store.broken() {
            // A commit failed half way: start again from the disk.
            panic!("the disk failed during a commit");
        }
        msg = match answer {
            Some(a) => alaya_sys::msg_call(
                call::REPLY_RECV,
                ep,
                &a,
                Caps {
                    reply,
                    ..Caps::default()
                },
            ),
            None => alaya_sys::msg_call(
                call::RECV,
                ep,
                &Msg::default(),
                Caps {
                    reply,
                    ..Caps::default()
                },
            ),
        };
    }
}

struct Server<'a> {
    store: &'a mut Store<KeeperDisk>,
}

impl Server<'_> {
    fn handle(&mut self, m: &Msg, now: u64) -> Msg {
        let person = m.badge >> 48;
        let app = (m.badge >> 8) & 0xff_ffff_ffff;
        let ch = m.badge & 0xff;
        let w = m.words;
        let r = self.op(m.label(), person, app, ch, &w, now);
        match r {
            Ok(words) => {
                let mut out = [0u64; 6];
                out[1..1 + words.len()].copy_from_slice(&words);
                Msg::new(0, &out[..1 + words.len()])
            }
            Err(e) => Msg::new(0, &[code(e)]),
        }
    }

    fn name(ch: u64, len: u64) -> Result<Vec<u8>, Error> {
        if len == 0 || len > alaya_store::NAME_MAX as u64 {
            return Err(Error::BadName);
        }
        Ok(xfer(ch)[..len as usize].to_vec())
    }

    fn op(
        &mut self,
        label: u64,
        p: u64,
        app: u64,
        ch: u64,
        w: &[u64; 6],
        now: u64,
    ) -> Result<Vec<u64>, Error> {
        let s = &mut *self.store;
        let len = |n: u64| (n as usize).min(fs::XFER_BYTES);
        Ok(match label {
            fs::UNLOCK => {
                s.unlock(p, &secret(&w[..4]))?;
                alloc::vec![]
            }
            fs::ADD_PERSON => {
                s.add_person(p, &secret(&w[..4]))?;
                alloc::vec![]
            }
            fs::ROOT => {
                let c = Class::from(w[0]).ok_or(Error::BadRequest)?;
                alloc::vec![s.root(c).0]
            }
            fs::CREATE => {
                let name = Self::name(ch, w[2])?;
                alloc::vec![s.create(p, app, FileId(w[0]), &name, w[1] != 0)?.0]
            }
            fs::LOOKUP => {
                let name = Self::name(ch, w[1])?;
                alloc::vec![s.lookup(p, FileId(w[0]), &name)?.0]
            }
            fs::WRITE => {
                let n = len(w[2]);
                s.write(p, app, FileId(w[0]), w[1], &xfer(ch)[..n])?;
                alloc::vec![]
            }
            fs::READ => {
                let n = len(w[2]);
                alloc::vec![s.read(p, app, FileId(w[0]), w[1], &mut xfer(ch)[..n])? as u64]
            }
            fs::STAT => {
                let st = s.stat(p, FileId(w[0]))?;
                alloc::vec![st.size, st.version, st.is_dir as u64]
            }
            fs::REMOVE => {
                let name = Self::name(ch, w[1])?;
                s.remove(p, app, FileId(w[0]), &name)?;
                alloc::vec![]
            }
            fs::TRUNCATE => {
                s.truncate(p, app, FileId(w[0]), w[1])?;
                alloc::vec![]
            }
            fs::SAVE => alloc::vec![s.commit(now)?],
            fs::HISTORY => {
                let h = s.history(p)?;
                let buf = xfer(ch);
                let n = h.len().min(fs::XFER_BYTES / 16);
                for (i, m) in h.iter().rev().take(n).enumerate() {
                    put_words(buf, 16 * i, &[m.cn, m.time]);
                }
                alloc::vec![n as u64, h.len() as u64]
            }
            fs::CHANGES => {
                let c = s.changes(p, w[0])?;
                let buf = xfer(ch);
                let n = c.len().min(fs::XFER_BYTES / 24);
                for (i, x) in c.iter().take(n).enumerate() {
                    put_words(buf, 24 * i, &[x.app, x.file.0, x.what]);
                }
                alloc::vec![n as u64]
            }
            fs::READS => {
                let c = s.reads(p, w[0])?;
                let buf = xfer(ch);
                let n = c.len().min(fs::XFER_BYTES / 24);
                for (i, x) in c.iter().take(n).enumerate() {
                    put_words(buf, 24 * i, &[x.app, x.file.0, x.version]);
                }
                alloc::vec![n as u64]
            }
            fs::PAST_READ => {
                let class = FileId(w[1]).class();
                let past = s.past(p, class, w[0])?;
                let n = len(w[3]);
                alloc::vec![s.past_read(&past, FileId(w[1]), w[2], &mut xfer(ch)[..n])? as u64]
            }
            fs::PAST_LOOKUP => {
                let dir = FileId(w[1]);
                let past = s.past(p, dir.class(), w[0])?;
                let name = Self::name(ch, w[2])?;
                alloc::vec![s.past_lookup(&past, dir, &name)?.0]
            }
            fs::LIST => {
                let l = s.list(p, FileId(w[0]))?;
                let buf = xfer(ch);
                let n = l.len().min(fs::XFER_BYTES / 48);
                for (i, (name, f)) in l.iter().take(n).enumerate() {
                    let o = 48 * i;
                    put_words(buf, o, &[f.0, name.len() as u64]);
                    buf[o + 16..o + 48].fill(0);
                    buf[o + 16..o + 16 + name.len()].copy_from_slice(name);
                }
                alloc::vec![n as u64]
            }
            fs::CHECK => {
                if s.has_changes() {
                    s.commit(now)?;
                }
                let r = s.check()?;
                alloc::vec![r.problems.len() as u64, r.blocks, r.moments, r.data_blocks]
            }
            fs::STATS => alloc::vec![
                s.dev.stats.commits,
                s.last_commit(),
                s.used_blocks(),
                s.blocks(),
                s.history_blocks(),
            ],
            fs::KEEP_APPS => {
                s.keep_apps(p)?;
                alloc::vec![]
            }
            fs::RELEASE_APPS => {
                s.release_apps(p)?;
                alloc::vec![]
            }
            fs::MAKE_ROOM => {
                if !s.make_room(w[1], w[0])? {
                    return Err(Error::Full);
                }
                alloc::vec![]
            }
            fs::DIE_AFTER => {
                s.dev.disk.countdown = Some(w[0]);
                alloc::vec![]
            }
            fs::TRACK => {
                s.track_reads = w[0] != 0;
                alloc::vec![]
            }
            _ => return Err(Error::BadRequest),
        })
    }
}
