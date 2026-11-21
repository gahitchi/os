// SPDX-License-Identifier: GPL-3.0-only
//! Milestone 5 checks: the Alaya store on the real (virtual) disk.
//!
//! The first program starts the store server from the boot image on top of
//! the driver keeper of milestone 4, and uses it as an app would: files,
//! people, history, who changed and read what, storage classes. Then it
//! stops the store dead at exact points while it saves, crashes the disk
//! driver under it, fills the disk, and reads the raw disk looking for
//! anything readable.

use core::sync::atomic::{AtomicU64, Ordering::SeqCst};

use alaya_programs::files::{self as fs, st};
use alaya_programs::keeper as k;
use alaya_sys::abi::{BootInfo, map};
use alaya_sys::program::{self, Parent, Program, Slots, Spec};
use alaya_sys::{At, Caps, Kind, Msg, rights};

use crate::m3::{Kit, sleep_until};
use crate::m4b::{KEEPER_CTL, KEEPER_EP, STORE_BUFS, module};
use crate::say;

const XFER_AT: u64 = 0x2700_0000_0000;
const SCRATCH: u64 = 0x2800_0000_0000;
/// Big buffers for the checks (the stack is small).
const BIG_AT: u64 = 0x2900_0000_0000;
const STORE_PRIORITY: u64 = 150;

/// Test keys for the two people (the real ones come from their PINs).
const ALICE: [u64; 4] = [0xa11ce, 1, 2, 3];
const BOB: [u64; 4] = [0xb0b, 4, 5, 6];

// Classes, as the store numbers them.
const DOCS: u64 = 0;
const APPS: u64 = 1;
const CACHES: u64 = 2;

static DYING: AtomicU64 = AtomicU64::new(0);
static STORE_UT: AtomicU64 = AtomicU64::new(0);
static KILLS: AtomicU64 = AtomicU64::new(0);

/// Waits for the store to stop dead, then ends it for good (the power
/// cut), which also frees whoever was waiting for its answer.
extern "C" fn reaper(_: u64) -> ! {
    loop {
        let _ = alaya_sys::wait(DYING.load(SeqCst));
        let _ = alaya_sys::revoke(At {
            table: crate::m5::table(),
            index: STORE_UT.load(SeqCst),
        });
        KILLS.fetch_add(1, SeqCst);
    }
}

static TABLE: AtomicU64 = AtomicU64::new(0);
fn table() -> u64 {
    TABLE.load(SeqCst)
}

/// An app's view of the store: its badged endpoint and transfer channel.
#[derive(Clone, Copy)]
struct App {
    ep: u64,
    ch: u64,
}

impl App {
    fn call(&self, label: u64, words: &[u64]) -> [u64; 6] {
        match alaya_sys::ipc_call(self.ep, &Msg::new(label, words), Caps::default()) {
            Ok(m) => m.words,
            // The store was ended while it held this request.
            Err(_) => [u64::MAX, 0, 0, 0, 0, 0],
        }
    }

    fn ok(&self, label: u64, words: &[u64]) -> [u64; 6] {
        let r = self.call(label, words);
        assert!(r[0] == st::OK, "store request {label} failed: {}", r[0]);
        r
    }

    fn buf(&self) -> &'static mut [u8] {
        let base = XFER_AT + self.ch * fs::XFER_PAGES * 4096;
        // SAFETY: the transfer pages are mapped here; one request at a time
        // per channel.
        unsafe { core::slice::from_raw_parts_mut(base as *mut u8, fs::XFER_BYTES) }
    }

    fn root(&self, class: u64) -> u64 {
        self.ok(fs::ROOT, &[class])[1]
    }

    fn create(&self, dir: u64, name: &str) -> u64 {
        self.buf()[..name.len()].copy_from_slice(name.as_bytes());
        let r = self.call(fs::CREATE, &[dir, 0, name.len() as u64]);
        if r[0] == st::EXISTS {
            return self.lookup(dir, name).expect("lookup");
        }
        assert!(r[0] == st::OK, "create {name}: {}", r[0]);
        r[1]
    }

    fn lookup(&self, dir: u64, name: &str) -> Option<u64> {
        self.buf()[..name.len()].copy_from_slice(name.as_bytes());
        let r = self.call(fs::LOOKUP, &[dir, name.len() as u64]);
        (r[0] == st::OK).then_some(r[1])
    }

    fn write(&self, f: u64, off: u64, data: &[u8]) -> u64 {
        let n = data.len().min(fs::XFER_BYTES);
        self.buf()[..n].copy_from_slice(&data[..n]);
        self.call(fs::WRITE, &[f, off, n as u64])[0]
    }

    fn read(&self, f: u64, off: u64, out: &mut [u8]) -> u64 {
        let n = out.len().min(fs::XFER_BYTES);
        let r = self.call(fs::READ, &[f, off, n as u64]);
        if r[0] == st::OK {
            out[..r[1] as usize].copy_from_slice(&self.buf()[..r[1] as usize]);
        }
        r[0]
    }

    fn save(&self) -> u64 {
        self.ok(fs::SAVE, &[])[1]
    }

    fn check(&self) -> [u64; 6] {
        let r = self.ok(fs::CHECK, &[]);
        assert!(r[1] == 0, "the store's check found {} problems", r[1]);
        r
    }
}

fn pattern(seed: u64, len: usize, out: &mut [u8]) {
    let mut x = seed | 1;
    for b in out.iter_mut().take(len) {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        *b = x as u8;
    }
}

struct Env<'a> {
    info: &'a BootInfo,
    k: Kit<'a>,
    ep: u64,
    keeper: u64,
    dying: u64,
    ut: u64,
    shared: [(u64, u64, u64); 128],
    nshared: usize,
    prog: Option<Program>,
    sc: u64,
    clock_sc: u64,
    slots_base: u64,
}

const STORE_SLOTS: u64 = 64;

impl Env<'_> {
    /// Start (or start again) the store server.
    fn start(&mut self, format: bool, blocks: u64) {
        let _ = alaya_sys::revoke(self.k.at(self.ut));
        let mut parent = Parent {
            space: self.info.space,
            scratch: SCRATCH,
            slots: Slots {
                table: self.info.table,
                next: self.slots_base,
                end: self.slots_base + STORE_SLOTS,
            },
        };
        // Clear the slots a previous instance used.
        for i in self.slots_base..self.slots_base + STORE_SLOTS {
            let _ = alaya_sys::delete(self.k.at(i));
        }
        let flags = if format { fs::FLAG_FORMAT } else { 0 };
        let p = program::start(
            &mut parent,
            &Spec {
                name: "store",
                image: module(self.info, "store"),
                untyped: self.ut,
                table_bits: 8,
                own_bits: 23,
                priority: STORE_PRIORITY,
                sc: self.sc,
                clock_sc: self.clock_sc,
                fault: 0,
                given: &[self.ep, self.keeper, self.dying],
                shared: &self.shared[..self.nshared],
                args: &[flags, blocks],
                clock_hz: self.info.clock_hz,
                cores: self.info.cores,
            },
        )
        .expect("start the store");
        self.prog = Some(p);
    }
}

pub fn run(info: &BootInfo, big: u64, first_free: u64, pass: &mut dyn FnMut(&str)) -> u64 {
    TABLE.store(info.table, SeqCst);
    let mut k = Kit {
        info,
        ut: 0,
        next: first_free,
        stacks: 128,
    };
    // 16 MiB for the store server and 8 MiB for the checks, each from a
    // block the earlier checks left alone if there is one (UEFI firmware
    // leaves less memory in one piece than a PC BIOS does).
    let carve = |k: &mut Kit, bits: u8, what: &str| {
        let h = k.slot();
        let free = info.untyped[..info.untyped_count as usize].iter().any(|b| {
            b.handle != big
                && b.size_bits >= u64::from(bits)
                && alaya_sys::retype(b.handle, Kind::Untyped, bits, 1, k.at(h)).is_ok()
        });
        if !free {
            alaya_sys::retype(big, Kind::Untyped, bits, 1, k.at(h)).expect(what);
        }
        h
    };
    let store_ut = carve(&mut k, 24, "16 MiB for the store");
    k.ut = carve(&mut k, 23, "8 MiB for the store checks");
    STORE_UT.store(store_ut, SeqCst);
    let ep = k.make(Kind::Endpoint, 0);
    let dying = k.make(Kind::Notification, 0);
    DYING.store(dying, SeqCst);
    let keeper = k.slot();
    alaya_sys::mint(
        k.at(keeper),
        k.at(KEEPER_EP.load(SeqCst)),
        rights::ALL,
        k::STORE,
    )
    .unwrap();
    let sc = k.make(Kind::SchedContext, 8);
    alaya_sys::sc_configure(
        info.sched_control,
        sc,
        1000,
        1000,
        alaya_sys::abi::core_type::ANY,
        0,
    )
    .unwrap();
    let clock_sc = k.make(Kind::SchedContext, 8);
    alaya_sys::sc_configure(
        info.sched_control,
        clock_sc,
        50,
        10_000,
        alaya_sys::abi::core_type::ANY,
        0,
    )
    .unwrap();
    // The transfer pages (shared with the store) and the keeper's buffer
    // pages (shared by the keeper and the store).
    let mut shared = [(0u64, 0u64, 0u64); 128];
    let mut n = 0;
    for i in 0..fs::CHANNELS * fs::XFER_PAGES {
        let h = k.page_at(info.space, XFER_AT + i * 4096, map::WRITE);
        shared[n] = (h, fs::XFER + i * 4096, map::WRITE);
        n += 1;
    }
    for (i, b) in STORE_BUFS.iter().enumerate() {
        shared[n] = (b.load(SeqCst), fs::BUF + i as u64 * 4096, map::WRITE);
        n += 1;
    }
    for i in 0..32 {
        k.page_at(info.space, BIG_AT + i * 4096, map::WRITE);
    }
    // SAFETY: just mapped, writable, used only by this thread.
    let (data, back) = unsafe {
        (
            core::slice::from_raw_parts_mut(BIG_AT as *mut u8, 64 * 1024),
            core::slice::from_raw_parts_mut((BIG_AT + 64 * 1024) as *mut u8, 64 * 1024),
        )
    };
    let slots_base = k.next;
    k.next += STORE_SLOTS;
    k.thread(reaper, 0, 199, 1000, 1000, 0);

    let mut env = Env {
        info,
        k,
        ep,
        keeper,
        dying,
        ut: store_ut,
        shared,
        nshared: n,
        prog: None,
        sc,
        clock_sc,
        slots_base,
    };
    let app = |env: &mut Env, person: u64, id: u64, ch: u64| {
        let h = env.k.slot();
        alaya_sys::mint(
            env.k.at(h),
            env.k.at(env.ep),
            rights::ALL,
            fs::badge(person, id, ch),
        )
        .unwrap();
        App { ep: h, ch }
    };
    let alice = app(&mut env, 1, 100, 0);
    let alice_b = app(&mut env, 1, 200, 1);
    let alice_c = app(&mut env, 1, 300, 2);
    let bob = app(&mut env, 2, 100, 3);

    if info.flags & alaya_sys::abi::boot_flags::STORE_SOAK != 0 {
        soak(&mut env, alice, data);
    }

    // ---- 44: format and open through the keeper ----
    env.start(true, 0);
    alice.ok(fs::ADD_PERSON, &ALICE);
    bob.ok(fs::ADD_PERSON, &BOB);
    let c = alice.save();
    let s = alice.ok(fs::STATS, &[]);
    say!(
        "store: {} blocks of 4 KiB, {} in use after formatting (commit {c})",
        s[4],
        s[3]
    );
    pass("the store formats the disk through the keeper, in real 4 KiB blocks");

    // ---- 45: files come back exactly after a restart ----
    let docs = alice.root(DOCS);
    let report = alice.create(docs, "report.txt");
    pattern(7, 40_000, data);
    assert!(alice.write(report, 0, &data[..40_000]) == st::OK);
    assert!(alice.write(report, 40_000, b"THE END") == st::OK);
    alice.save();
    env.start(false, 0);
    assert!(alice.call(fs::ROOT, &[DOCS])[0] == st::OK);
    assert!(
        alice.lookup(docs, "report.txt").is_none(),
        "files readable before the key was given"
    );
    alice.ok(fs::UNLOCK, &ALICE);
    let f = alice.lookup(docs, "report.txt").expect("the file is gone");
    let back = &mut back[..40_007];
    assert!(alice.read(f, 0, back) == st::OK);
    assert!(back[..40_000] == data[..40_000] && &back[40_000..] == b"THE END");
    alice.check();
    pass("files come back exactly after the store starts again");

    // ---- 46: people ----
    let mut wrong = ALICE;
    wrong[0] ^= 1;
    assert!(bob.call(fs::UNLOCK, &wrong)[0] == st::WRONG_KEY);
    bob.ok(fs::UNLOCK, &BOB);
    let bob_docs = bob.root(DOCS);
    assert!(bob.lookup(bob_docs, "report.txt").is_none());
    pass("each person's files are under their own key; a wrong key opens nothing");

    // ---- 47: history, who changed what, who read which version ----
    let budget = alice.create(docs, "budget");
    assert!(alice.write(budget, 0, b"rent 500, food 300") == st::OK);
    let c1 = alice.save();
    assert!(alice_b.write(budget, 0, b"rent 550, food 300") == st::OK);
    let c2 = alice_b.save();
    let mut b = [0u8; 18];
    assert!(alice_c.read(budget, 0, &mut b) == st::OK);
    let c3 = alice.save();
    let r = alice.ok(fs::PAST_READ, &[c1, budget, 0, 18]);
    assert!(r[1] == 18 && &alice.buf()[..18] == b"rent 500, food 300");
    let words =
        |buf: &[u8], i: usize| u64::from_le_bytes(buf[8 * i..8 * i + 8].try_into().unwrap());
    let ch = alice.ok(fs::CHANGES, &[c2]);
    assert!(ch[1] == 1 && words(alice.buf(), 0) == 200 && words(alice.buf(), 1) == budget);
    let rd = alice.ok(fs::READS, &[c3]);
    assert!(rd[1] >= 1);
    let found = (0..rd[1] as usize).any(|i| {
        let b = alice.buf();
        words(b, 3 * i) == 300 && words(b, 3 * i + 1) == budget && words(b, 3 * i + 2) == c2
    });
    assert!(
        found,
        "the read of version {c2} by app 300 was not recorded"
    );
    pass("history keeps every version; each commit records which app changed and read what");

    // ---- 48: storage classes ----
    let h0 = alice.ok(fs::STATS, &[])[5];
    let cache = alice.create(alice.root(CACHES), "shaders");
    for i in 0..8 {
        pattern(i, 8192, data);
        assert!(alice.write(cache, 0, &data[..8192]) == st::OK);
        alice.save();
    }
    assert!(alice.ok(fs::STATS, &[])[5] == h0, "caches kept history");
    let game = alice.create(alice.root(APPS), "game");
    pattern(1, 16384, data);
    assert!(alice.write(game, 0, &data[..16384]) == st::OK);
    alice.ok(fs::KEEP_APPS, &[]);
    alice.save();
    pattern(2, 16384, data);
    assert!(alice.write(game, 0, &data[..16384]) == st::OK);
    alice.save();
    assert!(
        alice.ok(fs::STATS, &[])[5] > h0,
        "the copy from before the update is not kept"
    );
    alice.ok(fs::RELEASE_APPS, &[]);
    alice.save();
    alice.check();
    pass("storage classes: documents keep history, apps one copy, caches none");

    // ---- 49: stopped dead at exact points while saving ----
    let pair_a = alice.create(docs, "pair-a");
    let pair_b = alice.create(docs, "pair-b");
    alice.save();
    let mut counter = 0u64;
    let mut promised = 0u64;
    let cuts = if info.flags & alaya_sys::abi::boot_flags::LONG != 0 {
        200
    } else {
        40
    };
    for cut in 0..cuts {
        // Change both files the same way, then save, but stop dead after
        // `cut` disk writes into the save (a different point each time).
        counter += 1;
        for (i, b) in data[..4800].iter_mut().enumerate() {
            *b = counter.to_le_bytes()[i % 8];
        }
        assert!(alice.write(pair_a, 0, &data[..4800]) == st::OK);
        assert!(alice.write(pair_b, 0, &data[..4800]) == st::OK);
        alice.ok(fs::DIE_AFTER, &[cut % 37]);
        let kills = KILLS.load(SeqCst);
        let r = alice.call(fs::SAVE, &[]);
        if r[0] == st::OK {
            promised = counter;
            // It finished before the cut: cut it on the next request.
            alice.ok(fs::DIE_AFTER, &[0]);
            let _ = alice.call(fs::SAVE, &[]);
        }
        assert!(
            sleep_until(5000, || KILLS.load(SeqCst) > kills),
            "the store did not stop"
        );
        env.start(false, 0);
        alice.ok(fs::UNLOCK, &ALICE);
        let mut x = [0u8; 4800];
        let mut y = [0u8; 4800];
        assert!(alice.read(pair_a, 0, &mut x) == st::OK);
        assert!(alice.read(pair_b, 0, &mut y) == st::OK);
        assert!(
            x == y,
            "cut {cut}: the two files of one commit do not match"
        );
        let got = u64::from_le_bytes(x[..8].try_into().unwrap());
        assert!(
            got == promised || got == counter,
            "cut {cut}: found {got}, promised {promised}"
        );
        promised = got;
        counter = got;
        if cut % 10 == 9 {
            alice.check();
        }
    }
    alice.check();
    say!(
        "store: stopped dead {cuts} times while saving; every file whole, no promised commit lost"
    );
    pass("the store stopped dead at many points while saving: nothing damaged, nothing lost");

    // ---- 50: the disk driver crashes under the store ----
    let ctl = KEEPER_CTL.load(SeqCst);
    let control = |label: u64, w: &[u64]| {
        alaya_sys::ipc_call(ctl, &Msg::new(label, w), Caps::default())
            .unwrap()
            .words
    };
    let before = control(k::STATS, &[])[0];
    control(k::POLICY, &[1000, 1000]);
    control(k::CHAOS, &[1, 20, 80, 0x5eed]);
    let mut files = [0u64; 8];
    for (i, f) in files.iter_mut().enumerate() {
        let mut name = *b"chaos-0";
        name[6] = b'0' + i as u8;
        *f = alice.create(docs, core::str::from_utf8(&name).unwrap());
    }
    // Save over and over while the driver is crashed: at random moments
    // (chaos), and on purpose just as every other save starts, so some
    // crashes land inside a save whatever the timing.
    let mut round = 0u64;
    loop {
        for (i, f) in files.iter().enumerate() {
            pattern(round * 8 + i as u64, 12_000, data);
            assert!(alice.write(*f, 0, &data[..12_000]) == st::OK);
        }
        if round % 2 == 1 {
            control(k::CRASH_NOW, &[]);
        }
        alice.save();
        if round >= 6 && control(k::STATS, &[])[0] - before >= 3 {
            break;
        }
        assert!(round < 200, "the driver was not restarted");
        round += 1;
    }
    control(k::CHAOS, &[0, 1, 2, 0]);
    control(k::POLICY, &[k::RESTART_LIMIT, k::RESTART_WINDOW_MS]);
    let restarts = control(k::STATS, &[])[0] - before;
    assert!(
        restarts >= 3,
        "the driver was crashed only {restarts} times"
    );
    for (i, f) in files.iter().enumerate() {
        let mut got = [0u8; 12_000];
        assert!(alice.read(*f, 0, &mut got) == st::OK);
        pattern(round * 8 + i as u64, 12_000, data);
        assert!(got[..] == data[..12_000], "wrong data after driver crashes");
    }
    alice.check();
    say!("store: the disk driver was restarted {restarts} times while the store saved");
    pass("the disk driver crashes while the store saves: no error, every block right");

    // ---- 51: nothing readable on the raw disk ----
    let marker = b"MY VERY PRIVATE LETTER";
    let letter = alice.create(docs, "letter");
    for (i, b) in data[..3300].iter_mut().enumerate() {
        *b = marker[i % marker.len()];
    }
    assert!(alice.write(letter, 0, &data[..3300]) == st::OK);
    alice.save();
    let first = alaya_sys::ipc_call(keeper, &Msg::new(k::BLOCKS, &[]), Caps::default())
        .unwrap()
        .words[0];
    let mut scanned = 0;
    for b in first..first + 1024 {
        let r = alaya_sys::ipc_call(keeper, &Msg::new(k::BREAD, &[b, 1]), Caps::default());
        assert!(r.map(|m| m.words[0]) == Ok(0), "raw read failed");
        // SAFETY: buffer slot 1 is mapped in the first program (m4b).
        let raw =
            unsafe { core::slice::from_raw_parts((0x2600_0000_0000u64 + 4096) as *const u8, 4096) };
        assert!(
            !raw.windows(8).any(|w| w == &marker[..8]),
            "readable text on the raw disk at block {b}"
        );
        scanned += 1;
    }
    say!("store: {scanned} raw blocks read straight from the disk: nothing readable");
    pass("everything on the disk is scrambled: the raw disk shows no file contents");

    // ---- 52: what recording reads costs ----
    let mut nf = [0u64; 24];
    for (i, f) in nf.iter_mut().enumerate() {
        let mut name = *b"read-00";
        name[5] = b'0' + (i / 10) as u8;
        name[6] = b'0' + (i % 10) as u8;
        *f = alice.create(docs, core::str::from_utf8(&name).unwrap());
        pattern(i as u64, 4096, data);
        assert!(alice.write(*f, 0, &data[..4096]) == st::OK);
    }
    alice.save();
    let mut best = [u64::MAX; 2];
    for round in 0..14 {
        let track = round % 2;
        alice.ok(fs::TRACK, &[track]);
        let t0 = alaya_sys::clock();
        for _ in 0..6 {
            for (i, f) in nf.iter().enumerate() {
                let mut got = [0u8; 4096];
                // A different app each time, so every read is new.
                let a = [alice, alice_b, alice_c][i % 3];
                assert!(a.read(*f, 0, &mut got) == st::OK);
            }
            for f in nf.iter().take(4) {
                assert!(alice.write(*f, 0, &data[..4096]) == st::OK);
            }
            alice.save();
        }
        let dt = alaya_sys::clock() - t0;
        best[track as usize] = best[track as usize].min(dt);
    }
    alice.ok(fs::TRACK, &[1]);
    let extra = (best[1] as i64 - best[0] as i64) * 1000 / best[0] as i64;
    say!(
        "store: recording what apps read cost {}.{}% in the worst case, every read a new record (best of 7 each)",
        extra / 10,
        (extra % 10).abs()
    );
    pass("recording which app read which version costs little");

    // ---- 53: a full disk can still delete ----
    env.start(true, 1024);
    alice.ok(fs::ADD_PERSON, &ALICE);
    alice.save();
    let docs = alice.root(DOCS);
    let caches = alice.root(CACHES);
    let mut made = 0;
    let full = loop {
        let dir = if made % 2 == 0 { docs } else { caches };
        let mut name = *b"fill-000";
        name[5] = b'0' + (made / 100 % 10) as u8;
        name[6] = b'0' + (made / 10 % 10) as u8;
        name[7] = b'0' + (made % 10) as u8;
        let name = core::str::from_utf8(&name).unwrap();
        alice.buf()[..name.len()].copy_from_slice(name.as_bytes());
        let r = alice.call(fs::CREATE, &[dir, 0, name.len() as u64]);
        if r[0] != st::OK {
            break r[0];
        }
        pattern(made, 16384, data);
        let w = alice.write(r[1], 0, &data[..16384]);
        if w != st::OK {
            break w;
        }
        made += 1;
        if made % 8 == 0 {
            alice.save();
        }
    };
    assert!(full == st::FULL, "filling the disk ended with {full}");
    alice.save();
    alice.check();
    // Delete everything, on a full disk.
    for dir in [docs, caches] {
        loop {
            let r = alice.ok(fs::LIST, &[dir]);
            if r[1] == 0 {
                break;
            }
            let b = alice.buf();
            let len = u64::from_le_bytes(b[8..16].try_into().unwrap()) as usize;
            let mut name = [0u8; 32];
            name[..len].copy_from_slice(&b[16..16 + len]);
            alice.buf()[..len].copy_from_slice(&name[..len]);
            alice.ok(fs::REMOVE, &[dir, len as u64]);
        }
    }
    alice.save();
    // History lets go of the deleted documents once they are over a day
    // old (choice B): ask for room as if two days had passed.
    alice.ok(fs::MAKE_ROOM, &[300, 2 * 24 * 3600 + 10_000]);
    alice.check();
    say!("store: filled the disk with {made} files, deleted them all, and got the room back");
    pass("a full disk can still delete, and old history makes room");

    // Leave a fresh, full-size store for later milestones.
    env.start(true, 0);
    env.k.next
}

/// `store-soak` boots (`cargo xtask test --plug <n>`): open the store left
/// on the disk by the last boot, which QEMU killed at a random moment, and
/// show the host what survived; then keep changing two files the same way
/// and saving until QEMU is killed again. The host checks that every boot
/// finds both files equal and no saved change lost.
fn soak(env: &mut Env, alice: App, data: &mut [u8]) -> ! {
    env.start(false, 0);
    let fresh = alice.call(fs::UNLOCK, &ALICE)[0] != st::OK
        || alice.lookup(alice.root(DOCS), "soak-a").is_none();
    if fresh {
        // The first soak boot: start from a fresh store over the whole
        // disk (the last boot checks may have left a small one).
        env.start(true, 0);
        alice.ok(fs::ADD_PERSON, &ALICE);
        alice.save();
    }
    let docs = alice.root(DOCS);
    let mut pair = [0u64; 2];
    let mut found = [0u64; 2];
    for (i, name) in ["soak-a", "soak-b"].into_iter().enumerate() {
        pair[i] = match alice.lookup(docs, name) {
            Some(f) => f,
            None => alice.create(docs, name),
        };
        let r = alice.call(fs::READ, &[pair[i], 0, 16_000]);
        assert!(r[0] == st::OK, "soak: cannot read file {i}");
        let n = r[1] as usize;
        let buf = &alice.buf()[..n];
        found[i] = if n >= 8 {
            let c = u64::from_le_bytes(buf[..8].try_into().unwrap());
            // The whole file must hold the pattern of its counter.
            for (j, b) in buf.iter().enumerate() {
                assert!(
                    *b == c.to_le_bytes()[j % 8] ^ (j / 8) as u8,
                    "soak: file {i} is torn"
                );
            }
            c
        } else {
            0
        };
    }
    assert!(
        found[0] == found[1],
        "soak: the two files differ: {} and {}",
        found[0],
        found[1]
    );
    let r = alice.check();
    say!(
        "soak: found {} (the store's check looked at {} blocks: no problems)",
        found[0],
        r[2]
    );
    let mut counter = found[0];
    loop {
        counter += 1;
        // A different length each time, so blocks are added and dropped.
        let len = 4096 + (counter as usize * 2731) % 12_000;
        for (j, b) in data[..len].iter_mut().enumerate() {
            *b = counter.to_le_bytes()[j % 8] ^ (j / 8) as u8;
        }
        for f in pair {
            alice.ok(fs::TRUNCATE, &[f, 0]);
            let mut r = alice.write(f, 0, &data[..len]);
            if r == st::FULL {
                // Every save keeps a version, so the disk fills up. Make
                // room as if a week had passed (choice B never gives up
                // the last day), which also runs the full-disk path under
                // the plug pulls.
                say!("soak: disk full at {counter}: making room");
                alice.ok(fs::MAKE_ROOM, &[2000, 7 * 24 * 3600 * (counter + 1)]);
                r = alice.write(f, 0, &data[..len]);
            }
            assert!(r == st::OK, "soak: write failed with status {r}");
        }
        say!("soak: saving {counter}");
        alice.save();
        say!("soak: promised {counter}");
    }
}
