// SPDX-License-Identifier: GPL-3.0-only
//! The disk test app: writes sectors with known data and reads them back,
//! from several threads, as fast as the disk answers, and counts every
//! answer that is not exactly right. It finds the disk through the
//! registry and never talks to the driver itself, only to its keeper.
//!
//! It also asks the memory helper for memory: a big chunk, a huge page it
//! uses, and more than its budget allows (which must be refused).

#![no_std]
#![no_main]

use core::sync::atomic::{AtomicU64, Ordering};

use alaya_programs::{
    DISKTEST_G_REGISTRY, DISKTEST_G_SC, DISKTEST_SPAN, DISKTEST_THREADS, TEST_PAGE, TestPage,
    keeper as k, memory, registry, status,
};
use alaya_sys::abi::{StartInfo, map, start};
use alaya_sys::rt::Mem;
use alaya_sys::{Caps, Kind, Msg, MsgInfo, say};

alaya_sys::entry!(main);

const MAX_THREADS: usize = 8;
const MAX_SPAN: usize = 64;
/// Seed each sector was last written with (0: not known).
static SEEDS: [[AtomicU64; MAX_SPAN]; MAX_THREADS] =
    [const { [const { AtomicU64::new(0) }; MAX_SPAN] }; MAX_THREADS];
static DISK: AtomicU64 = AtomicU64::new(0);
static SPAN: AtomicU64 = AtomicU64::new(0);
/// A notification nobody signals: workers sleep on it once told to stop.
static PARK: AtomicU64 = AtomicU64::new(0);

fn page() -> &'static TestPage {
    // SAFETY: the first program maps the test page here before we start.
    unsafe { &*(TEST_PAGE as *const TestPage) }
}

/// Ask the registry for `name`; the capability lands in `slot`.
fn lookup(name: &str, slot: u64) -> bool {
    let n = registry::name(name);
    let r = alaya_sys::ipc_call(
        start::FIRST_GIVEN + DISKTEST_G_REGISTRY,
        &Msg::new(registry::LOOKUP, &n),
        Caps {
            recv: slot,
            ..Caps::default()
        },
    );
    matches!(r, Ok(m) if m.words[0] == 0 && m.info.has(MsgInfo::CAP))
}

fn memory_checks(mem: &mut Mem) -> Result<(), u64> {
    let helper = mem.slot().unwrap();
    if !lookup("memory", helper) {
        say!("no memory helper in the registry");
        return Err(11);
    }
    let ask = |label: u64, bits: u64, slot: u64| -> u64 {
        alaya_sys::ipc_call(
            helper,
            &Msg::new(label, &[bits]),
            Caps {
                recv: slot,
                ..Caps::default()
            },
        )
        .map(|m| m.words[0])
        .unwrap_or(99)
    };
    // A 1 MiB chunk, used for real.
    let chunk = mem.slot().unwrap();
    if ask(memory::UNTYPED, 20, chunk) != memory::OK {
        return Err(12);
    }
    let p = mem.slot().unwrap();
    if alaya_sys::retype(chunk, Kind::Page, 12, 1, mem.at(p)).is_err() {
        return Err(13);
    }
    // A huge page for a game's big buffers, mapped and touched.
    let huge = mem.slot().unwrap();
    if ask(memory::HUGE_PAGE, 0, huge) != memory::OK {
        return Err(14);
    }
    let at = 0x2000_0000_0000u64;
    if alaya_sys::page_map(huge, start::SPACE, at, map::WRITE).is_err() {
        return Err(15);
    }
    for off in (0..(2u64 << 20)).step_by(4096) {
        // SAFETY: the huge page is mapped writable there.
        unsafe { core::ptr::write_volatile((at + off) as *mut u64, off) };
    }
    let sum: u64 = (0..(2u64 << 20))
        .step_by(4096)
        // SAFETY: as above.
        .map(|off| unsafe { core::ptr::read_volatile((at + off) as *const u64) })
        .sum();
    if sum != (0..(2u64 << 20)).step_by(4096).sum::<u64>() {
        return Err(16);
    }
    page().huge_pages.store(1, Ordering::Release);
    // More than the budget: refused, and nothing changes.
    let big = mem.slot().unwrap();
    if ask(memory::UNTYPED, 26, big) != memory::OVER_BUDGET {
        return Err(17);
    }
    page().refused.fetch_add(1, Ordering::AcqRel);
    let used = alaya_sys::ipc_call(helper, &Msg::new(memory::USAGE, &[]), Caps::default())
        .map(|m| (m.words[0], m.words[1]))
        .unwrap_or((0, 0));
    say!(
        "the memory helper gave a 1 MiB chunk and a 2 MiB huge page ({} KiB of a {} KiB budget), and refused 64 MiB more",
        used.0 >> 10,
        used.1 >> 10
    );
    if used.0 != (1 << 20) + (2 << 20) {
        return Err(used.0);
    }
    Ok(())
}

fn main(info: &'static StartInfo) -> ! {
    let mut mem = Mem::new(info);
    let t = page();
    let r = memory_checks(&mut mem);
    if let Err(e) = r {
        say!("memory helper check failed at step {e}");
    }
    t.memory
        .store(if r.is_ok() { 1 } else { 2 }, Ordering::Release);

    let disk = mem.slot().unwrap();
    while !lookup("disk", disk) {
        alaya_sys::yield_now();
    }
    DISK.store(disk, Ordering::Release);
    PARK.store(
        mem.make(Kind::Notification, 0).expect("notification"),
        Ordering::Release,
    );
    let threads = (info.args[DISKTEST_THREADS] as usize).clamp(1, MAX_THREADS);
    let span = (info.args[DISKTEST_SPAN] as usize).clamp(1, MAX_SPAN);
    SPAN.store(span as u64, Ordering::Release);
    // The driver may still be starting: wait until it knows the size.
    let size = || {
        alaya_sys::ipc_call(disk, &Msg::new(k::SIZE, &[]), Caps::default())
            .map(|m| m.words[0])
            .unwrap_or(0)
    };
    let give_up = alaya_sys::clock() + 30 * info.clock_hz;
    let mut sectors = size();
    while sectors == 0 && alaya_sys::clock() < give_up {
        alaya_sys::yield_now();
        sectors = size();
    }
    t.sectors.store(sectors, Ordering::Release);
    if sectors < (threads * span) as u64 {
        t.error.store(1, Ordering::Release);
        say!("the disk has {sectors} sectors; too small");
    }
    for n in 1..threads {
        let sc = start::FIRST_GIVEN + DISKTEST_G_SC + (n as u64 - 1);
        mem.thread(worker, n as u64, sc, 100).expect("thread");
    }
    worker(0)
}

extern "C" fn worker(n: u64) -> ! {
    let t = page();
    t.threads.fetch_add(1, Ordering::AcqRel);
    let disk = DISK.load(Ordering::Acquire);
    let span = SPAN.load(Ordering::Acquire);
    let seeds = &SEEDS[n as usize];
    let mut rng = 0x9e37_79b9_7f4a_7c15u64 ^ (n + 1).wrapping_mul(0x2545_f491_4f6c_dd1d);
    let mut next = || {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        rng
    };
    loop {
        if t.stop.load(Ordering::Acquire) != 0 {
            // Sleep, not spin: spinning threads would keep four cores busy
            // entering the kernel for the rest of the boot checks.
            let _ = alaya_sys::wait(PARK.load(Ordering::Acquire));
            continue;
        }
        let i = next() % span;
        let sector = n * span + i;
        let known = seeds[i as usize].load(Ordering::Relaxed);
        let write = known == 0 || next() % 2 == 0;
        let (label, seed) = if write {
            (k::WRITE, next() | 1)
        } else {
            (k::READ, known)
        };
        let got = alaya_sys::ipc_call(disk, &Msg::new(label, &[sector, seed]), Caps::default());
        let (st, sum) = match got {
            Ok(m) => (m.words[0], m.words[1]),
            Err(_) => (99, 0),
        };
        let right = match (st, write) {
            (status::OK, true) => {
                seeds[i as usize].store(seed, Ordering::Relaxed);
                true
            }
            (status::OK, false) => sum == alaya_programs::expected(seed, sector),
            (status::DEVICE_OFF, _) => {
                // The clear answer of a switched-off device. A write that
                // got it may or may not have reached the disk.
                if write {
                    seeds[i as usize].store(0, Ordering::Relaxed);
                }
                t.off.fetch_add(1, Ordering::AcqRel);
                continue;
            }
            _ => false,
        };
        if right {
            t.ok.fetch_add(1, Ordering::AcqRel);
        } else {
            for (a, v) in t.last_wrong.iter().zip([label, sector, st, sum]) {
                a.store(v, Ordering::Release);
            }
            t.wrong.fetch_add(1, Ordering::AcqRel);
        }
    }
}
