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
use alaya_store::again::ReadThen;
use alaya_store::disk::{Disk, IoError};
use alaya_store::format::Block;
use alaya_store::grants::{Access, Lasts};
use alaya_store::history::{Restored, Undo};
use alaya_store::jobs::Job;
use alaya_store::made::Source;
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
    /// Where the keeper's buffer pages are mapped here.
    buf: u64,
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
        unsafe { core::ptr::copy_nonoverlapping(self.buf as *const u8, buf.as_mut_ptr(), 4096) };
        Ok(())
    }

    fn write(&mut self, addr: u64, buf: &Block) -> Result<(), IoError> {
        self.tick();
        // SAFETY: as in `read`.
        unsafe { core::ptr::copy_nonoverlapping(buf.as_ptr(), self.buf as *mut u8, 4096) };
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
    // The store holds the keys and everyone's files: guarded.
    alaya_sys::space_guard(start::SPACE, true).expect("guard");
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
        buf: fs::BUF,
        first: m.words[0],
        blocks,
        countdown: None,
        dying,
        never,
    };
    let now = || rt::now_ms() / 1000;
    let mut format = info.args[fs::ARG_FLAGS] & fs::FLAG_FORMAT != 0;
    let mut store = match format {
        true => Store::format(disk(), &MACHINE, Options::default()).expect("format"),
        false => match Store::open(disk(), &MACHINE) {
            Ok(s) => s,
            // A fresh disk.
            Err(Error::NotAStore) => {
                format = true;
                Store::format(disk(), &MACHINE, Options::default()).expect("format")
            }
            // A store from an older milestone, in a layout this one does
            // not read. While Alaya is only tested, it starts again
            // empty; a real update moves the files over (milestone 8).
            Err(Error::Format) => {
                say!("the disk holds an older store layout: starting again with an empty store");
                format = true;
                Store::format(disk(), &MACHINE, Options::default()).expect("format")
            }
            Err(e) => panic!("cannot open the store: {e:?}"),
        },
    };
    // New lock keys for files come from a generator stirred with what this
    // machine can give: the processor's random numbers on a PC that has
    // them, and the jitter of its clock.
    let (seed, rdrand) = entropy();
    store.stir(&seed);
    say!(
        "new file keys are stirred with {}",
        if rdrand {
            "the processor's random numbers and clock jitter"
        } else {
            "clock jitter"
        }
    );
    // About half a MiB of tree nodes kept in memory.
    store.dev.cache_nodes = 128;
    say!(
        "{} the store ({} blocks of 4 KiB) at commit {}",
        if format { "formatted" } else { "opened" },
        blocks,
        store.last_commit()
    );
    let mut last_save = rt::now_ms();
    // The backup stick, if there is one: its own keeper, its own pages.
    let stick = (info.args[fs::ARG_FLAGS] & fs::FLAG_STICK != 0)
        .then(|| {
            let sk = g + fs::G_STICK;
            let m = alaya_sys::ipc_call(sk, &Msg::new(k::BLOCKS, &[]), Caps::default()).ok()?;
            say!("a backup stick of {} MiB is there", m.words[1] / 256);
            Some(KeeperDisk {
                keeper: sk,
                buf: fs::STICK_BUF,
                first: m.words[0],
                blocks: m.words[1],
                countdown: None,
                dying,
                never,
            })
        })
        .flatten();
    let mut serve = Server {
        store: &mut store,
        hold_until: 0,
        stick,
        jobs: None,
        replay: Vec::new(),
        clips: Vec::new(),
    };
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
    let mut last_auto = now();
    let mut last_forget = 0;
    loop {
        let answer = match msg {
            Ok(m) if m.info.has(MsgInfo::SIGNAL) => {
                // Once a day: empty the caches nobody used for 30 days
                // (choice D). A person's own files are never cleaned.
                let day = now() / (24 * 3600);
                if m.badge & TICK_BIT != 0 && day != last_clean {
                    last_clean = day;
                    let _ = serve.store.clean_caches(now(), fs::CACHE_UNUSED_DAYS);
                    // Thin history (decision 33), saved with the next save.
                    let _ = serve.store.thin(now());
                }
                // Documents whose hour is up are forgotten for good
                // (decision 62), and that is saved at once.
                // Once a second is plenty: the grace is an hour, and every
                // look costs a walk of each person's records.
                if m.badge & TICK_BIT != 0 && rt::now_ms() >= last_forget + 1000 {
                    last_forget = rt::now_ms();
                    let mut forgot = 0;
                    for p in serve.store.people().unwrap_or_default() {
                        forgot += serve.store.forget_due(p, now()).unwrap_or(0);
                    }
                    if forgot > 0 && serve.store.commit(now()).is_ok() {
                        say!("forgot {forgot} documents for good: their keys are gone");
                    }
                }
                // Backups by themselves (decision 36): for each person who
                // made one, every ten minutes when something changed.
                if m.badge & TICK_BIT != 0
                    && now() >= last_auto + fs::BACKUP_EVERY
                    && let Some(stick) = serve.stick.as_mut()
                {
                    last_auto = now();
                    for p in serve.store.people().unwrap_or_default() {
                        let Ok(Some((_, _, moment))) = serve.store.last_backup(p) else {
                            continue;
                        };
                        let newest = serve.store.history(p).ok().and_then(|h| h.last().copied());
                        if newest.is_some_and(|d| d.cn > moment) {
                            match serve.store.backup(p, stick, false) {
                                Ok(r) => say!(
                                    "backed up by itself: run {}, {} changed, {} gone",
                                    r.run,
                                    r.written,
                                    r.gone
                                ),
                                Err(e) => say!("the backup by itself did not run: {e:?}"),
                            }
                        }
                    }
                }
                if m.badge & TICK_BIT != 0
                    && serve.store.has_changes()
                    && rt::now_ms() - last_save >= fs::AUTOSAVE_MS
                    && rt::now_ms() >= serve.hold_until
                {
                    let _ = serve.store.commit(now());
                    last_save = rt::now_ms();
                }
                None
            }
            Ok(m) => {
                let a = serve.handle(&m, now());
                if matches!(
                    m.label(),
                    fs::SAVE | fs::RECORD_ACTION | fs::UNDO | fs::UNDO_CHANGE | fs::RESTORE
                ) {
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
    /// An app began an action: no saving by itself until then (ms).
    hold_until: u64,
    /// The backup stick (milestone 7), if there is one.
    stick: Option<KeeperDisk>,
    /// The jobs running now (milestone 7, step 3), by person and app;
    /// None when they must be looked up again.
    jobs: Option<Vec<Job>>,
    /// Apps run again on what they read at a past moment: (person, app,
    /// the files at their versions). Until the app closes.
    replay: Vec<(u64, u64, Vec<ReadThen>)>,
    /// The clipboard of each person: the bytes, and the exact version of
    /// the document they came from, if any.
    clips: Vec<(u64, Vec<u8>, Option<Source>)>,
}

/// Bytes to stir the key generator with, and whether the processor's
/// random numbers were among them.
fn entropy() -> ([u8; 64], bool) {
    let mut h = alaya_crypto::Sha256::new();
    #[cfg_attr(not(target_arch = "x86_64"), allow(unused_mut))]
    let mut rdrand = false;
    #[cfg(target_arch = "x86_64")]
    {
        let has = core::arch::x86_64::__cpuid(1).ecx & (1 << 30) != 0;
        if has {
            for _ in 0..16 {
                let mut x = 0u64;
                // SAFETY: the processor said it has RDRAND.
                if unsafe { core::arch::x86_64::_rdrand64_step(&mut x) } == 1 {
                    h.update(&x.to_le_bytes());
                    rdrand = true;
                }
            }
        }
    }
    // Clock jitter: how long small pieces of work take, many times.
    let mut acc = 0u64;
    for i in 0..4096u64 {
        let t0 = alaya_sys::arch::clock();
        for j in 0..(i % 7 + 1) {
            acc = acc.wrapping_mul(6364136223846793005).wrapping_add(j);
        }
        let d = alaya_sys::arch::clock().wrapping_sub(t0);
        h.update(&(d ^ acc).to_le_bytes());
    }
    h.update(&rt::now_us().to_le_bytes());
    let a = h.finish();
    let mut out = [0u8; 64];
    out[..32].copy_from_slice(&a);
    out[32..].copy_from_slice(&alaya_crypto::sha256(&a));
    (out, rdrand)
}

/// Words from the transfer pages.
fn word_at(buf: &[u8], i: usize) -> u64 {
    u64::from_le_bytes(buf[8 * i..8 * i + 8].try_into().unwrap())
}

impl Server<'_> {
    fn handle(&mut self, m: &Msg, now: u64) -> Msg {
        let person = m.badge >> 48;
        let app = (m.badge >> 8) & 0xff_ffff_ffff;
        let ch = m.badge & 0xff;
        let w = m.words;
        let r = match self.guard(m.label(), person, app, ch, &w, now) {
            Ok(Some(words)) => Ok(words),
            Ok(None) => self.op(m.label(), person, app, ch, &w, now),
            Err(e) => Err(e),
        };
        match r {
            Ok(words) => {
                let mut out = [0u64; 6];
                out[1..1 + words.len()].copy_from_slice(&words);
                Msg::new(0, &out[..1 + words.len()])
            }
            Err(e) => Msg::new(0, &[code(e)]),
        }
    }

    /// Grants (milestone 7, step 2): an app that is not one of Alaya's
    /// own carriers sees, of the person's documents, only what it was
    /// handed and what it made itself. Returns the answer when it answers
    /// the request itself, None to let it through, an error to refuse.
    fn guard(
        &mut self,
        label: u64,
        p: u64,
        app: u64,
        ch: u64,
        w: &[u64; 6],
        now: u64,
    ) -> Result<Option<Vec<u64>>, Error> {
        if !restricted(app) {
            return Ok(None);
        }
        if let Some(job) = self.job_for(p, app) {
            return self.job_guard(label, app, ch, w, &job);
        }
        if let Some(i) = self.replay.iter().position(|r| r.0 == p && r.1 == app) {
            let files = self.replay[i].2.clone();
            return self.replay_guard(label, p, ch, w, &files);
        }
        let s = &mut *self.store;
        let docs = |f: u64| FileId(f).class() == Class::Documents;
        let need = |s: &mut Store<KeeperDisk>, f: u64, write: bool| need(s, p, app, f, write, now);
        match label {
            fs::ROOT
            | fs::SAVE
            | fs::STATS
            | fs::ACTION_BEGIN
            | fs::RECORD_ACTION
            | fs::JOB
            | fs::JOB_DONE => Ok(None),
            fs::WRITE | fs::TRUNCATE | fs::AS_OF => need(s, w[0], true).map(|_| None),
            fs::MARK => need(s, w[0], false).map(|_| None),
            fs::MADE_FROM => {
                need(s, w[0], true)?;
                let n = (w[1] as usize).min(alaya_store::made::MADE_MAX);
                for i in 0..n {
                    // A source it may read, or one the clipboard handed it.
                    let f = word_at(xfer(ch), 3 * i);
                    if !s.was_pasted(p, app, FileId(f))? {
                        need(s, f, false)?;
                    }
                }
                Ok(None)
            }
            fs::COPY => {
                need(s, w[0], false)?;
                s.note_private_read(p, app, FileId(w[0]), now)?;
                Ok(None)
            }
            fs::COPY_TEXT | fs::PASTE => Ok(None),
            fs::READ | fs::STAT => match need(s, w[0], false)? {
                Access::Pinned(cn) => {
                    if label == fs::READ {
                        // Reading a private document marks the app
                        // (decision 27).
                        s.note_private_read(p, app, FileId(w[0]), now)?;
                    }
                    let past = s.past(p, Class::Documents, cn)?;
                    let f = FileId(w[0]);
                    if label == fs::STAT {
                        let st = s.past_stat(&past, f)?;
                        Ok(Some(alloc::vec![
                            st.size,
                            st.version,
                            st.is_dir as u64,
                            st.edits
                        ]))
                    } else {
                        let n = (w[2] as usize).min(fs::XFER_BYTES);
                        let got = s.past_read(&past, f, w[1], &mut xfer(ch)[..n])?;
                        Ok(Some(alloc::vec![got as u64]))
                    }
                }
                _ => {
                    if label == fs::READ {
                        s.note_private_read(p, app, FileId(w[0]), now)?;
                    }
                    Ok(None)
                }
            },
            fs::CREATE => {
                if !docs(w[0]) {
                    return Ok(None);
                }
                // Only in the documents' top folder or a folder handed
                // over; what it makes is its own.
                if w[0] != s.root(Class::Documents).0 {
                    need(s, w[0], true)?;
                }
                let name = Self::name(ch, w[2])?;
                let f = s.create(p, app, FileId(w[0]), &name, w[1] != 0)?;
                s.own_file(p, app, f)?;
                Ok(Some(alloc::vec![f.0]))
            }
            fs::LOOKUP => {
                if !docs(w[0]) {
                    return Ok(None);
                }
                let name = Self::name(ch, w[1])?;
                let f = s.lookup(p, FileId(w[0]), &name)?;
                need(s, f.0, false)?;
                Ok(Some(alloc::vec![f.0]))
            }
            fs::LIST => {
                if !docs(w[0]) {
                    return Ok(None);
                }
                let mut l = s.list(p, FileId(w[0]))?;
                let mut keep = Vec::with_capacity(l.len());
                for (name, f) in l.drain(..) {
                    if s.access(p, app, f, now)? != Access::No {
                        keep.push((name, f));
                    }
                }
                Ok(Some(put_list(ch, &keep)))
            }
            fs::REMOVE | fs::RENAME => {
                let (dir, at) = (w[0], len_name(w, label));
                if !docs(dir) {
                    return Ok(None);
                }
                let name = Self::name(ch, at)?;
                let f = s.lookup(p, FileId(dir), &name)?;
                need(s, f.0, true)?;
                if label == fs::RENAME && !docs(w[1]) {
                    return Err(Error::BadRequest);
                }
                Ok(None)
            }
            // History, the timeline, undo, stories, grants and the tests'
            // own requests are for Alaya's carriers only.
            _ => Err(Error::BadRequest),
        }
    }

    /// The job `app` runs for person `p`, if any.
    fn job_for(&mut self, p: u64, app: u64) -> Option<Job> {
        if self.jobs.is_none() {
            let mut all = Vec::new();
            for q in self.store.people().unwrap_or_default() {
                if let Ok(j) = self.store.jobs(q) {
                    all.extend(j);
                }
            }
            self.jobs = Some(all);
        }
        self.jobs
            .as_ref()
            .and_then(|l| l.iter().find(|j| j.owner == p && j.app == app).cloned())
    }

    /// Jobs with rules (milestone 7, step 3): an app with a job works on
    /// its side copy, and sees and changes only the job's folder. Its
    /// private files stay invisible to it, the files that must stay
    /// unchanged are read only, and once it says it is done it changes
    /// nothing more. Everything else of the person's is not there for it.
    fn job_guard(
        &mut self,
        label: u64,
        app: u64,
        ch: u64,
        w: &[u64; 6],
        job: &Job,
    ) -> Result<Option<Vec<u64>>, Error> {
        let s = &mut *self.store;
        let side = job.side;
        let docs = |f: u64| FileId(f).class() == Class::Documents;
        let may = |s: &mut Store<KeeperDisk>, f: u64, write: bool| -> Result<(), Error> {
            let f = FileId(f);
            if !s.job_may(job, f, false)? || s.is_private(job.owner, f)? {
                return Err(Error::NotFound);
            }
            if write && !s.job_may(job, f, true)? {
                return Err(Error::BadRequest);
            }
            Ok(())
        };
        let len = |n: u64| (n as usize).min(fs::XFER_BYTES);
        match label {
            fs::ROOT | fs::SAVE | fs::STATS | fs::JOB | fs::JOB_DONE => return Ok(None),
            fs::READ
            | fs::WRITE
            | fs::STAT
            | fs::TRUNCATE
            | fs::CREATE
            | fs::LOOKUP
            | fs::LIST
            | fs::REMOVE
            | fs::RENAME
                if !docs(w[0]) =>
            {
                // Its own app files and caches, as always.
                if label == fs::RENAME && docs(w[1]) {
                    return Err(Error::BadRequest);
                }
                return Ok(None);
            }
            _ => {}
        }
        let out = match label {
            fs::READ => {
                may(s, w[0], false)?;
                let n = len(w[2]);
                alloc::vec![s.read(side, app, FileId(w[0]), w[1], &mut xfer(ch)[..n])? as u64]
            }
            fs::STAT => {
                may(s, w[0], false)?;
                let st = s.stat(side, FileId(w[0]))?;
                alloc::vec![st.size, st.version, st.is_dir as u64, st.edits]
            }
            fs::WRITE => {
                may(s, w[0], true)?;
                let n = len(w[2]);
                s.write(side, app, FileId(w[0]), w[1], &xfer(ch)[..n])?;
                alloc::vec![]
            }
            fs::TRUNCATE => {
                may(s, w[0], true)?;
                s.truncate(side, app, FileId(w[0]), w[1])?;
                alloc::vec![]
            }
            fs::CREATE => {
                may(s, w[0], true)?;
                let name = Self::name(ch, w[2])?;
                alloc::vec![s.create(side, app, FileId(w[0]), &name, w[1] != 0)?.0]
            }
            fs::LOOKUP => {
                may(s, w[0], false)?;
                let name = Self::name(ch, w[1])?;
                let f = s.lookup(side, FileId(w[0]), &name)?;
                may(s, f.0, false)?;
                alloc::vec![f.0]
            }
            fs::LIST => {
                may(s, w[0], false)?;
                let mut l = s.list(side, FileId(w[0]))?;
                let mut keep = Vec::with_capacity(l.len());
                for (name, f) in l.drain(..) {
                    if !s.is_private(job.owner, f)? {
                        keep.push((name, f));
                    }
                }
                put_list(ch, &keep)
            }
            fs::REMOVE => {
                may(s, w[0], true)?;
                let name = Self::name(ch, w[1])?;
                let f = s.lookup(side, FileId(w[0]), &name)?;
                may(s, f.0, true)?;
                s.remove(side, app, FileId(w[0]), &name)?;
                alloc::vec![]
            }
            fs::RENAME => {
                may(s, w[0], true)?;
                if !docs(w[1]) {
                    return Err(Error::BadRequest);
                }
                may(s, w[1], true)?;
                let name = Self::name(ch, w[2])?;
                let f = s.lookup(side, FileId(w[0]), &name)?;
                may(s, f.0, true)?;
                let b = len(w[3]);
                if b == 0 || b > alaya_store::NAME_MAX {
                    return Err(Error::BadName);
                }
                let to = xfer(ch)[fs::NAME2..fs::NAME2 + b].to_vec();
                s.rename(side, app, FileId(w[0]), &name, FileId(w[1]), &to)?;
                alloc::vec![]
            }
            // Its "made from" notes, the person's history, undo, grants:
            // not while it works under rules.
            _ => return Err(Error::BadRequest),
        };
        Ok(Some(out))
    }

    /// "Run it again": the app sees only the documents it had read then,
    /// each at the version it read, and changes none of them. Its own app
    /// files work as always.
    fn replay_guard(
        &mut self,
        label: u64,
        p: u64,
        ch: u64,
        w: &[u64; 6],
        files: &[ReadThen],
    ) -> Result<Option<Vec<u64>>, Error> {
        let s = &mut *self.store;
        let docs = |f: u64| FileId(f).class() == Class::Documents;
        let then = |f: u64| files.iter().find(|r| r.file.0 == f).copied();
        match label {
            fs::ROOT | fs::SAVE | fs::STATS | fs::JOB | fs::ACTION_BEGIN | fs::RECORD_ACTION => {
                return Ok(None);
            }
            fs::READ
            | fs::WRITE
            | fs::STAT
            | fs::TRUNCATE
            | fs::CREATE
            | fs::LOOKUP
            | fs::LIST
            | fs::REMOVE
            | fs::RENAME
                if !docs(w[0]) =>
            {
                if label == fs::RENAME && docs(w[1]) {
                    return Err(Error::BadRequest);
                }
                return Ok(None);
            }
            _ => {}
        }
        let out = match label {
            fs::READ | fs::STAT => {
                let r = then(w[0]).ok_or(Error::NotFound)?;
                let past = s.past(p, Class::Documents, r.moment)?;
                if label == fs::STAT {
                    let st = s.past_stat(&past, r.file)?;
                    alloc::vec![st.size, st.version, st.is_dir as u64, st.edits]
                } else {
                    let n = (w[2] as usize).min(fs::XFER_BYTES);
                    alloc::vec![s.past_read(&past, r.file, w[1], &mut xfer(ch)[..n])? as u64]
                }
            }
            fs::LOOKUP => {
                let name = Self::name(ch, w[1])?;
                let f = s.lookup(p, FileId(w[0]), &name)?;
                then(f.0).ok_or(Error::NotFound)?;
                alloc::vec![f.0]
            }
            fs::LIST => {
                let mut l = s.list(p, FileId(w[0]))?;
                l.retain(|(_, f)| then(f.0).is_some());
                put_list(ch, &l)
            }
            // Read only: nothing of the person's changes.
            _ => return Err(Error::BadRequest),
        };
        Ok(Some(out))
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
                self.jobs = None;
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
                alloc::vec![st.size, st.version, st.is_dir as u64, st.edits]
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
                put_list(ch, &l)
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
            fs::MADE_FROM => {
                let n = w[1] as usize;
                if n > alaya_store::made::MADE_MAX {
                    return Err(Error::BadRequest);
                }
                let buf = xfer(ch);
                let at = |i: usize| u64::from_le_bytes(buf[8 * i..8 * i + 8].try_into().unwrap());
                let sources: Vec<Source> = (0..n)
                    .map(|i| Source {
                        file: FileId(at(3 * i)),
                        version: at(3 * i + 1),
                        edits: at(3 * i + 2),
                    })
                    .collect();
                s.made_from(p, app, FileId(w[0]), &sources)?;
                alloc::vec![]
            }
            fs::MARK => {
                let m = s.mark(p, FileId(w[0]))?;
                alloc::vec![
                    m.kind as u64 | (m.why as u64) << 8,
                    m.source.map_or(0, |f| f.0),
                    m.now_version,
                    m.maker,
                    m.sources,
                ]
            }
            fs::AS_OF => {
                s.set_as_of(p, app, FileId(w[0]), w[1] != 0)?;
                alloc::vec![]
            }
            fs::RENAME => {
                let (a, b) = (len(w[2]), len(w[3]));
                if a == 0 || b == 0 || a > alaya_store::NAME_MAX || b > alaya_store::NAME_MAX {
                    return Err(Error::BadName);
                }
                let buf = xfer(ch);
                let name = buf[..a].to_vec();
                let to = buf[fs::NAME2..fs::NAME2 + b].to_vec();
                s.rename(p, app, FileId(w[0]), &name, FileId(w[1]), &to)?;
                alloc::vec![]
            }
            fs::ACTION_BEGIN => {
                if s.has_changes() {
                    s.commit(now)?;
                }
                self.hold_until = rt::now_ms() + fs::ACTION_MS;
                alloc::vec![]
            }
            fs::RECORD_ACTION => {
                let n = len(w[1]).min(fs::LABEL);
                let label = xfer(ch)[..n].to_vec();
                let no = s.record_action(p, app, w[0], &label)?;
                s.commit(now)?;
                self.hold_until = 0;
                alloc::vec![no]
            }
            fs::ACTIONS => {
                let max = (w[1] as usize).min(fs::XFER_BYTES / fs::ACTION_BYTES);
                let l = s.actions(p, w[0], max)?;
                let buf = xfer(ch);
                for (i, a) in l.iter().enumerate() {
                    let o = fs::ACTION_BYTES * i;
                    put_words(
                        buf,
                        o,
                        &[
                            a.no,
                            a.cn,
                            a.app,
                            a.kind | (a.undone as u64) << 8 | a.undoes << 16,
                        ],
                    );
                    buf[o + 32..o + 48].copy_from_slice(&a.label);
                }
                alloc::vec![l.len() as u64]
            }
            fs::UNDO | fs::UNDO_CHANGE => {
                let r = if label == fs::UNDO {
                    s.undo(p, app, w[0], w[1] != 0, now)?
                } else {
                    s.undo_change(p, app, w[0], w[1], w[2] != 0, now)?
                };
                let buf = xfer(ch);
                match r {
                    Undo::Done { action, files } => {
                        let n = files.len().min(fs::XFER_BYTES / 8);
                        for (i, f) in files.iter().take(n).enumerate() {
                            put_words(buf, 8 * i, &[f.0]);
                        }
                        let l0 = u64::from_le_bytes(action.label[..8].try_into().unwrap());
                        let l1 = u64::from_le_bytes(action.label[8..].try_into().unwrap());
                        alloc::vec![fs::undo::DONE, n as u64, action.kind, l0, l1]
                    }
                    Undo::Conflicts(files) => {
                        let n = files.len().min(fs::XFER_BYTES / 8);
                        for (i, f) in files.iter().take(n).enumerate() {
                            put_words(buf, 8 * i, &[f.0]);
                        }
                        alloc::vec![fs::undo::CONFLICTS, n as u64]
                    }
                }
            }
            fs::HAPPENINGS => {
                let h = s.happenings(p, w[0], (w[1] as usize).min(256))?;
                let buf = xfer(ch);
                let mut rows = 0;
                let mut commits = 0;
                'all: for x in &h {
                    if rows + x.changes.len() > fs::XFER_BYTES / fs::HAPPENING_BYTES {
                        break 'all;
                    }
                    for c in &x.changes {
                        put_words(
                            buf,
                            fs::HAPPENING_BYTES * rows,
                            &[x.cn, x.time, c.app, c.file.0, c.what],
                        );
                        rows += 1;
                    }
                    commits += 1;
                }
                alloc::vec![rows as u64, commits]
            }
            fs::NAME_OF => {
                let cn = (w[1] != 0).then_some(w[1]);
                match s.name_of(p, FileId(w[0]), cn)? {
                    Some((name, dir)) => {
                        xfer(ch)[..name.len()].copy_from_slice(&name);
                        alloc::vec![name.len() as u64, dir.0]
                    }
                    None => return Err(Error::NotFound),
                }
            }
            fs::NAME_VERSION => {
                if w[1] == 0 {
                    s.unname_version(p, w[0])?;
                } else {
                    let name = Self::name(ch, w[1])?;
                    s.name_version(p, w[0], &name)?;
                }
                s.commit(now)?;
                alloc::vec![]
            }
            fs::NAMED => {
                let l = s.named_versions(p)?;
                let buf = xfer(ch);
                let n = l.len().min(fs::XFER_BYTES / fs::NAMED_BYTES);
                for (i, (m, name)) in l.iter().take(n).enumerate() {
                    let o = fs::NAMED_BYTES * i;
                    let k = name.len().min(32);
                    put_words(buf, o, &[m.cn, m.time, k as u64]);
                    buf[o + 24..o + 56].fill(0);
                    buf[o + 24..o + 24 + k].copy_from_slice(&name[..k]);
                }
                alloc::vec![n as u64]
            }
            fs::PAST_LIST => {
                let dir = FileId(w[1]);
                let past = s.past(p, dir.class(), w[0])?;
                let l = s.past_list(&past, dir)?;
                let n = l.len().min(fs::XFER_BYTES / fs::PAST_ENTRY_BYTES);
                let mut rows = Vec::with_capacity(n);
                for (name, f) in l.into_iter().take(n) {
                    let st = s.past_stat(&past, f)?;
                    rows.push((name, f, st.size, st.is_dir));
                }
                let buf = xfer(ch);
                for (i, (name, f, size, is_dir)) in rows.iter().enumerate() {
                    let o = fs::PAST_ENTRY_BYTES * i;
                    put_words(buf, o, &[f.0, name.len() as u64, *size, *is_dir as u64]);
                    buf[o + 32..o + 64].fill(0);
                    buf[o + 32..o + 32 + name.len()].copy_from_slice(name);
                }
                alloc::vec![n as u64]
            }
            fs::RESTORE => {
                let n = (w[1] as usize).min(fs::XFER_BYTES / 8);
                let files: Vec<FileId> = (0..n).map(|i| FileId(word_at(xfer(ch), i))).collect();
                let r = s.restore(p, app, &files, w[0])?;
                let (code, files) = match r {
                    Restored::Done(f) => {
                        s.commit(now)?;
                        (fs::undo::DONE, f)
                    }
                    Restored::Blocked(f) => (fs::undo::CONFLICTS, f),
                };
                let buf = xfer(ch);
                for (i, f) in files.iter().enumerate() {
                    put_words(buf, 8 * i, &[f.0]);
                }
                alloc::vec![code, files.len() as u64]
            }
            fs::STORY => {
                let max = (w[1] as usize).min(256);
                let st = s.story(p, FileId(w[0]), max)?;
                let buf = xfer(ch);
                for (i, c) in st.changes.iter().enumerate() {
                    put_words(buf, fs::STORY_BYTES * i, &[c.0, c.1, c.2, c.3]);
                }
                let at = fs::STORY_BYTES * st.changes.len();
                let used = st.used_by.len().min((fs::XFER_BYTES - at) / 8);
                for (i, f) in st.used_by.iter().take(used).enumerate() {
                    put_words(buf, at + 8 * i, &[f.0]);
                }
                let (by, cn, time) = st.made_by.unwrap_or((0, 0, 0));
                alloc::vec![by, cn, time, st.changes.len() as u64, used as u64]
            }
            fs::GRANT => {
                if app != fs::DESKTOP_APP && app != fs::INIT_APP {
                    return Err(Error::BadRequest);
                }
                let lasts = Lasts::from_code(w[3], w[4]).ok_or(Error::BadRequest)?;
                s.grant(p, w[0], FileId(w[1]), w[2] != 0, lasts)?;
                alloc::vec![]
            }
            fs::REVOKE => alloc::vec![s.revoke(p, w[0], FileId(w[1]))? as u64],
            fs::GRANTS => {
                let l = s.grants(p, (w[0] != 0).then_some(w[0]))?;
                let n = l.len().min(fs::XFER_BYTES / fs::GRANT_BYTES);
                let buf = xfer(ch);
                for (i, g) in l.iter().take(n).enumerate() {
                    let (kind, arg) = g.lasts.code();
                    put_words(
                        buf,
                        fs::GRANT_BYTES * i,
                        &[
                            g.app,
                            g.file.0,
                            kind | (g.write as u64) << 8 | (g.own as u64) << 9,
                            arg,
                        ],
                    );
                }
                alloc::vec![n as u64]
            }
            fs::APP_CLOSED => {
                if app != fs::DESKTOP_APP && app != fs::INIT_APP {
                    return Err(Error::BadRequest);
                }
                self.replay.retain(|r| !(r.0 == p && r.1 == w[0]));
                alloc::vec![s.app_closed(p, w[0])?]
            }
            fs::RUN_AGAIN => {
                if s.has_changes() {
                    s.commit(now)?;
                }
                let l = s.read_then(p, w[0], w[1])?;
                let buf = xfer(ch);
                let n = l.len().min(fs::XFER_BYTES / 32);
                for (i, r) in l.iter().take(n).enumerate() {
                    put_words(
                        buf,
                        32 * i,
                        &[r.file.0, r.version, r.moment, r.exact as u64],
                    );
                }
                let exact = l.iter().filter(|r| r.exact).count();
                self.replay.retain(|r| !(r.0 == p && r.1 == w[0]));
                self.replay.push((p, w[0], l));
                alloc::vec![n as u64, exact as u64]
            }
            fs::FORGET_LATER => {
                let name = Self::name(ch, w[1])?;
                let f = s.forget_later(p, app, FileId(w[0]), &name, now + w[2])?;
                s.commit(now)?;
                alloc::vec![f.0]
            }
            fs::FORGETTING => {
                let l = s.forgetting(p)?;
                let n = l.len().min(fs::XFER_BYTES / 32);
                let buf = xfer(ch);
                for (i, (f, at, cn)) in l.iter().take(n).enumerate() {
                    put_words(buf, 32 * i, &[f.0, *at, at.saturating_sub(now), *cn]);
                }
                alloc::vec![n as u64]
            }
            fs::KEEP_FORGOTTEN => match s.keep_forgotten(p, app, FileId(w[0]))? {
                Restored::Done(_) => {
                    s.commit(now)?;
                    alloc::vec![]
                }
                Restored::Blocked(_) => return Err(Error::Exists),
            },
            fs::SEARCH => {
                let len = w[0] as usize;
                if len == 0 || len > 64 {
                    return Err(Error::BadRequest);
                }
                let q = xfer(ch)[..len].to_vec();
                let most = (w[1] as usize).min(fs::XFER_BYTES / fs::SEARCH_BYTES);
                let found = s.search(p, &q, most, w[2] as usize)?;
                let buf = xfer(ch);
                for (i, f) in found.iter().enumerate() {
                    let o = fs::SEARCH_BYTES * i;
                    let past = f.past.unwrap_or(alaya_store::Moment { cn: 0, time: 0 });
                    put_words(
                        buf,
                        o,
                        &[
                            f.file.0,
                            f.dir.0,
                            f.at.unwrap_or(u64::MAX),
                            past.cn,
                            past.time,
                            f.gone as u64,
                            f.name.len() as u64,
                            f.line.len() as u64,
                        ],
                    );
                    buf[o + 64..o + 160].fill(0);
                    buf[o + 64..o + 64 + f.name.len()].copy_from_slice(&f.name);
                    let l = f.line.len().min(64);
                    buf[o + 96..o + 96 + l].copy_from_slice(&f.line[..l]);
                }
                alloc::vec![found.len() as u64]
            }
            fs::BACKUP | fs::RESTORE_BACKUP | fs::STICK_FIND => {
                let Some(stick) = self.stick.as_mut() else {
                    return Err(Error::NotFound);
                };
                match label {
                    fs::BACKUP => {
                        let r = s.backup(p, stick, w[0] != 0)?;
                        s.commit(now)?;
                        say!(
                            "backup run {}: {} changed, {} gone, {} KiB",
                            r.run,
                            r.written,
                            r.gone,
                            r.bytes / 1024
                        );
                        alloc::vec![r.run, r.written, r.gone, r.used, r.bytes]
                    }
                    fs::RESTORE_BACKUP => {
                        let b = xfer(ch);
                        let key =
                            alaya_store::backup::recovery_from(&b[..39]).ok_or(Error::WrongKey)?;
                        let n = (w[0] as usize).min(alaya_store::NAME_MAX);
                        let into = b[64..64 + n].to_vec();
                        let r = s.restore_backup(p, app, stick, &key, &into)?;
                        s.commit(now)?;
                        alloc::vec![r.folders, r.documents, r.into, r.run]
                    }
                    _ => {
                        if app != fs::INIT_APP {
                            return Err(Error::BadRequest);
                        }
                        let n = (w[0] as usize).min(64);
                        let needle = xfer(ch)[..n].to_vec();
                        alloc::vec![alaya_store::backup::raw_find(stick, &needle)? as u64]
                    }
                }
            }
            fs::RECOVERY_KEY => {
                use alaya_programs::desktop::role;
                if ![fs::DESKTOP_APP, fs::INIT_APP, fs::app_id(role::SETTINGS)].contains(&app) {
                    return Err(Error::BadRequest);
                }
                let (r, new) = s.recovery_key(p)?;
                xfer(ch)[..39].copy_from_slice(&alaya_store::backup::recovery_text(&r));
                alloc::vec![new as u64]
            }
            fs::PRIVATE => {
                let f = FileId(w[0]);
                if w[1] < 2 {
                    s.set_private(p, f, w[1] == 1)?;
                    s.commit(now)?;
                }
                alloc::vec![s.is_private(p, f)? as u64]
            }
            fs::MARKS => {
                let l = s.marks(p)?;
                let n = l.len().min(fs::XFER_BYTES / 24);
                let buf = xfer(ch);
                for (i, m) in l.iter().take(n).enumerate() {
                    put_words(buf, 24 * i, &[m.app, m.file.0, m.since]);
                }
                alloc::vec![n as u64]
            }
            fs::JOB => match s.job_of(p, app)? {
                Some(j) => alloc::vec![1, j.rules.folder.0, j.done as u64, j.rules.checks],
                None => alloc::vec![0],
            },
            fs::JOB_DONE => {
                let j = s.job_of(p, app)?.ok_or(Error::NotFound)?;
                s.job_done(j.side)?;
                self.jobs = None;
                alloc::vec![]
            }
            fs::JOB_START => {
                // What the rules name must be saved: the job starts from
                // the last saved moment.
                if s.has_changes() {
                    s.commit(now)?;
                }
                let n = (w[4] as usize).min(alaya_store::jobs::KEEP_SAME_MAX + 1);
                let buf = xfer(ch);
                let keep_same = (0..n).map(|i| FileId(word_at(buf, i))).collect();
                let l = (w[5] as usize).min(alaya_store::side::LABEL_MAX);
                let label = buf[fs::JOB_LABEL_AT..fs::JOB_LABEL_AT + l].to_vec();
                let rules = alaya_store::jobs::Rules {
                    folder: FileId(w[1]),
                    keep_same,
                    checks: w[2],
                    checklist: (w[3] != 0).then_some(FileId(w[3])),
                };
                let side = s.job_start(p, w[0], &label, &rules, now)?;
                s.commit(now)?;
                self.jobs = None;
                alloc::vec![side]
            }
            fs::JOB_CHECK => {
                let j = s.job_of(p, w[0])?.ok_or(Error::NotFound)?;
                let r = s.job_check(j.side)?;
                let buf = xfer(ch);
                let n = r.checks.len().min(64);
                for (i, c) in r.checks.iter().take(n).enumerate() {
                    put_words(
                        buf,
                        32 * i,
                        &[
                            c.check,
                            c.line as u64,
                            c.ok as u64,
                            c.file.map_or(0, |f| f.0),
                        ],
                    );
                }
                let m = r.stale.len().min(64);
                for (i, f) in r.stale.iter().take(m).enumerate() {
                    put_words(buf, 32 * n + 8 * i, &[f.0]);
                }
                alloc::vec![r.passed() as u64, n as u64, m as u64]
            }
            fs::JOB_KEEP => {
                let j = s.job_of(p, w[0])?.ok_or(Error::NotFound)?;
                if s.has_changes() {
                    s.commit(now)?;
                }
                let buf = xfer(ch);
                let n = (w[2] as usize).min(256);
                let mine: Vec<FileId> = (0..n).map(|i| FileId(word_at(buf, i))).collect();
                let (kind, files) = match s.job_keep(j.side, w[1] != 0, &[], &mine)? {
                    alaya_store::jobs::Kept::Kept(f) => {
                        s.commit(now)?;
                        (fs::job_kept::KEPT, f)
                    }
                    alaya_store::jobs::Kept::Held(r) => (fs::job_kept::HELD, r.stale),
                    alaya_store::jobs::Kept::Conflicts(f) => (fs::job_kept::CONFLICTS, f),
                };
                self.jobs = None;
                let m = files.len().min(256);
                for (i, f) in files.iter().take(m).enumerate() {
                    put_words(buf, 8 * i, &[f.0]);
                }
                alloc::vec![kind, m as u64]
            }
            fs::JOB_DISCARD => {
                let j = s.job_of(p, w[0])?.ok_or(Error::NotFound)?;
                s.job_discard(j.side)?;
                s.commit(now)?;
                self.jobs = None;
                alloc::vec![]
            }
            fs::COPY => {
                let f = FileId(w[0]);
                let st = s.stat(p, f)?;
                if st.is_dir {
                    return Err(Error::IsADirectory);
                }
                let n = (w[2] as usize).min(fs::CLIP_MAX);
                let mut data = alloc::vec![0u8; n];
                let got = s.read(p, app, f, w[1], &mut data)?;
                data.truncate(got);
                let src = (f.class() == Class::Documents).then_some(Source {
                    file: f,
                    version: st.version,
                    edits: st.edits,
                });
                self.clips.retain(|c| c.0 != p);
                self.clips.push((p, data, src));
                alloc::vec![got as u64]
            }
            fs::COPY_TEXT => {
                let n = (w[0] as usize).min(fs::CLIP_MAX);
                let data = xfer(ch)[..n].to_vec();
                self.clips.retain(|c| c.0 != p);
                self.clips.push((p, data, None));
                alloc::vec![n as u64]
            }
            fs::PASTE => {
                let Some((_, data, src)) = self.clips.iter().find(|c| c.0 == p).cloned() else {
                    return Ok(alloc::vec![0, 0, 0, 0]);
                };
                xfer(ch)[..data.len()].copy_from_slice(&data);
                if let Some(src) = src {
                    s.note_pasted(p, app, src)?;
                    // The mark spreads with what is pasted (decision 27);
                    // Alaya's own programs are never marked (rule B1).
                    if restricted(app) {
                        s.note_private_read(p, app, src.file, now)?;
                    }
                }
                let (f, v, e) = src.map_or((0, 0, 0), |x| (x.file.0, x.version, x.edits));
                alloc::vec![data.len() as u64, f, v, e]
            }
            fs::WHAT_CHANGED => match s.what_changed(p, FileId(w[0]))? {
                Some(d) => {
                    let buf = xfer(ch);
                    let (a, b) = (d.then.len().min(fs::DIFF_AT), d.now.len().min(fs::DIFF_AT));
                    buf[..a].copy_from_slice(&d.then[..a]);
                    buf[fs::DIFF_AT..fs::DIFF_AT + b].copy_from_slice(&d.now[..b]);
                    alloc::vec![d.source.0, d.exact as u64, a as u64, b as u64]
                }
                None => alloc::vec![0, 0, 0, 0],
            },
            fs::STICK => {
                let last = s.last_backup(p)?.map_or((0, 0), |(_, run, m)| (run, m));
                match &self.stick {
                    Some(k) => alloc::vec![1, k.blocks, last.0, last.1],
                    None => alloc::vec![0, 0, last.0, last.1],
                }
            }
            fs::FAKE_HISTORY => {
                let made = s.fake_history(p, app, w[0], w[1].max(1), now)?;
                alloc::vec![made]
            }
            fs::TRACK => {
                s.track_reads = w[0] != 0;
                alloc::vec![]
            }
            _ => return Err(Error::BadRequest),
        })
    }
}

/// Apps that see only what they are handed: every app (numbers from
/// `fs::app_id(0)` up) but Alaya's own carriers among them (the file
/// browser, Settings and "What happened?"). Numbers below are Alaya's own
/// programs: the desktop, the first program and its store checks.
fn restricted(app: u64) -> bool {
    use alaya_programs::desktop::role;
    app >= fs::app_id(0)
        && ![
            fs::app_id(role::FILES),
            fs::app_id(role::SETTINGS),
            fs::app_id(role::BACK),
        ]
        .contains(&app)
}

/// The length word of the name a REMOVE or RENAME names.
fn len_name(w: &[u64; 6], label: u64) -> u64 {
    if label == fs::RENAME { w[2] } else { w[1] }
}

/// A folder's entries in the transfer page: 48 bytes each.
fn put_list(ch: u64, l: &[(Vec<u8>, FileId)]) -> Vec<u64> {
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

/// What `app` may do with document `f`, or a refusal that says no more
/// than "not found". Files that are not documents pass.
fn need(
    s: &mut Store<KeeperDisk>,
    p: u64,
    app: u64,
    f: u64,
    write: bool,
    now: u64,
) -> Result<Access, Error> {
    if FileId(f).class() != Class::Documents {
        return Ok(Access::Current(true));
    }
    match s.access(p, app, FileId(f), now)? {
        Access::No => Err(Error::NotFound),
        Access::Pinned(_) if write => Err(Error::BadRequest),
        a => Ok(a),
    }
}
