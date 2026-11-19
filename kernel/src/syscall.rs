// SPDX-License-Identifier: GPL-3.0-only
//! System calls: the `syscall` instruction's entry path and dispatch.
//!
//! The register layout is described in `alaya-abi`. The entry code swaps
//! in the kernel's GS base, switches to this core's kernel stack and saves
//! every register in a [`SyscallFrame`]; the dispatcher takes the big
//! kernel lock and runs the call. A call that was interrupted (a long
//! revoke or retype) is restarted: the saved instruction pointer is moved
//! back onto the `syscall` instruction, so the program runs it again with
//! the same registers once pending interrupts were handled.

use core::arch::global_asm;

use alaya_abi::{Error, call, debug};

use crate::arch::cpu::{self, msr};
use crate::arch::gdt::{KERNEL_CODE, USER_DATA};
use crate::caps::{self, Outcome};
use crate::percpu::{KERNEL_RSP_OFFSET, USER_RSP_OFFSET};
use crate::{kprint, kprintln, sync};

/// Registers saved on entry, lowest address first.
#[repr(C)]
#[derive(Debug)]
pub struct SyscallFrame {
    pub r15: u64,
    pub r14: u64,
    pub r13: u64,
    pub r12: u64,
    pub r10: u64,
    pub r9: u64,
    pub r8: u64,
    pub rbp: u64,
    pub rdi: u64,
    pub rsi: u64,
    pub rdx: u64,
    pub rbx: u64,
    pub rax: u64,
    /// From RCX: where the program continues.
    pub rip: u64,
    /// From R11.
    pub rflags: u64,
    pub rsp: u64,
}

global_asm!(
    r#"
    .section .text.syscall, "ax"
    .global alaya_syscall_entry
alaya_syscall_entry:
    swapgs
    mov gs:[{user_rsp}], rsp
    mov rsp, gs:[{kernel_rsp}]
    push qword ptr gs:[{user_rsp}]
    push r11
    push rcx
    push rax
    push rbx
    push rdx
    push rsi
    push rdi
    push rbp
    push r8
    push r9
    push r10
    push r12
    push r13
    push r14
    push r15
    cld
    mov rdi, rsp
    call {dispatch}
    pop r15
    pop r14
    pop r13
    pop r12
    pop r10
    pop r9
    pop r8
    pop rbp
    pop rdi
    pop rsi
    pop rdx
    pop rbx
    pop rax
    pop rcx
    pop r11
    pop rsp
    swapgs
    sysretq
    "#,
    user_rsp = const USER_RSP_OFFSET,
    kernel_rsp = const KERNEL_RSP_OFFSET,
    dispatch = sym dispatch,
);

unsafe extern "C" {
    fn alaya_syscall_entry();
}

/// RFLAGS bits cleared on entry: interrupts, direction, trap, alignment
/// check. The kernel runs with interrupts off.
const FMASK: u64 = (1 << 9) | (1 << 10) | (1 << 8) | (1 << 18);

/// Turn on `syscall` on this core.
pub fn init_this_core() {
    // SAFETY: these MSRs only configure the syscall instruction; the
    // selectors match the GDT layout (see gdt.rs).
    unsafe {
        let star = (u64::from(USER_DATA & !3) - 8) << 48 | u64::from(KERNEL_CODE) << 32;
        cpu::wrmsr(msr::STAR, star);
        cpu::wrmsr(msr::LSTAR, alaya_syscall_entry as *const () as u64);
        cpu::wrmsr(msr::FMASK, FMASK);
        let efer = cpu::rdmsr(msr::EFER);
        cpu::wrmsr(msr::EFER, efer | cpu::EFER_SCE);
    }
}

/// Length of the `syscall` instruction, for restarts.
const SYSCALL_LEN: u64 = 2;

/// User addresses are below this; the last page stays unmapped so that a
/// `syscall` at the very top cannot return to a non-canonical address.
pub const USER_END: u64 = 0x0000_7fff_ffff_f000;

extern "C" fn dispatch(f: &mut SyscallFrame) {
    let mut k = sync::BKL.lock();
    let result = match f.rax {
        call::CALL => {
            let root = k.init.expect("system call without a program").root;
            let args = [f.rdx, f.r10, f.r8, f.r9, f.r12, f.r13];
            match caps::invoke(&mut k, root, f.rdi, f.rsi, args) {
                Ok(Outcome::Done(r)) => Ok(r),
                Ok(Outcome::Restart) => {
                    k.restarts += 1;
                    f.rip -= SYSCALL_LEN;
                    return;
                }
                Err(e) => Err(e),
            }
        }
        call::YIELD => Ok([0; 4]),
        call::DEBUG => match f.rdi {
            debug::PRINT => print(&k, f.rdx, f.rsi).map(|()| [0; 4]),
            debug::REPORT => {
                drop(k);
                report(f.rsi)
            }
            debug::COUNTERS => Ok([k.restarts, k.caps.revoked, 0, 0]),
            _ => Err(Error::NoSuchCall),
        },
        _ => Err(Error::NoSuchCall),
    };
    match result {
        Ok(r) => {
            f.rax = 0;
            [f.rdx, f.r10, f.r8, f.r9] = r;
        }
        Err(e) => f.rax = e as u64,
    }
    assert!(
        f.rip < USER_END,
        "return address {:#x} outside user space",
        f.rip
    );
}

/// Print a program's message on the serial port, prefixed with "init: ".
fn print(k: &sync::KernelState, at: u64, len: u64) -> Result<(), Error> {
    if len > debug::MAX_PRINT as u64 {
        return Err(Error::InvalidArgument);
    }
    let space = &k.init.ok_or(Error::NoSuchCall)?.space;
    let mut buf = [0u8; debug::MAX_PRINT];
    for i in 0..len {
        let addr = at.checked_add(i).filter(|&a| a < USER_END);
        let (phys, flags) = addr
            .and_then(|a| space.translate(a))
            .ok_or(Error::InvalidArgument)?;
        if flags & crate::mm::paging::USER == 0 {
            return Err(Error::InvalidArgument);
        }
        // SAFETY: the byte is mapped for the program, so it is RAM the
        // direct map covers.
        buf[i as usize] = unsafe { *((phys + crate::limine::hhdm_offset().unwrap()) as *const u8) };
    }
    let text = core::str::from_utf8(&buf[..len as usize]).unwrap_or("(not UTF-8)");
    kprint!("init: {text}");
    Ok(())
}

/// The first program finished its tests.
fn report(status: u64) -> ! {
    if status != 0 {
        panic!("the first program reported failure {status}");
    }
    kprintln!("{}", crate::BOOT_OK_MARKER);
    crate::smp::idle()
}
