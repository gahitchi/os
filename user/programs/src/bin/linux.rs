// SPDX-License-Identifier: GPL-3.0-only
//! The Linux world (milestone 8b, step 1; decisions 66, 92, 163 to 166).
//!
//! A Linux program runs here unchanged. Its threads "speak Linux": the
//! kernel turns every system call they make into a message to this
//! program, which answers it as Linux would. The Linux program has no
//! capabilities at all, so it cannot reach anything of Alaya's except
//! through these answers.
//!
//! Each Linux program gets its own address space, made from its own
//! piece of this program's memory (one revoke ends it and takes all of it
//! back). Every page it has is also mapped here, in a window kept for it,
//! so its memory can be read and written directly.
//!
//! The flight recorder (decision 165): the last requests of each program
//! are kept, and when it crashes the Linux world says in plain words what
//! it was doing just before.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::Write;

use alaya_programs::linux as lx;
use alaya_sys::abi::{FAULT_LABEL, LINUX_LABEL, StartInfo, linux, map, rights, start};
use alaya_sys::elf::Elf;
use alaya_sys::rt::Mem;
use alaya_sys::{At, Error, Kind, Msg, say};

alaya_sys::entry!(main);
alaya_sys::heap!();

/// Linux programs at once.
const PROCS: usize = 4;
/// Linux addresses are below this (512 GiB).
const LIMIT: u64 = 0x80_0000_0000;
/// Where each program's memory is seen here: program `i` at `WINDOW + i
/// * LIMIT`.
const WINDOW: u64 = 0x3000_0000_0000;
const STACK_TOP: u64 = 0x7f_ff00_0000;
const STACK_PAGES: u64 = 32;
/// Where anonymous memory goes, upwards.
const MMAP_BASE: u64 = 0x10_0000_0000;
/// Each program's own memory: 1 MiB, as a power of two.
const PROC_BITS: u8 = 20;
/// Requests the flight recorder keeps.
const RECORD: usize = 64;

const PAGE: u64 = 4096;

// Linux error numbers.
const EBADF: i64 = 9;
const ENOENT: i64 = 2;
const ENOMEM: i64 = 12;
const EFAULT: i64 = 14;
const EINVAL: i64 = 22;
const ENOTTY: i64 = 25;
const ENOSYS: i64 = 38;

/// The Linux calls the world knows, by meaning (the numbers differ
/// between processors).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[cfg_attr(target_arch = "aarch64", allow(dead_code))] // Open, ArchPrctl: PC only
enum Sys {
    Read,
    Write,
    Writev,
    Openat,
    Open,
    Close,
    Mmap,
    Munmap,
    Mprotect,
    Brk,
    Ioctl,
    Getpid,
    Gettid,
    Getuid,
    Uname,
    ArchPrctl,
    SetTidAddress,
    ClockGettime,
    SigAction,
    SigProcMask,
    Exit,
    ExitGroup,
    Unknown(u64),
}

#[cfg(target_arch = "x86_64")]
fn sys_of(nr: u64) -> Sys {
    match nr {
        0 => Sys::Read,
        1 => Sys::Write,
        2 => Sys::Open,
        3 => Sys::Close,
        9 => Sys::Mmap,
        10 => Sys::Mprotect,
        11 => Sys::Munmap,
        12 => Sys::Brk,
        13 => Sys::SigAction,
        14 => Sys::SigProcMask,
        16 => Sys::Ioctl,
        20 => Sys::Writev,
        39 => Sys::Getpid,
        60 => Sys::Exit,
        63 => Sys::Uname,
        102 | 104 | 107 | 108 => Sys::Getuid,
        158 => Sys::ArchPrctl,
        186 => Sys::Gettid,
        218 => Sys::SetTidAddress,
        228 => Sys::ClockGettime,
        231 => Sys::ExitGroup,
        257 => Sys::Openat,
        n => Sys::Unknown(n),
    }
}

#[cfg(target_arch = "aarch64")]
fn sys_of(nr: u64) -> Sys {
    match nr {
        29 => Sys::Ioctl,
        56 => Sys::Openat,
        57 => Sys::Close,
        63 => Sys::Read,
        64 => Sys::Write,
        66 => Sys::Writev,
        93 => Sys::Exit,
        94 => Sys::ExitGroup,
        96 => Sys::SetTidAddress,
        113 => Sys::ClockGettime,
        134 => Sys::SigAction,
        135 => Sys::SigProcMask,
        160 => Sys::Uname,
        172 => Sys::Getpid,
        174..=177 => Sys::Getuid,
        178 => Sys::Gettid,
        214 => Sys::Brk,
        215 => Sys::Munmap,
        222 => Sys::Mmap,
        226 => Sys::Mprotect,
        n => Sys::Unknown(n),
    }
}

#[cfg(target_arch = "x86_64")]
const MACHINE: &str = "x86_64";
#[cfg(target_arch = "aarch64")]
const MACHINE: &str = "aarch64";

/// One request, as the flight recorder keeps it.
#[derive(Clone, Copy)]
struct Seen {
    sys: Sys,
    args: [u64; 3],
    result: i64,
}

impl Seen {
    /// What it did, in plain words.
    fn words(&self, out: &mut String) {
        let [a, _, c] = self.args;
        let _ = match self.sys {
            Sys::Write if self.result >= 0 && (a == 1 || a == 2) => {
                write!(out, "wrote {} bytes to the screen", self.result)
            }
            Sys::Write | Sys::Writev => write!(out, "tried to write to descriptor {a}"),
            Sys::Read => write!(out, "read from descriptor {a}"),
            Sys::Open | Sys::Openat => write!(out, "tried to open a file"),
            Sys::Close => write!(out, "closed descriptor {a}"),
            Sys::Mmap if self.result >= 0 => match self.args[1].div_ceil(PAGE) {
                1 => write!(out, "asked for a page of memory"),
                n => write!(out, "asked for {n} pages of memory"),
            },
            Sys::Mmap => write!(out, "asked for memory and was refused"),
            Sys::Munmap => write!(out, "gave memory back"),
            Sys::Brk => write!(out, "moved its memory break"),
            Sys::Getpid | Sys::Gettid => write!(out, "asked who it is"),
            Sys::Getuid => write!(out, "asked which user it runs as"),
            Sys::Uname => write!(out, "asked which system this is"),
            Sys::ClockGettime => write!(out, "read the clock"),
            Sys::ArchPrctl | Sys::SetTidAddress => write!(out, "set up its threads"),
            Sys::Unknown(n) => write!(
                out,
                "asked for something the Linux world does not know yet (call {n})"
            ),
            _ => write!(out, "made a {:?} request ({c:#x})", self.sys),
        };
    }
}

struct Proc {
    name: &'static str,
    ut: u64,
    space: u64,
    thread: u64,
    fault: u64,
    sc: usize,
    /// Its pages: address -> (handle here, handle in its space).
    pages: BTreeMap<u64, (u64, u64)>,
    brk_start: u64,
    brk: u64,
    mmap_next: u64,
    /// What it printed and is not a whole line yet.
    line: Vec<u8>,
    out_bytes: u64,
    out_hash: u64,
    requests: u64,
    unknown: u64,
    record: [Option<Seen>; RECORD],
}

impl Proc {
    fn note(&mut self, s: Seen) {
        self.record[(self.requests as usize) % RECORD] = Some(s);
        self.requests += 1;
        if matches!(s.sys, Sys::Unknown(_)) {
            self.unknown += 1;
        }
    }

    /// The last `n` requests, oldest first.
    fn last(&self, n: usize) -> Vec<Seen> {
        let have = (self.requests as usize).min(RECORD).min(n);
        (0..have)
            .rev()
            .filter_map(|k| self.record[(self.requests as usize - 1 - k) % RECORD])
            .collect()
    }
}

struct World {
    mem: Mem,
    free: Vec<u64>,
    ep: u64,
    scs: [u64; PROCS],
    procs: [Option<Proc>; PROCS],
    priority: u64,
    clock_hz: u64,
    seed: u64,
}

fn at(index: u64) -> At {
    At {
        table: start::TABLE,
        index,
    }
}

/// FNV-1a over what a program printed (init compares it).
fn fnv(mut h: u64, bytes: &[u8]) -> u64 {
    for &b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    h
}

impl World {
    fn slot(&mut self) -> Result<u64, Error> {
        match self.free.pop() {
            Some(s) => Ok(s),
            None => self.mem.slot(),
        }
    }

    fn window(i: usize) -> u64 {
        WINDOW + i as u64 * LIMIT
    }

    /// A fresh zeroed page at Linux address `addr` of program `i`.
    fn map_page(&mut self, i: usize, addr: u64, flags: u64) -> Result<(), Error> {
        let p = self.procs[i].as_ref().unwrap();
        if p.pages.contains_key(&addr) {
            return Ok(());
        }
        if !(PAGE..LIMIT).contains(&addr) {
            return Err(Error::InvalidArgument);
        }
        let (ut, space) = (p.ut, p.space);
        let here = self.slot()?;
        alaya_sys::retype(ut, Kind::Page, 12, 1, at(here))?;
        alaya_sys::page_map(here, start::SPACE, Self::window(i) + addr, map::WRITE)?;
        let there = self.slot()?;
        alaya_sys::copy(at(there), at(here), rights::ALL)?;
        alaya_sys::page_map(there, space, addr, flags)?;
        self.procs[i]
            .as_mut()
            .unwrap()
            .pages
            .insert(addr, (here, there));
        Ok(())
    }

    fn unmap_page(&mut self, i: usize, addr: u64) {
        let Some((here, there)) = self.procs[i].as_mut().unwrap().pages.remove(&addr) else {
            return;
        };
        for h in [there, here] {
            let _ = alaya_sys::page_unmap(h);
            let _ = alaya_sys::delete(at(h));
            self.free.push(h);
        }
    }

    /// Program `i`'s memory from `addr`, `len` bytes, if all of it is
    /// mapped.
    fn bytes(&self, i: usize, addr: u64, len: u64) -> Option<&'static mut [u8]> {
        let p = self.procs[i].as_ref()?;
        let end = addr.checked_add(len)?;
        if end > LIMIT {
            return None;
        }
        let mut page = addr & !(PAGE - 1);
        while page < end {
            p.pages.get(&page)?;
            page += PAGE;
        }
        // SAFETY: every page of the range is mapped writable in this
        // program's window for program `i`, and nothing else uses it.
        Some(unsafe {
            core::slice::from_raw_parts_mut((Self::window(i) + addr) as *mut u8, len as usize)
        })
    }

    fn put(&self, i: usize, addr: u64, data: &[u8]) -> bool {
        match self.bytes(i, addr, data.len() as u64) {
            Some(b) => {
                b.copy_from_slice(data);
                true
            }
            None => false,
        }
    }

    /// Start a Linux program. Its name, arguments and environment are as
    /// given; its memory is its own piece of this program's.
    fn spawn(
        &mut self,
        i: usize,
        name: &'static str,
        image: &[u8],
        argv: &[&str],
        env: &[&str],
    ) -> Result<(), Error> {
        let elf = Elf::parse(image).ok_or(Error::InvalidArgument)?;
        let ut = self.slot()?;
        alaya_sys::retype(self.mem.ut, Kind::Untyped, PROC_BITS, 1, at(ut))?;
        let space = self.slot()?;
        alaya_sys::retype(ut, Kind::AddressSpace, 13, 1, at(space))?;
        let pool = self.slot()?;
        alaya_sys::retype(ut, Kind::Untyped, 16, 1, at(pool))?;
        alaya_sys::space_assign(space, pool)?;
        self.free.push(pool);
        let thread = self.slot()?;
        alaya_sys::retype(ut, Kind::Thread, 12, 1, at(thread))?;
        let fault = self.slot()?;
        alaya_sys::mint(at(fault), at(self.ep), rights::ALL, i as u64 + 1)?;
        let sc = i;
        self.procs[i] = Some(Proc {
            name,
            ut,
            space,
            thread,
            fault,
            sc,
            pages: BTreeMap::new(),
            brk_start: 0,
            brk: 0,
            mmap_next: MMAP_BASE,
            line: Vec::new(),
            out_bytes: 0,
            out_hash: 0xcbf2_9ce4_8422_2325,
            requests: 0,
            unknown: 0,
            record: [None; RECORD],
        });

        // Its program: each page once, with what all its segments need.
        let mut flags: BTreeMap<u64, u64> = BTreeMap::new();
        let mut top = 0;
        for s in elf.segments() {
            let mut f = 0;
            if s.write {
                f |= map::WRITE;
            }
            if s.execute {
                f |= map::EXECUTE;
            }
            let mut page = s.vaddr & !(PAGE - 1);
            while page < s.vaddr + s.mem_size {
                *flags.entry(page).or_insert(0) |= f;
                page += PAGE;
            }
            top = top.max(s.vaddr + s.mem_size);
        }
        for (&page, &f) in &flags {
            self.map_page(i, page, f)?;
        }
        let mut phdr = 0;
        let (phoff, phent, phnum) = elf.headers();
        for s in elf.segments() {
            let b = self
                .bytes(i, s.vaddr, s.file_size as u64)
                .ok_or(Error::NoMemory)?;
            b.copy_from_slice(&elf.bytes[s.offset..s.offset + s.file_size]);
            if phoff >= s.offset && phoff < s.offset + s.file_size {
                phdr = s.vaddr + (phoff - s.offset) as u64;
            }
        }
        let brk = top.next_multiple_of(PAGE);
        {
            let p = self.procs[i].as_mut().unwrap();
            p.brk_start = brk;
            p.brk = brk;
        }

        // Its stack: strings at the top, then the vectors Linux programs
        // expect (argument count, arguments, environment, auxiliary).
        for k in 0..STACK_PAGES {
            self.map_page(i, STACK_TOP - (k + 1) * PAGE, map::WRITE)?;
        }
        let mut sp = STACK_TOP;
        let mut push_str = |w: &World, s: &str| -> u64 {
            sp -= s.len() as u64 + 1;
            w.put(i, sp, s.as_bytes());
            w.put(i, sp + s.len() as u64, &[0]);
            sp
        };
        let mut arg_at = Vec::new();
        for a in argv {
            arg_at.push(push_str(self, a));
        }
        let mut env_at = Vec::new();
        for e in env {
            env_at.push(push_str(self, e));
        }
        let platform = push_str(self, MACHINE);
        sp &= !15;
        sp -= 16;
        let random = sp;
        self.seed = self
            .seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let r = [self.seed, self.seed.rotate_left(29) ^ alaya_sys::clock()];
        self.put(i, random, &r[0].to_le_bytes());
        self.put(i, random + 8, &r[1].to_le_bytes());
        let aux: [(u64, u64); 13] = [
            (3, phdr),
            (4, phent as u64),
            (5, phnum as u64),
            (6, PAGE),
            (9, elf.entry),
            (11, 1000),
            (12, 1000),
            (13, 1000),
            (14, 1000),
            (15, platform),
            (23, 0),
            (25, random),
            (0, 0),
        ];
        let mut words: Vec<u64> = Vec::new();
        words.push(argv.len() as u64);
        words.extend(&arg_at);
        words.push(0);
        words.extend(&env_at);
        words.push(0);
        for (k, v) in aux {
            words.push(k);
            words.push(v);
        }
        sp -= 8 * words.len() as u64;
        sp &= !15;
        for (k, w) in words.iter().enumerate() {
            self.put(i, sp + 8 * k as u64, &w.to_le_bytes());
        }

        alaya_sys::thread_configure(thread, 0, space, fault)?;
        alaya_sys::thread_priority(thread, self.priority, self.priority)?;
        alaya_sys::thread_write(thread, elf.entry, sp, 0, 0, 0)?;
        alaya_sys::thread_linux(thread, linux::ON, 0, 0)?;
        alaya_sys::sc_bind(self.scs[sc], thread)?;
        alaya_sys::thread_start(thread)?;
        Ok(())
    }

    /// End program `i`: everything it had goes back at once.
    fn end(&mut self, i: usize) -> Proc {
        let p = self.procs[i].take().unwrap();
        let _ = alaya_sys::thread_stop(p.thread);
        let _ = alaya_sys::sc_unbind(self.scs[p.sc]);
        // Everything made from its memory (its pages, here and there, its
        // address space and thread) goes with one revoke.
        let _ = alaya_sys::revoke(at(p.ut));
        let _ = alaya_sys::delete(at(p.ut));
        let _ = alaya_sys::delete(at(p.fault));
        self.free.push(p.ut);
        self.free.push(p.fault);
        for (here, there) in p.pages.values() {
            self.free.push(*here);
            self.free.push(*there);
        }
        self.free.push(p.space);
        self.free.push(p.thread);
        p
    }

    /// Bytes a program printed: whole lines are shown.
    fn printed(&mut self, i: usize, data: &[u8]) {
        let p = self.procs[i].as_mut().unwrap();
        p.out_bytes += data.len() as u64;
        p.out_hash = fnv(p.out_hash, data);
        for &b in data {
            if b == b'\n' {
                let line = core::str::from_utf8(&p.line).unwrap_or("(not text)");
                say!("{}> {}", p.name, line);
                p.line.clear();
            } else if p.line.len() < 200 {
                p.line.push(b);
            }
        }
    }

    /// Answer one Linux request of program `i`.
    fn request(&mut self, i: usize, nr: u64, a: [u64; 6]) -> Option<u64> {
        let sys = sys_of(nr);
        let r: i64 = match sys {
            Sys::Write => self.write(i, a[0], a[1], a[2]),
            Sys::Writev => {
                let mut total = 0;
                let mut bad = false;
                for k in 0..a[2].min(64) {
                    let Some(v) = self.bytes(i, a[1] + 16 * k, 16) else {
                        bad = true;
                        break;
                    };
                    let base = u64::from_le_bytes(v[..8].try_into().unwrap());
                    let len = u64::from_le_bytes(v[8..].try_into().unwrap());
                    let n = self.write(i, a[0], base, len);
                    if n < 0 {
                        bad = total == 0;
                        if bad {
                            total = n;
                        }
                        break;
                    }
                    total += n;
                }
                if bad && total == 0 { -EFAULT } else { total }
            }
            Sys::Read => match a[0] {
                0 => 0,
                _ => -EBADF,
            },
            Sys::Open | Sys::Openat => -ENOENT,
            Sys::Close => match a[0] {
                0..=2 => 0,
                _ => -EBADF,
            },
            Sys::Brk => self.brk(i, a[0]) as i64,
            Sys::Mmap => self.mmap(i, a),
            Sys::Munmap => {
                if !a[0].is_multiple_of(PAGE) {
                    -EINVAL
                } else {
                    let mut page = a[0];
                    while page < a[0].saturating_add(a[1]).min(LIMIT) {
                        self.unmap_page(i, page);
                        page += PAGE;
                    }
                    0
                }
            }
            Sys::Mprotect => 0,
            Sys::Ioctl => -ENOTTY,
            Sys::Getpid | Sys::Gettid | Sys::SetTidAddress => 100 + i as i64,
            Sys::Getuid => 1000,
            Sys::Uname => {
                let mut u = [0u8; 65 * 6];
                for (k, s) in [
                    "Linux",
                    "alaya",
                    "6.1.0-alaya",
                    "#1 Alaya",
                    MACHINE,
                    "(none)",
                ]
                .iter()
                .enumerate()
                {
                    u[65 * k..65 * k + s.len()].copy_from_slice(s.as_bytes());
                }
                if self.put(i, a[0], &u) { 0 } else { -EFAULT }
            }
            Sys::ClockGettime => {
                let t = alaya_sys::clock();
                let hz = self.clock_hz.max(1);
                let sec = t / hz;
                let nsec = ((t % hz) as u128 * 1_000_000_000 / hz as u128) as u64;
                let mut b = [0u8; 16];
                b[..8].copy_from_slice(&sec.to_le_bytes());
                b[8..].copy_from_slice(&nsec.to_le_bytes());
                if self.put(i, a[1], &b) { 0 } else { -EFAULT }
            }
            Sys::ArchPrctl => match a[0] {
                // ARCH_SET_FS
                0x1002 if a[1] < LIMIT => {
                    let t = self.procs[i].as_ref().unwrap().thread;
                    match alaya_sys::thread_linux(t, linux::TLS, a[1], 0) {
                        Ok(()) => 0,
                        Err(_) => -EINVAL,
                    }
                }
                _ => -EINVAL,
            },
            Sys::SigAction | Sys::SigProcMask => 0,
            Sys::Exit | Sys::ExitGroup => {
                let p = self.procs[i].as_mut().unwrap();
                p.note(Seen {
                    sys,
                    args: [a[0], a[1], a[2]],
                    result: 0,
                });
                return None;
            }
            Sys::Unknown(_) => -ENOSYS,
        };
        let p = self.procs[i].as_mut().unwrap();
        p.note(Seen {
            sys,
            args: [a[0], a[1], a[2]],
            result: r,
        });
        Some(r as u64)
    }

    fn write(&mut self, i: usize, fd: u64, buf: u64, len: u64) -> i64 {
        if fd != 1 && fd != 2 {
            return -EBADF;
        }
        let len = len.min(1 << 20);
        let Some(b) = self.bytes(i, buf, len) else {
            return -EFAULT;
        };
        self.printed(i, b);
        len as i64
    }

    fn brk(&mut self, i: usize, want: u64) -> u64 {
        let (start, now) = {
            let p = self.procs[i].as_ref().unwrap();
            (p.brk_start, p.brk)
        };
        if want < start || want >= MMAP_BASE {
            return now;
        }
        let mut page = now;
        while page < want {
            if self.map_page(i, page, map::WRITE).is_err() {
                return now;
            }
            page += PAGE;
        }
        let new = want.next_multiple_of(PAGE);
        self.procs[i].as_mut().unwrap().brk = new.max(now);
        want
    }

    fn mmap(&mut self, i: usize, a: [u64; 6]) -> i64 {
        const PROT_WRITE: u64 = 2;
        const PROT_EXEC: u64 = 4;
        const MAP_FIXED: u64 = 0x10;
        const MAP_ANONYMOUS: u64 = 0x20;
        let (hint, len, prot, flags) = (a[0], a[1], a[2], a[3]);
        if len == 0 || flags & MAP_ANONYMOUS == 0 {
            return if len == 0 { -EINVAL } else { -ENODEV_FILES };
        }
        let len = len.next_multiple_of(PAGE);
        let base = if flags & MAP_FIXED != 0 {
            if !hint.is_multiple_of(PAGE) {
                return -EINVAL;
            }
            let mut page = hint;
            while page < hint + len {
                self.unmap_page(i, page);
                page += PAGE;
            }
            hint
        } else {
            let p = self.procs[i].as_mut().unwrap();
            let b = p.mmap_next;
            p.mmap_next += len + PAGE;
            b
        };
        if base + len > STACK_TOP - STACK_PAGES * PAGE {
            return -ENOMEM;
        }
        let mut f = 0;
        if prot & PROT_WRITE != 0 {
            f |= map::WRITE;
        }
        if prot & PROT_EXEC != 0 {
            f |= map::EXECUTE;
        }
        let mut page = base;
        while page < base + len {
            if self.map_page(i, page, f).is_err() {
                return -ENOMEM;
            }
            page += PAGE;
        }
        base as i64
    }

    /// Program `i` crashed: what it was doing, in plain words.
    fn crashed(&mut self, i: usize, w: [u64; 6]) {
        let p = self.procs[i].as_ref().unwrap();
        let addr = w[2];
        let mut s = String::new();
        let _ = write!(s, "{} crashed. Just before, it ", p.name);
        let last = p.last(5);
        for (k, seen) in last.iter().enumerate() {
            if k > 0 {
                s.push_str(", ");
            }
            seen.words(&mut s);
        }
        if last.is_empty() {
            s.push_str("had not asked for anything yet");
        }
        let page = addr & !(PAGE - 1);
        if p.pages.contains_key(&page) {
            let _ = write!(
                s,
                "; then it used its memory at {addr:#x} in a way it may not"
            );
        } else {
            let _ = write!(
                s,
                "; then it touched memory at {addr:#x} that it never asked for"
            );
        }
        say!("flight record: {s}");
    }
}

/// A Linux test program carried by the world.
struct Test {
    name: &'static str,
    image: &'static [u8],
}

const TESTS: [Test; 3] = [
    Test {
        name: "hello",
        image: include_bytes!(concat!(env!("OUT_DIR"), "/linux-hello")),
    },
    Test {
        name: "sandbox",
        image: include_bytes!(concat!(env!("OUT_DIR"), "/linux-sandbox")),
    },
    Test {
        name: "crash",
        image: include_bytes!(concat!(env!("OUT_DIR"), "/linux-crash")),
    },
];

/// Linux's ENODEV, for file mappings (no files yet).
const ENODEV_FILES: i64 = 19;

fn main(info: &'static StartInfo) -> ! {
    let g = start::FIRST_GIVEN;
    let mut mem = Mem::new(info);
    alaya_sys::heap::init(&mut mem, 1).expect("heap");
    let ep = g + lx::G_WORLD;
    let mut scs = [0; PROCS];
    for (k, s) in scs.iter_mut().enumerate() {
        *s = g + lx::G_SCS + k as u64;
    }
    let replies = [
        mem.make(Kind::Reply, 0).expect("reply"),
        mem.make(Kind::Reply, 0).expect("reply"),
    ];
    let mut w = World {
        mem,
        free: Vec::new(),
        ep,
        scs,
        procs: [const { None }; PROCS],
        priority: info.priority,
        clock_hz: info.clock_hz,
        seed: alaya_sys::clock() | 1,
    };
    // Who asked for each running program, waiting for it to end.
    let mut waiting: [Option<u64>; PROCS] = [None; PROCS];
    let mut cur = 0;
    say!("the Linux world is ready ({MACHINE})");
    loop {
        let rep = replies[cur];
        let m = match alaya_sys::recv(ep, rep) {
            Ok(m) => m,
            Err(e) => {
                say!("receive failed: {e:?}");
                continue;
            }
        };
        let label = m.label();
        if m.badge == 0 {
            // Alaya asks: run a Linux program and answer when it ends.
            if label != lx::RUN {
                let _ = alaya_sys::reply(rep, &Msg::new(lx::BAD, &[]));
                continue;
            }
            let Some(t) = TESTS.get(m.words[0] as usize) else {
                let _ = alaya_sys::reply(rep, &Msg::new(lx::BAD, &[]));
                continue;
            };
            let Some(i) = (0..PROCS).find(|&k| w.procs[k].is_none() && waiting[k].is_none()) else {
                let _ = alaya_sys::reply(rep, &Msg::new(lx::BUSY, &[]));
                continue;
            };
            let mut argv = [t.name; 1 + lx::ARGS.len()];
            argv[1..].copy_from_slice(&lx::ARGS);
            match w.spawn(i, t.name, t.image, &argv, &lx::ENV) {
                Ok(()) => {
                    // The answer waits for the end: this reply object stays
                    // with the asker, and the other one takes its place.
                    waiting[i] = Some(rep);
                    cur = 1 - cur;
                    if waiting.iter().flatten().count() > 1 {
                        say!("only one Linux program can be waited for at a time");
                    }
                }
                Err(e) => {
                    say!("{} did not start: {e:?}", t.name);
                    if w.procs[i].is_some() {
                        w.end(i);
                    }
                    let _ = alaya_sys::reply(rep, &Msg::new(lx::BAD, &[]));
                }
            }
            continue;
        }
        let i = (m.badge - 1) as usize;
        if i >= PROCS || w.procs[i].is_none() {
            continue;
        }
        let ended = if label == FAULT_LABEL {
            w.crashed(i, m.words);
            Some(128 + 11)
        } else if (LINUX_LABEL..LINUX_LABEL + linux::CALLS).contains(&label) {
            match w.request(i, label - LINUX_LABEL, m.words) {
                Some(r) => {
                    let _ = alaya_sys::reply(rep, &Msg::new(0, &[r]));
                    None
                }
                None => Some(m.words[0] & 0xff),
            }
        } else {
            None
        };
        if let Some(code) = ended {
            let p = w.end(i);
            if !p.line.is_empty() {
                say!(
                    "{}> {}",
                    p.name,
                    core::str::from_utf8(&p.line).unwrap_or("?")
                );
            }
            say!(
                "{} ended with {code}: {} requests, {} unknown, {} bytes printed",
                p.name,
                p.requests,
                p.unknown,
                p.out_bytes
            );
            if let Some(asker) = waiting[i].take() {
                let _ = alaya_sys::reply(
                    asker,
                    &Msg::new(
                        lx::DONE,
                        &[code, p.out_bytes, p.out_hash, p.requests, p.unknown],
                    ),
                );
                // That reply object is free again.
                cur = replies.iter().position(|&r| r == asker).unwrap_or(cur);
            }
        }
    }
}
