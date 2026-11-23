// SPDX-License-Identifier: GPL-3.0-only
//! Milestone 4 checks, part 2: the system's helpers and the driver keeper.
//!
//! The first program sets up what later becomes the system's own start:
//!
//! - it walks the PCI bus and finds the virtual disk;
//! - it locks the disk to its own memory (an I/O space the IOMMU enforces)
//!   and takes an interrupt for it;
//! - it runs the **registry** (programs find each other by name) and the
//!   **memory helper** (memory in big chunks and huge pages, with a budget
//!   per app) as threads of its own;
//! - with the **program starter** it starts the driver keeper and the disk
//!   test app from the boot image; the keeper starts the driver.
//!
//! Then it attacks the driver while the test app keeps using the disk:
//! random crashes and freezes, a driver that crashes too often, a thousand
//! live updates, and a broken update that must go back by itself. The test
//! app must never see a wrong answer.

use core::sync::atomic::{AtomicU64, Ordering::SeqCst};

use alaya_programs::{TEST_PAGE, TestPage, keeper as k, memory, registry, status};
use alaya_sys::abi::{BootInfo, boot_flags, map};
use alaya_sys::elf::Elf;
use alaya_sys::program::{self, Parent, Slots, Spec};
use alaya_sys::{At, Caps, Kind, Msg, MsgInfo, rights};

use crate::m3::{HZ, Kit, TICK, TICKS, sleep_ms, ticker};
use crate::say;

/// Windows in the first program's address space.
const SCAN: u64 = 0x2200_0000_0000;
const IMAGES: u64 = 0x2300_0000_0000;
const TEST: u64 = 0x2400_0000_0000;
const SCRATCH: u64 = 0x2500_0000_0000;
/// The store's buffer pages, as the first program holds them.
const BUFS: u64 = 0x2600_0000_0000;
/// The store's buffer pages (shared with the keeper), for milestone 5.
pub static STORE_BUFS: [AtomicU64; k::BUF_SLOTS as usize] =
    [const { AtomicU64::new(0) }; k::BUF_SLOTS as usize];
/// The keeper's endpoint, for milestone 5 to mint the store's copy.
pub static KEEPER_EP: AtomicU64 = AtomicU64::new(0);
/// The first program's control copy of the keeper's endpoint.
pub static KEEPER_CTL: AtomicU64 = AtomicU64::new(0);

/// Priorities: the first program is at 200.
const KEEPER_PRIORITY: u64 = 180;
const HELPER_PRIORITY: u64 = 160;
const APP_PRIORITY: u64 = 100;

/// Badges on the registry and the memory helper.
const KEEPER: u64 = 1;
const APP: u64 = 2;
/// The memory helper's accounts: the disk test app's, and the first
/// program's own (to show budgets are separate).
const APP_BUDGET: u64 = 4 << 20;
const OWN: u64 = 3;
const OWN_BUDGET: u64 = 2 << 20;
/// The fuzzer's account (badge 1), and its budget.
const FUZZ: u64 = 1;
const FUZZ_BUDGET: u64 = 1 << 20;

/// Handles the fuzzer attacks the helpers with, and checks them with
/// afterwards: registry, memory helper and keeper (app side) copies for
/// it; then the first program's own registry, keeper control and memory
/// helper handles.
pub static HELPERS: [AtomicU64; 6] = [const { AtomicU64::new(0) }; 6];

const APP_THREADS: u64 = 4;
const APP_SPAN: u64 = 16;

// ---------------------------------------------------------------------------
// The registry

const NAMES: usize = 16;
static REG_EP: AtomicU64 = AtomicU64::new(0);
static REG_REPLY: AtomicU64 = AtomicU64::new(0);
/// First of `NAMES` slots where registered capabilities are kept.
static REG_SLOTS: AtomicU64 = AtomicU64::new(0);
static REG_NAMES: [[AtomicU64; 2]; NAMES] =
    [const { [AtomicU64::new(0), AtomicU64::new(0)] }; NAMES];
static REG_COUNT: AtomicU64 = AtomicU64::new(0);

extern "C" fn registry_server(_: u64) -> ! {
    let ep = REG_EP.load(SeqCst);
    let rep = REG_REPLY.load(SeqCst);
    let base = REG_SLOTS.load(SeqCst);
    loop {
        let n = REG_COUNT.load(SeqCst);
        let m = alaya_sys::msg_call(
            alaya_sys::abi::call::RECV,
            ep,
            &Msg::default(),
            Caps {
                recv: base + n.min(NAMES as u64 - 1),
                reply: rep,
                send: 0,
            },
        );
        let Ok(m) = m else { continue };
        let name = [m.words[0], m.words[1]];
        let answer = match m.label() {
            registry::REGISTER if m.info.has(MsgInfo::CAP) && (n as usize) < NAMES - 1 => {
                REG_NAMES[n as usize][0].store(name[0], SeqCst);
                REG_NAMES[n as usize][1].store(name[1], SeqCst);
                REG_COUNT.store(n + 1, SeqCst);
                (Msg::new(0, &[0]), 0)
            }
            registry::LOOKUP => {
                let found = (0..n as usize).find(|&i| {
                    REG_NAMES[i][0].load(SeqCst) == name[0]
                        && REG_NAMES[i][1].load(SeqCst) == name[1]
                });
                match found {
                    Some(i) => (
                        Msg {
                            info: MsgInfo::new(0, 1).with_cap(),
                            badge: 0,
                            words: [0; 6],
                        },
                        base + i as u64,
                    ),
                    None => (Msg::new(0, &[1]), 0),
                }
            }
            _ => (Msg::new(0, &[1]), 0),
        };
        let _ = alaya_sys::msg_call(
            alaya_sys::abi::call::SEND,
            rep,
            &answer.0,
            Caps {
                send: answer.1,
                ..Caps::default()
            },
        );
    }
}

// ---------------------------------------------------------------------------
// The memory helper

static MEM_EP: AtomicU64 = AtomicU64::new(0);
static MEM_REPLY: AtomicU64 = AtomicU64::new(0);
static MEM_POOL: AtomicU64 = AtomicU64::new(0);
static MEM_TABLE: AtomicU64 = AtomicU64::new(0);
/// Slots: one account per badge, then two for handing things out.
static MEM_SLOTS: AtomicU64 = AtomicU64::new(0);
const ACCOUNTS: u64 = 4;
const MEM_SLOT_COUNT: u64 = ACCOUNTS + 2;
/// Used bytes per account (badge).
static USED: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];

fn budget(badge: u64) -> u64 {
    match badge {
        APP => APP_BUDGET,
        OWN => OWN_BUDGET,
        FUZZ => FUZZ_BUDGET,
        _ => 0,
    }
}

extern "C" fn memory_server(_: u64) -> ! {
    let ep = MEM_EP.load(SeqCst);
    let rep = MEM_REPLY.load(SeqCst);
    let pool = MEM_POOL.load(SeqCst);
    let table = MEM_TABLE.load(SeqCst);
    let base = MEM_SLOTS.load(SeqCst);
    let mut next = 0;
    let mut got = alaya_sys::recv(ep, rep);
    loop {
        let Ok(m) = got else {
            got = alaya_sys::recv(ep, rep);
            continue;
        };
        let acct = (m.badge as usize).min(USED.len() - 1);
        let used = USED[acct].load(SeqCst);
        let want = match m.label() {
            memory::UNTYPED => {
                let bits = m.words[0];
                if !(u64::from(memory::MIN_BITS)..=u64::from(memory::MAX_BITS)).contains(&bits) {
                    None
                } else {
                    Some((Kind::Untyped, bits as u8))
                }
            }
            memory::HUGE_PAGE => Some((Kind::Page, 21)),
            _ => None,
        };
        let (answer, cap) = match (m.label(), want) {
            (memory::USAGE, _) => (Msg::new(0, &[used, budget(m.badge)]), 0),
            (_, None) => (Msg::new(0, &[memory::BAD_REQUEST]), 0),
            (_, Some((kind, bits))) => {
                let size = 1u64 << bits;
                if used + size > budget(m.badge) {
                    (Msg::new(0, &[memory::OVER_BUDGET]), 0)
                } else {
                    // Each app's memory comes from its own account, an
                    // untyped block the helper keeps: what it hands out
                    // moves to the app, and one revoke of the account
                    // takes it all back.
                    let account = base + acct as u64;
                    let fresh = alaya_sys::inspect(At {
                        table,
                        index: account,
                    })
                    .is_err();
                    let opened = if fresh {
                        let bits = (64 - (budget(m.badge) - 1).leading_zeros()) as u8;
                        alaya_sys::retype(
                            pool,
                            Kind::Untyped,
                            bits,
                            1,
                            At {
                                table,
                                index: account,
                            },
                        )
                    } else {
                        Ok(())
                    };
                    let give = base + ACCOUNTS + next;
                    let made = opened.and_then(|()| {
                        alaya_sys::retype(account, kind, bits, 1, At { table, index: give })
                    });
                    match made {
                        Ok(()) => {
                            next = (next + 1) % 2;
                            USED[acct].store(used + size, SeqCst);
                            (
                                Msg {
                                    info: MsgInfo::new(0, 1).moving_cap(),
                                    badge: 0,
                                    words: [memory::OK, 0, 0, 0, 0, 0],
                                },
                                give,
                            )
                        }
                        Err(_) => (Msg::new(0, &[memory::NO_MEMORY]), 0),
                    }
                }
            }
        };
        got = alaya_sys::msg_call(
            alaya_sys::abi::call::REPLY_RECV,
            ep,
            &answer,
            Caps {
                send: cap,
                recv: 0,
                reply: rep,
            },
        );
    }
}

// ---------------------------------------------------------------------------
// PCI

/// A PCI function's configuration space, mapped at `SCAN`.
pub(crate) fn cfg8(o: u64) -> u8 {
    // SAFETY: the configuration page is mapped at SCAN while scanning.
    unsafe { core::ptr::read_volatile((SCAN + o) as *const u8) }
}
pub(crate) fn cfg16(o: u64) -> u16 {
    // SAFETY: as above.
    unsafe { core::ptr::read_volatile((SCAN + o) as *const u16) }
}
pub(crate) fn cfg32(o: u64) -> u32 {
    // SAFETY: as above.
    unsafe { core::ptr::read_volatile((SCAN + o) as *const u32) }
}

/// Map the configuration page of `bdf` at `SCAN`; returns its handle.
pub(crate) fn map_config(k: &mut Kit, base: u64, bdf: u16) -> u64 {
    let h = k.slot();
    alaya_sys::device_page(
        k.info.device_control,
        base + (u64::from(bdf) << 12),
        12,
        k.at(h),
    )
    .unwrap();
    alaya_sys::page_map(h, k.info.space, SCAN, map::WRITE).unwrap();
    h
}

pub(crate) fn unmap_config(k: &mut Kit, h: u64) {
    alaya_sys::page_unmap(h).unwrap();
    alaya_sys::delete(k.at(h)).unwrap();
}

/// Physical address of BAR `bar` (memory BARs only).
pub(crate) fn bar_address(bar: u64) -> u64 {
    let lo = cfg32(0x10 + 4 * bar);
    if lo & 1 != 0 {
        return 0;
    }
    let mut a = u64::from(lo & !0xf);
    if (lo >> 1) & 3 == 2 {
        a |= u64::from(cfg32(0x10 + 4 * (bar + 1))) << 32;
    }
    a
}

/// The device pages a virtio driver needs: every page its register blocks
/// and its MSI-X table touch, as (BAR, page) codes.
pub(crate) fn virtio_pages(codes: &mut [u64; 12]) -> usize {
    let mut n = 0;
    let mut add = |bar: u64, off: u64, len: u64| {
        for p in off / 4096..=(off + len.max(1) - 1) / 4096 {
            let c = alaya_programs::driver::bar_code(bar, p);
            if !codes[..n].contains(&c) && n < codes.len() {
                codes[n] = c;
                n += 1;
            }
        }
    };
    let mut at = u64::from(cfg8(0x34) & !3);
    let mut guard = 0;
    while at != 0 && guard < 48 {
        guard += 1;
        match cfg8(at) {
            0x09 => {
                let kind = cfg8(at + 3);
                if (1..=4).contains(&kind) {
                    add(
                        u64::from(cfg8(at + 4)),
                        u64::from(cfg32(at + 8)),
                        u64::from(cfg32(at + 12)),
                    );
                }
            }
            0x11 => {
                let size = u64::from(cfg16(at + 2) & 0x7ff) + 1;
                let t = cfg32(at + 4);
                add(u64::from(t & 7), u64::from(t & !7), size * 16);
            }
            _ => {}
        }
        at = u64::from(cfg8(at + 1) & !3);
    }
    n
}

// ---------------------------------------------------------------------------

fn test_page() -> &'static TestPage {
    // SAFETY: mapped at TEST below, and stays mapped.
    unsafe { &*(TEST as *const TestPage) }
}

/// The part of an ELF file a program starter reads: headers and segments,
/// not the debug information after them.
fn loadable_len(image: &[u8]) -> usize {
    let Some(e) = Elf::parse(image) else {
        return image.len();
    };
    let mut end = 64;
    let phoff = u64::from_le_bytes(image[32..40].try_into().unwrap()) as usize;
    let phnum = u16::from_le_bytes(image[56..58].try_into().unwrap()) as usize;
    let phentsize = u16::from_le_bytes(image[54..56].try_into().unwrap()) as usize;
    end = end.max(phoff + phnum * phentsize);
    for s in e.segments() {
        end = end.max(s.offset + s.file_size);
    }
    end.min(image.len())
}

pub(crate) fn module<'a>(info: &'a BootInfo, name: &str) -> &'a [u8] {
    let m = info.modules[..info.module_count as usize]
        .iter()
        .find(|m| m.name() == name)
        .unwrap_or_else(|| panic!("the boot image has no {name}"));
    // SAFETY: the kernel mapped the boot modules here, read only.
    unsafe { core::slice::from_raw_parts(m.address as *const u8, m.size as usize) }
}

/// Ask the keeper something.
fn control(ep: u64, label: u64, words: &[u64]) -> [u64; 6] {
    alaya_sys::ipc_call(ep, &Msg::new(label, words), Caps::default())
        .unwrap()
        .words
}

fn stats(ep: u64) -> [u64; 6] {
    control(ep, k::STATS, &[])
}

/// Sleep until `done`, at most `ms`.
fn wait_for(ms: u64, done: impl Fn() -> bool) -> bool {
    crate::m3::sleep_until(ms, done)
}

fn no_wrong_answers(t: &TestPage) {
    let wrong = t.wrong.load(SeqCst);
    if wrong != 0 {
        let w: [u64; 4] = core::array::from_fn(|i| t.last_wrong[i].load(SeqCst));
        panic!(
            "the test app got {wrong} wrong answer(s); the last: label {}, sector {}, status {}, checksum {:#x}",
            w[0], w[1], w[2], w[3]
        );
    }
}

/// Run the checks; returns the next free slot.
pub fn run(info: &BootInfo, big: u64, first_free: u64, pass: &mut dyn FnMut(&str)) -> u64 {
    HZ.store(info.clock_hz, SeqCst);
    let long = info.flags & boot_flags::LONG != 0;
    let mut k = Kit {
        info,
        ut: 0,
        next: first_free,
        stacks: 0,
    };
    // 32 MiB for all of it, from a block the earlier checks left alone
    // if there is one.
    let fresh = info.untyped[..info.untyped_count as usize]
        .iter()
        .filter(|b| b.handle != big && b.size_bits >= 25)
        .min_by_key(|b| b.size_bits);
    match fresh {
        Some(b) if b.size_bits == 25 => k.ut = b.handle,
        Some(b) => {
            k.ut = k.slot();
            alaya_sys::retype(b.handle, Kind::Untyped, 25, 1, k.at(k.ut)).unwrap();
        }
        None => {
            k.ut = k.slot();
            alaya_sys::retype(big, Kind::Untyped, 25, 1, k.at(k.ut)).unwrap();
        }
    }
    // The big pieces first, largest first, so alignment wastes nothing.
    let keeper_ut = k.make(Kind::Untyped, 24);
    let mem_pool = k.make(Kind::Untyped, 23);
    let app_ut = k.make(Kind::Untyped, 22);
    let tick = k.make(Kind::Notification, 0);
    TICK.store(tick, SeqCst);
    TICKS.store(0, SeqCst);
    k.thread(ticker, tick, 254, 50, 2000, 0);

    // Find the disk on the PCI bus.
    assert!(info.ecam_base != 0, "no PCI configuration space");
    assert!(info.iommu != 0, "the IOMMU is off");
    let mut disk = None;
    let mut seen = 0;
    let bus = info.ecam_bus_start as u16;
    for dev in 0..32u16 {
        for func in 0..8u16 {
            let bdf = bus << 8 | dev << 3 | func;
            let h = map_config(&mut k, info.ecam_base, bdf);
            let (vendor, device) = (cfg16(0), cfg16(2));
            let multi = cfg8(0x0e) & 0x80 != 0;
            if vendor != 0xffff {
                seen += 1;
                if vendor == 0x1af4 && device == 0x1042 {
                    disk = Some(bdf);
                }
            }
            unmap_config(&mut k, h);
            if vendor == 0xffff || (func == 0 && !multi) {
                break;
            }
        }
    }
    let bdf = disk.expect("no virtual disk (virtio block device) on the PCI bus");
    say!(
        "pci: {} functions on bus {}; the virtual disk is {:02x}:{:02x}.{}",
        seen,
        bus,
        bdf >> 8,
        (bdf >> 3) & 31,
        bdf & 7
    );

    // Its register pages, and its configuration page for the driver.
    let cfg = map_config(&mut k, info.ecam_base, bdf);
    let mut codes = [0u64; 12];
    let ncodes = virtio_pages(&mut codes);
    let mut bars = [0u64; 6];
    for (i, b) in bars.iter_mut().enumerate() {
        *b = bar_address(i as u64);
    }
    alaya_sys::page_unmap(cfg).unwrap();
    let mut dev_pages = [0u64; 12];
    for i in 0..ncodes {
        let (bar, page) = (codes[i] >> 16, codes[i] & 0xffff);
        assert!(bars[bar as usize] != 0, "BAR {bar} is not memory");
        let h = k.slot();
        alaya_sys::device_page(
            info.device_control,
            bars[bar as usize] + page * 4096,
            12,
            k.at(h),
        )
        .unwrap();
        dev_pages[i] = h;
    }
    // Device pages of RAM are refused.
    let ram = k.make(Kind::Page, 12);
    let ram_phys = alaya_sys::page_address(ram).unwrap();
    let r = alaya_sys::device_page(info.device_control, ram_phys, 12, k.at(k.next));
    assert!(r.is_err(), "a device page was made over RAM");

    // Lock the disk to its own memory, and give it an interrupt.
    let io = k.make(Kind::IoSpace, 13);
    let io_pool = k.make(Kind::Untyped, 16);
    alaya_sys::iospace_assign(io, io_pool).unwrap();
    alaya_sys::device_bind(info.device_control, io, bdf).unwrap();
    let irq = k.slot();
    let vector = alaya_sys::irq_get(info.irq_control, 0, k.at(irq)).unwrap();
    say!(
        "the disk is locked to its own I/O space; its interrupt is vector {:#x}, {} register page(s)",
        vector,
        ncodes
    );
    let faults_before = alaya_sys::device_faults(info.device_control).unwrap();
    pass("PCI scan: the disk is found, locked to its own memory, and given an interrupt");

    // The registry and the memory helper.
    let reg = k.make(Kind::Endpoint, 0);
    REG_EP.store(reg, SeqCst);
    REG_REPLY.store(k.make(Kind::Reply, 0), SeqCst);
    REG_SLOTS.store(k.next, SeqCst);
    k.next += NAMES as u64;
    k.thread(registry_server, 0, HELPER_PRIORITY, 1000, 1000, 0);
    let mem_ep = k.make(Kind::Endpoint, 0);
    MEM_EP.store(mem_ep, SeqCst);
    MEM_REPLY.store(k.make(Kind::Reply, 0), SeqCst);
    MEM_POOL.store(mem_pool, SeqCst);
    MEM_TABLE.store(info.table, SeqCst);
    MEM_SLOTS.store(k.next, SeqCst);
    k.next += MEM_SLOT_COUNT;
    k.thread(memory_server, 0, HELPER_PRIORITY, 1000, 1000, 0);
    let badged = |k: &mut Kit, ep: u64, badge: u64| {
        let h = k.slot();
        alaya_sys::mint(k.at(h), k.at(ep), rights::ALL, badge).unwrap();
        h
    };
    let reg_init = badged(&mut k, reg, OWN);
    let mem_app = badged(&mut k, mem_ep, APP);
    let n = registry::name("memory");
    let m = Msg {
        info: MsgInfo::new(registry::REGISTER, 2).with_cap(),
        badge: 0,
        words: [n[0], n[1], 0, 0, 0, 0],
    };
    let r = alaya_sys::ipc_call(
        reg_init,
        &m,
        Caps {
            send: mem_app,
            ..Caps::default()
        },
    )
    .unwrap();
    assert!(r.words[0] == 0, "the registry refused a name");
    // Its own account: 2 MiB, so a 1 MiB chunk fits, then 2 MiB more does
    // not.
    let mem_own = badged(&mut k, mem_ep, OWN);
    let ask = |bits: u64, slot: u64| {
        alaya_sys::ipc_call(
            mem_own,
            &Msg::new(memory::UNTYPED, &[bits]),
            Caps {
                recv: slot,
                ..Caps::default()
            },
        )
        .unwrap()
        .words[0]
    };
    let s1 = k.slot();
    assert!(
        ask(20, s1) == memory::OK,
        "the memory helper refused memory within the budget"
    );
    assert!(alaya_sys::inspect(k.at(s1)).unwrap().is(Kind::Untyped));
    let s2 = k.slot();
    assert!(
        ask(21, s2) == memory::OVER_BUDGET,
        "the memory helper went over a budget"
    );
    assert!(ask(40, s2) == memory::BAD_REQUEST);
    pass("the memory helper gives memory and keeps each app within its budget");

    // The test page the app reports on.
    let tp = k.page_at(info.space, TEST, map::WRITE);

    // Driver images for the keeper: only the parts a starter reads.
    let mut image_pages = [(0u64, 0u64, 0u64); 128];
    let mut nimg = 0;
    let mut sizes = [0u64; 2];
    for (i, name) in ["blk1", "blk2"].iter().enumerate() {
        let full = module(info, name);
        let len = loadable_len(full);
        sizes[i] = len as u64;
        let base = IMAGES + i as u64 * k::IMAGE_STEP;
        for p in 0..(len as u64).div_ceil(4096) {
            let h = k.page_at(info.space, base + p * 4096, map::WRITE);
            let from = (p * 4096) as usize;
            let to = (from + 4096).min(len);
            // SAFETY: the page was just mapped writable there.
            let dst =
                unsafe { core::slice::from_raw_parts_mut((base + p * 4096) as *mut u8, to - from) };
            dst.copy_from_slice(&full[from..to]);
            image_pages[nimg] = (h, k::IMAGES + i as u64 * k::IMAGE_STEP + p * 4096, 0);
            nimg += 1;
        }
    }

    // The store's buffer pages (milestone 5): the keeper copies blocks
    // between them and the device's pages.
    for (i, b) in STORE_BUFS.iter().enumerate() {
        let h = k.page_at(info.space, BUFS + i as u64 * 4096, map::WRITE);
        b.store(h, SeqCst);
        image_pages[nimg] = (h, k::CLIENT_BUF + i as u64 * 4096, map::WRITE);
        nimg += 1;
    }

    // Start the keeper.
    let ep = k.make(Kind::Endpoint, 0);
    KEEPER_EP.store(ep, SeqCst);
    let ctl = badged(&mut k, ep, k::CONTROL);
    KEEPER_CTL.store(ctl, SeqCst);
    let reg_keeper = badged(&mut k, reg, KEEPER);
    let sc = |k: &mut Kit, budget: u64, period: u64| {
        let s = k.make(Kind::SchedContext, 8);
        alaya_sys::sc_configure(
            info.sched_control,
            s,
            budget,
            period,
            alaya_sys::abi::core_type::ANY,
            0,
        )
        .unwrap();
        s
    };
    let keeper_sc = sc(&mut k, 1000, 1000);
    let keeper_clock = sc(&mut k, 50, 5000);
    let driver_sc = [sc(&mut k, 1000, 1000), sc(&mut k, 1000, 1000)];
    let cfg_copy = k.slot();
    alaya_sys::copy(k.at(cfg_copy), k.at(cfg), rights::ALL).unwrap();
    let mut given = [0u64; 20];
    let fixed = [
        ep,
        io,
        irq,
        reg_keeper,
        driver_sc[0],
        driver_sc[1],
        cfg_copy,
    ];
    given[..fixed.len()].copy_from_slice(&fixed);
    given[fixed.len()..fixed.len() + ncodes].copy_from_slice(&dev_pages[..ncodes]);
    let mut args = [0u64; 16];
    args[k::ARG_BDF] = u64::from(bdf);
    args[k::ARG_IMAGES] = 2;
    args[k::ARG_IMAGE_SIZE] = sizes[0];
    args[k::ARG_IMAGE_SIZE + 1] = sizes[1];
    args[k::ARG_DEV_PAGES] = ncodes as u64;
    args[k::ARG_DEV_CODES..k::ARG_DEV_CODES + ncodes].copy_from_slice(&codes[..ncodes]);
    let mut parent = Parent {
        space: info.space,
        scratch: SCRATCH,
        slots: Slots {
            table: info.table,
            next: k.next,
            end: 1 << info.table_bits,
        },
    };
    let keeper = program::start(
        &mut parent,
        &Spec {
            name: "keeper",
            image: module(info, "keeper"),
            untyped: keeper_ut,
            table_bits: 10,
            own_bits: 23,
            priority: KEEPER_PRIORITY,
            sc: keeper_sc,
            clock_sc: keeper_clock,
            fault: 0,
            given: &given[..fixed.len() + ncodes],
            shared: &image_pages[..nimg],
            args: &args[..k::ARG_DEV_CODES + ncodes],
            clock_hz: info.clock_hz,
            cores: info.cores,
        },
    )
    .unwrap();
    let _ = keeper;
    k.next = parent.slots.next;

    // Start the disk test app.
    let reg_app = badged(&mut k, reg, APP);
    let mut app_given = [reg_app, 0, 0, 0];
    for g in app_given.iter_mut().skip(1) {
        *g = sc(&mut k, 1000, 1000);
    }
    let app_sc = sc(&mut k, 1000, 1000);
    parent.slots.next = k.next;
    program::start(
        &mut parent,
        &Spec {
            name: "disktest",
            image: module(info, "disktest"),
            untyped: app_ut,
            table_bits: 8,
            own_bits: 20,
            priority: APP_PRIORITY,
            sc: app_sc,
            clock_sc: 0,
            fault: 0,
            given: &app_given[..APP_THREADS as usize],
            shared: &[(tp, TEST_PAGE, map::WRITE)],
            args: &[APP_THREADS, APP_SPAN],
            clock_hz: info.clock_hz,
            cores: info.cores,
        },
    )
    .unwrap();
    k.next = parent.slots.next;
    let t = test_page();
    let started = wait_for(20_000, || {
        t.ok.load(SeqCst) >= 200 || t.error.load(SeqCst) != 0
    });
    assert!(
        t.error.load(SeqCst) == 0,
        "the test app could not start: {}",
        t.error.load(SeqCst)
    );
    assert!(started, "the test app got no answers from the disk");
    no_wrong_answers(t);
    say!(
        "the test app uses the disk ({} sectors) from {} threads through the keeper: {} right answers so far",
        t.sectors.load(SeqCst),
        t.threads.load(SeqCst),
        t.ok.load(SeqCst)
    );
    pass("program starter: the keeper, the driver and the test app run as separate programs");
    assert!(wait_for(5000, || t.memory.load(SeqCst) != 0));
    assert!(
        t.memory.load(SeqCst) == 1,
        "the memory helper checks of the test app failed"
    );
    assert!(t.huge_pages.load(SeqCst) == 1 && t.refused.load(SeqCst) == 1);
    pass(
        "an app found the memory helper in the registry, got a huge page, and was held to its budget",
    );

    // The guard probe of the driver's first start.
    let more = control(ctl, k::MORE, &[]);
    let faults = alaya_sys::device_faults(info.device_control).unwrap() - faults_before;
    assert!(more[2] == 1, "the driver did not run its guard probe");
    say!(
        "the driver pointed the disk at memory it was not given; the IOMMU blocked it and recorded {} blocked access(es)",
        faults
    );
    assert!(
        faults >= 1,
        "the IOMMU recorded no blocked access: the device may have reached memory outside its I/O space"
    );
    pass("the device memory guard stops a device reaching memory it was not given");

    // Random crashes and freezes. The minute of the crash rule is
    // shortened to a second so the driver can be killed often.
    let chaos_ms = if long { 10 * 60 * 1000 } else { 8_000 };
    control(ctl, k::POLICY, &[3, 1000]);
    let ok0 = t.ok.load(SeqCst);
    let s0 = stats(ctl);
    control(ctl, k::CHAOS, &[1, 400, 900, 0x5eed_0000 ^ info.clock_hz]);
    let mut left = chaos_ms;
    while left > 0 {
        let step = left.min(60_000);
        sleep_ms(step);
        left -= step;
        no_wrong_answers(t);
        if long {
            let s = stats(ctl);
            say!(
                "chaos: {} s left, {} restarts, {} right answers, 0 wrong",
                left / 1000,
                s[0] - s0[0],
                t.ok.load(SeqCst) - ok0
            );
        }
    }
    // The chaos picks its moments by the clock, and a restart takes a
    // while: on a slow host (plain emulation on a phone) 8 s can hold only
    // two restarts, more so when the dice choose freezes, which wait for
    // the watchdog. Keep the chaos going until there were at least three
    // restarts, for up to a minute more; a keeper that does not restart
    // the driver at all still fails.
    let mut extra_ms = 0;
    while stats(ctl)[0] - s0[0] < 3 && extra_ms < 60_000 {
        sleep_ms(500);
        extra_ms += 500;
        no_wrong_answers(t);
    }
    let chaos_ms = chaos_ms + extra_ms;
    control(ctl, k::CHAOS, &[0, 1, 2, 0]);
    // One more freeze, so the watchdog is certainly tested.
    let hangs = control(ctl, k::MORE, &[])[0];
    control(ctl, k::FREEZE_NOW, &[]);
    assert!(
        wait_for(15_000, || control(ctl, k::MORE, &[])[0] > hangs),
        "the watchdog did not notice a frozen driver"
    );
    let calm = wait_for(10_000, || stats(ctl)[5] == k::state::RUNNING);
    let s1 = stats(ctl);
    let more = control(ctl, k::MORE, &[]);
    let ok1 = t.ok.load(SeqCst);
    assert!(
        wait_for(5000, || t.ok.load(SeqCst) > ok1 + 50),
        "the disk did not come back after the chaos"
    );
    no_wrong_answers(t);
    assert!(
        calm && s1[5] == k::state::RUNNING,
        "the driver did not settle after the chaos"
    );
    assert!(
        t.off.load(SeqCst) == 0,
        "the device was switched off during the chaos"
    );
    let restarts = s1[0] - s0[0];
    assert!(
        restarts >= 3,
        "only {restarts} restarts in {chaos_ms} ms of chaos"
    );
    say!(
        "chaos for {} s: the driver was killed {} times ({} frozen, found by the watchdog), {} unfinished jobs were sent again; the app got {} right answers and 0 wrong",
        chaos_ms / 1000,
        restarts,
        more[0],
        s1[1] - s0[1],
        t.ok.load(SeqCst) - ok0
    );
    pass("random driver crashes and freezes: no error reaches the app");

    // A driver that keeps crashing: switched off, a clear error, try again.
    // The real rule (more than three crashes in a minute), and each crash
    // waits for the restart before it: on a slow host (a phone) a restart
    // can take longer than the one-second window the chaos used, so four
    // crashes 40 ms apart never fell inside it.
    control(ctl, k::POLICY, &[3, 60_000]);
    for i in 0..4 {
        let before = stats(ctl)[0];
        control(ctl, k::CRASH_NOW, &[]);
        if i < 3 {
            assert!(
                wait_for(10_000, || {
                    let s = stats(ctl);
                    s[0] > before && s[5] == k::state::RUNNING
                }),
                "the driver did not restart after crash {}",
                i + 1
            );
        }
    }
    assert!(
        wait_for(5000, || stats(ctl)[5] == k::state::OFF),
        "the device was not switched off"
    );
    assert!(
        wait_for(5000, || t.off.load(SeqCst) > 0),
        "the app did not get the clear error"
    );
    let off_seen = t.off.load(SeqCst);
    assert!(control(ctl, k::TRY_AGAIN, &[])[0] == 0);
    let ok2 = t.ok.load(SeqCst);
    assert!(
        wait_for(10_000, || t.ok.load(SeqCst) > ok2 + 50),
        "try again did not bring the disk back"
    );
    no_wrong_answers(t);
    say!(
        "after 4 quick crashes the disk was switched off; the app got {} clear \"device off\" answers, then \"try again\" brought it back",
        off_seen
    );
    pass(
        "a driver that keeps crashing is switched off with a clear error, and comes back on request",
    );
    control(ctl, k::POLICY, &[3, 60_000]);

    // Live updates while the app uses the disk.
    let updates = 1000;
    let ok3 = t.ok.load(SeqCst);
    let u0 = stats(ctl);
    let begin = alaya_sys::clock();
    let mut version = stats(ctl)[4];
    for i in 0..updates {
        // The app must really be using the disk between updates.
        let before = t.ok.load(SeqCst);
        assert!(
            wait_for(2000, || t.ok.load(SeqCst) > before),
            "the app made no progress before update {i}"
        );
        let image = if version == 1 { 1 } else { 0 };
        let r = control(ctl, k::UPDATE, &[image, 0, 0]);
        assert!(
            r[0] == status::OK && r[1] == image + 1,
            "update {i} to version {} failed: {r:?}",
            image + 1
        );
        version = r[1];
    }
    let us = crate::m3::ticks_to_us(alaya_sys::clock() - begin);
    no_wrong_answers(t);
    let u1 = stats(ctl);
    assert!(
        u1[2] - u0[2] == updates && u1[0] == u0[0],
        "updates restarted the driver"
    );
    say!(
        "{} live updates between versions 1 and 2 in {} ms ({} us each), with no reset of the disk; the app got {} right answers meanwhile, 0 wrong",
        updates,
        us / 1000,
        us / updates,
        t.ok.load(SeqCst) - ok3
    );
    pass("a running driver updated 1,000 times in a row, with no error reaching the app");

    // A broken update goes back by itself.
    let back = version;
    let image = if version == 1 { 1 } else { 0 };
    let r = control(
        ctl,
        k::UPDATE,
        &[image, 30_000, alaya_programs::driver::FLAG_FAULTY],
    );
    assert!(
        r[0] == status::OK,
        "the broken version did not start: {r:?}"
    );
    let rolled = wait_for(10_000, || {
        let s = stats(ctl);
        s[3] > u1[3] && s[4] == back
    });
    let s = stats(ctl);
    assert!(
        rolled && s[4] == back,
        "the old version did not come back: {s:?}"
    );
    let ok4 = t.ok.load(SeqCst);
    assert!(wait_for(5000, || t.ok.load(SeqCst) > ok4 + 50));
    no_wrong_answers(t);
    pass("a broken update goes back to the old version by itself, with nothing lost");

    // The real trial period: ten minutes with the old version kept.
    let image = if back == 1 { 1 } else { 0 };
    let r = control(ctl, k::UPDATE, &[image, u64::MAX, 0]);
    assert!(r[0] == status::OK);
    let s = stats(ctl);
    let left = control(ctl, k::MORE, &[])[4];
    assert!(
        s[5] == k::state::TRIAL && left > 590_000,
        "no ten-minute trial: {s:?}, {left} ms"
    );
    say!(
        "version {} runs on trial; the old version is kept for another {} s",
        s[4],
        left / 1000
    );
    pass("after a live update the old version is kept for ten minutes");

    t.stop.store(1, SeqCst);
    no_wrong_answers(t);
    let client = badged(&mut k, ep, k::CLIENT);
    let targets = [
        badged(&mut k, reg, FUZZ),
        badged(&mut k, mem_ep, FUZZ),
        client,
        reg_init,
        ctl,
        mem_own,
    ];
    for (h, v) in HELPERS.iter().zip(targets) {
        h.store(v, SeqCst);
    }
    say!(
        "the test app got {} right answers in all, 0 wrong",
        t.ok.load(SeqCst)
    );
    k.next
}
