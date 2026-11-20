// SPDX-License-Identifier: GPL-3.0-only
//! The small runtime of a started program: its entry point, printing with
//! its name in front, a crash on panic (so whoever looks after it notices),
//! its own memory, threads and a clock.

use core::fmt::{self, Write};
use core::sync::atomic::{AtomicU64, Ordering};

use crate::abi::{StartInfo, map, start};
use crate::{At, Error, Kind};

static mut NAME: [u8; 32] = [0; 32];
static NAME_LEN: AtomicU64 = AtomicU64::new(0);
static CLOCK_HZ: AtomicU64 = AtomicU64::new(1_000_000_000);

/// Set up the runtime from the start information. Called by [`entry!`].
pub fn init(info: &StartInfo) {
    let n = info.name.iter().position(|&b| b == 0).unwrap_or(32);
    // SAFETY: written once, before any other thread of the program runs.
    unsafe { (&mut *core::ptr::addr_of_mut!(NAME))[..n].copy_from_slice(&info.name[..n]) };
    NAME_LEN.store(n as u64, Ordering::Relaxed);
    CLOCK_HZ.store(info.clock_hz.max(1), Ordering::Relaxed);
}

/// The program's name.
pub fn name() -> &'static str {
    let n = NAME_LEN.load(Ordering::Relaxed) as usize;
    // SAFETY: written once by `init`.
    let b = unsafe { &(&*core::ptr::addr_of!(NAME))[..n] };
    core::str::from_utf8(b).unwrap_or("?")
}

/// Milliseconds since boot, from the time-stamp counter.
pub fn now_ms() -> u64 {
    let hz = CLOCK_HZ.load(Ordering::Relaxed);
    (crate::clock() as u128 * 1000 / hz as u128) as u64
}

struct Line {
    buf: [u8; 240],
    len: usize,
}

impl Write for Line {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let n = s.len().min(self.buf.len() - self.len);
        self.buf[self.len..self.len + n].copy_from_slice(&s.as_bytes()[..n]);
        self.len += n;
        Ok(())
    }
}

/// Print one line, with the program's name in front.
pub fn say(args: fmt::Arguments) {
    let mut line = Line {
        buf: [0; 240],
        len: 0,
    };
    let _ = write!(line, "{}: ", name());
    let _ = line.write_fmt(args);
    let _ = line.write_str("\n");
    // A long line may have been cut inside a character: keep what is whole.
    let b = &line.buf[..line.len];
    let text = match core::str::from_utf8(b) {
        Ok(t) => t,
        Err(e) => core::str::from_utf8(&b[..e.valid_up_to()]).unwrap_or("?"),
    };
    crate::print(text);
}

#[macro_export]
macro_rules! say {
    ($($arg:tt)*) => ($crate::rt::say(format_args!($($arg)*)));
}

/// What a panic does: print it, then crash with an invalid instruction so
/// the program's fault handler (its keeper) sees it.
pub fn panic(info: &core::panic::PanicInfo) -> ! {
    say(format_args!("PANIC: {}", info.message()));
    if let Some(at) = info.location() {
        say(format_args!("  at {}:{}", at.file(), at.line()));
    }
    // SAFETY: deliberately faults.
    unsafe { core::arch::asm!("ud2", options(noreturn)) }
}

/// Define `_start` and the panic handler of a program whose main function
/// is `$main(&'static StartInfo) -> !`.
#[macro_export]
macro_rules! entry {
    ($main:path) => {
        core::arch::global_asm!(
            ".section .text._start, \"ax\"",
            ".global _start",
            "_start:",
            "    and rsp, -16",
            "    call {main}",
            "    ud2",
            main = sym __alaya_start,
        );

        extern "C" fn __alaya_start(info: &'static $crate::abi::StartInfo) -> ! {
            if info.magic != $crate::abi::start::MAGIC {
                // SAFETY: no start information: crash at once.
                unsafe { core::arch::asm!("ud2", options(noreturn)) }
            }
            $crate::rt::init(info);
            $main(info)
        }

        #[panic_handler]
        fn panic(info: &core::panic::PanicInfo) -> ! {
            $crate::rt::panic(info)
        }
    };
}

/// A program's own memory: objects and pages from its untyped, in its own
/// table's free slots.
pub struct Mem {
    pub ut: u64,
    pub next: u64,
    pub end: u64,
    /// Next free address for stacks and buffers.
    pub heap: u64,
}

/// Where a program's own pages and thread stacks go.
pub const HEAP_BASE: u64 = 0x1000_0000_0000;

impl Mem {
    pub fn new(info: &StartInfo) -> Mem {
        Mem {
            ut: start::UNTYPED,
            next: info.first_free,
            end: info.table_slots,
            heap: HEAP_BASE,
        }
    }

    pub fn at(&self, index: u64) -> At {
        At {
            table: start::TABLE,
            index,
        }
    }

    pub fn slot(&mut self) -> Result<u64, Error> {
        if self.next >= self.end {
            return Err(Error::NoMemory);
        }
        self.next += 1;
        Ok(self.next - 1)
    }

    pub fn make(&mut self, kind: Kind, bits: u8) -> Result<u64, Error> {
        let h = self.slot()?;
        crate::retype(self.ut, kind, bits, 1, self.at(h))?;
        Ok(h)
    }

    /// `pages` fresh zeroed pages, mapped writable at a new address.
    pub fn pages(&mut self, pages: u64) -> Result<u64, Error> {
        let base = self.heap;
        for i in 0..pages {
            let p = self.make(Kind::Page, 12)?;
            crate::page_map(p, start::SPACE, base + i * 4096, map::WRITE)?;
        }
        // Leave an unmapped page after each block, so overruns fault.
        self.heap += (pages + 1) * 4096;
        Ok(base)
    }

    /// One fresh zeroed page, mapped writable: its handle and address.
    pub fn page(&mut self) -> Result<(u64, u64), Error> {
        let base = self.heap;
        let p = self.make(Kind::Page, 12)?;
        crate::page_map(p, start::SPACE, base, map::WRITE)?;
        self.heap += 2 * 4096;
        Ok((p, base))
    }

    /// A new address range of `pages` pages, not mapped yet.
    pub fn window(&mut self, pages: u64) -> u64 {
        let base = self.heap;
        self.heap += (pages + 1) * 4096;
        base
    }

    /// Start a thread of this program running `entry(arg)` on the
    /// scheduling context `sc`, at `priority`.
    pub fn thread(
        &mut self,
        entry: extern "C" fn(u64) -> !,
        arg: u64,
        sc: u64,
        priority: u64,
    ) -> Result<u64, Error> {
        let t = self.make(Kind::Thread, 12)?;
        let stack = self.pages(4)? + 4 * 4096;
        crate::thread_configure(t, start::TABLE, start::SPACE, 0)?;
        crate::thread_priority(t, priority, priority)?;
        crate::thread_write(t, entry as *const () as u64, stack - 8, arg, 0, 0)?;
        crate::sc_bind(sc, t)?;
        crate::thread_start(t)?;
        Ok(t)
    }
}

/// The clock: a thread on the clock scheduling context the parent gave
/// (a small budget every period) that wakes once per period and signals
/// `bits` on notification `ntfn` every `every` periods. This is how a
/// program waits for time to pass until the system has a timer service.
pub fn start_clock(
    mem: &mut Mem,
    ntfn: u64,
    bits: u64,
    every: u64,
    priority: u64,
) -> Result<(), Error> {
    CLOCK.store(ntfn | bits << 16 | every.max(1) << 40, Ordering::Relaxed);
    mem.thread(clock, 0, start::CLOCK_SC, priority).map(|_| ())
}

static CLOCK: AtomicU64 = AtomicU64::new(0);
/// Periods the clock has seen.
pub static TICKS: AtomicU64 = AtomicU64::new(0);

extern "C" fn clock(_: u64) -> ! {
    let c = CLOCK.load(Ordering::Relaxed);
    let (ntfn, bits, every) = (c & 0xffff, (c >> 16) & 0xff_ffff, c >> 40);
    loop {
        // Gives up the rest of this period's budget: wakes at the next.
        crate::yield_now();
        let t = TICKS.fetch_add(1, Ordering::Relaxed) + 1;
        if t.is_multiple_of(every) {
            let _ = crate::signal(ntfn, bits);
        }
    }
}
