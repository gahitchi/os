// SPDX-License-Identifier: GPL-3.0-only
//! The few things a program does that depend on the processor family: the
//! system call instruction and its registers, the clock, a deliberate
//! crash, and the first instruction of a program. Everything else in the
//! SDK is the same on every family.
//!
//! | role                 | x86-64 (PC) | ARM64      |
//! |----------------------|-------------|------------|
//! | instruction          | `syscall`   | `svc #0`   |
//! | call number / status | rax         | x8         |
//! | handle / badge       | rdi         | x0         |
//! | info / label         | rsi         | x1         |
//! | six data words       | rdx r10 r8 r9 r12 r13 | x2 to x7 |
//! | capability slots     | r14         | x9         |
//! | reply handle         | r15         | x10        |

/// The machine number in program files (ELF `e_machine`) this family runs.
#[cfg(target_arch = "x86_64")]
pub const ELF_MACHINE: u16 = 62;
#[cfg(target_arch = "aarch64")]
pub const ELF_MACHINE: u16 = 183;

/// The fault number a program gets when it tries an instruction only the
/// kernel may use, such as reading where the kernel's tables are: a
/// general protection fault on a PC, an undefined instruction on ARM.
#[cfg(target_arch = "x86_64")]
pub const PRIVILEGED_FAULT: u64 = 13;
#[cfg(target_arch = "aarch64")]
pub const PRIVILEGED_FAULT: u64 = 6;

/// The stack pointer a new thread starts with, given the top of its
/// stack: a PC's functions expect the return address just pushed, ARM's a
/// 16-byte aligned stack.
pub const fn entry_stack(top: u64) -> u64 {
    if cfg!(target_arch = "x86_64") {
        top - 8
    } else {
        top & !15
    }
}

/// One system call with every register of the message ABI: the call
/// number, the handle, the info word, six data words, the capability slots
/// and the reply handle. Returns the status, the badge, the info word and
/// the six words as the kernel left them.
#[inline(always)]
pub fn raw_call(
    nr: u64,
    handle: u64,
    info: u64,
    w: [u64; 6],
    caps: u64,
    reply: u64,
) -> (u64, u64, u64, [u64; 6]) {
    let (err, badge, info_out, w0, w1, w2, w3, w4, w5);
    #[cfg(target_arch = "x86_64")]
    // SAFETY: the kernel keeps every register but rcx, r11 and the outputs.
    unsafe {
        core::arch::asm!(
            "syscall",
            inout("rax") nr => err,
            inout("rdi") handle => badge,
            inout("rsi") info => info_out,
            inout("rdx") w[0] => w0,
            inout("r10") w[1] => w1,
            inout("r8") w[2] => w2,
            inout("r9") w[3] => w3,
            inout("r12") w[4] => w4,
            inout("r13") w[5] => w5,
            in("r14") caps,
            in("r15") reply,
            out("rcx") _,
            out("r11") _,
            options(nostack),
        );
    }
    #[cfg(target_arch = "aarch64")]
    // SAFETY: the kernel keeps every register but the outputs.
    unsafe {
        core::arch::asm!(
            "svc #0",
            inout("x8") nr => err,
            inout("x0") handle => badge,
            inout("x1") info => info_out,
            inout("x2") w[0] => w0,
            inout("x3") w[1] => w1,
            inout("x4") w[2] => w2,
            inout("x5") w[3] => w3,
            inout("x6") w[4] => w4,
            inout("x7") w[5] => w5,
            in("x9") caps,
            in("x10") reply,
            options(nostack),
        );
    }
    (err, badge, info_out, [w0, w1, w2, w3, w4, w5])
}

/// The clock, in ticks (`BootInfo::clock_hz` per second): the time-stamp
/// counter on a PC, the generic timer's virtual count on ARM.
#[inline]
pub fn clock() -> u64 {
    #[cfg(target_arch = "x86_64")]
    // SAFETY: reading the TSC has no side effects.
    unsafe {
        core::arch::x86_64::_rdtsc()
    }
    #[cfg(target_arch = "aarch64")]
    {
        let t: u64;
        // SAFETY: the kernel lets programs read the virtual count.
        unsafe {
            core::arch::asm!("isb", "mrs {}, cntvct_el0", out(reg) t, options(nomem, nostack))
        };
        t
    }
}

/// Crash on purpose with an invalid instruction, so that the thread's
/// fault handler sees it.
#[inline(always)]
pub fn crash() -> ! {
    // SAFETY: deliberately faults.
    unsafe {
        #[cfg(target_arch = "x86_64")]
        core::arch::asm!("ud2", options(noreturn));
        #[cfg(target_arch = "aarch64")]
        core::arch::asm!("udf #0", options(noreturn));
    }
}

/// Try an instruction only the kernel may use: read where the kernel's
/// interrupt table is (`sidt` on a PC, `vbar_el1` on ARM). The processor
/// must stop the program with [`PRIVILEGED_FAULT`].
#[inline(always)]
pub fn privileged_read() -> u64 {
    #[cfg(target_arch = "x86_64")]
    {
        let mut buf = [0u8; 16];
        // SAFETY: SIDT writes 10 bytes into the buffer (or faults).
        unsafe { core::arch::asm!("sidt [{}]", in(reg) buf.as_mut_ptr(), options(nostack)) };
        u64::from_le_bytes(buf[2..10].try_into().unwrap())
    }
    #[cfg(target_arch = "aarch64")]
    {
        let v: u64;
        // SAFETY: faults in user mode.
        unsafe { core::arch::asm!("mrs {}, vbar_el1", out(reg) v, options(nomem, nostack)) };
        v
    }
}

/// The first instructions of a program: align the stack, call `$main`
/// with the start information the kernel passed in the first argument
/// register, and crash if it ever returns.
#[macro_export]
macro_rules! start_code {
    ($main:path) => {
        #[cfg(target_arch = "x86_64")]
        core::arch::global_asm!(
            ".section .text._start, \"ax\"",
            ".global _start",
            "_start:",
            "    and rsp, -16",
            "    call {main}",
            "    ud2",
            main = sym $main,
        );
        #[cfg(target_arch = "aarch64")]
        core::arch::global_asm!(
            ".section .text._start, \"ax\"",
            ".global _start",
            "_start:",
            "    mov x16, sp",
            "    and x16, x16, #-16",
            "    mov sp, x16",
            "    mov x29, xzr",
            "    mov x30, xzr",
            "    bl {main}",
            "    udf #0",
            main = sym $main,
        );
    };
}
