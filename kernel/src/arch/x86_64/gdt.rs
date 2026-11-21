// SPDX-License-Identifier: GPL-3.0-only
//! Segment descriptors and the task state segment, one set per core.
//!
//! In 64-bit mode segments do almost nothing, but every core still needs a
//! GDT for its code and stack selectors and a TSS for the interrupt stacks
//! (IST) that faults such as a double fault switch to.

use core::arch::asm;
use core::mem::size_of;

pub const KERNEL_CODE: u16 = 0x08;
pub const KERNEL_DATA: u16 = 0x10;
/// User selectors, laid out for SYSRET: it loads SS from STAR[63:48] + 8
/// and CS from STAR[63:48] + 16, so user data comes right before user code.
pub const USER_DATA: u16 = 0x18 | 3;
pub const USER_CODE: u16 = 0x20 | 3;
const TSS_SELECTOR: u16 = 0x28;

/// Interrupt stack table slots (1-based, as the IDT encodes them).
pub const IST_DOUBLE_FAULT: u8 = 1;
pub const IST_NMI: u8 = 2;
pub const IST_MACHINE_CHECK: u8 = 3;
pub const IST_COUNT: usize = 3;

#[repr(C, packed(4))]
#[derive(Clone, Copy)]
pub struct Tss {
    _reserved0: u32,
    pub rsp: [u64; 3],
    _reserved1: u64,
    pub ist: [u64; 7],
    _reserved2: u64,
    _reserved3: u16,
    iomap_base: u16,
}

impl Tss {
    pub const fn new() -> Self {
        Self {
            _reserved0: 0,
            rsp: [0; 3],
            _reserved1: 0,
            ist: [0; 7],
            _reserved2: 0,
            _reserved3: 0,
            // No I/O permission bitmap: user code gets no port access.
            iomap_base: size_of::<Tss>() as u16,
        }
    }
}

#[repr(C, align(16))]
pub struct Gdt {
    entries: [u64; 7],
}

impl Gdt {
    pub const fn new() -> Self {
        Self { entries: [0; 7] }
    }
}

const fn segment(access: u64, flags: u64) -> u64 {
    // Base and limit are ignored in 64-bit mode; set the limit to the
    // maximum anyway so the descriptors look conventional.
    0xffff | (access << 40) | (0xf << 48) | (flags << 52)
}

/// Fill a core's GDT, load it, reload every segment register and load the
/// task register.
///
/// # Safety
/// `gdt` and `tss` must live for as long as the core runs, and must not be
/// shared with another core.
pub unsafe fn load(gdt: &'static mut Gdt, tss: &'static Tss) {
    let tss_addr = tss as *const Tss as u64;
    let tss_limit = (size_of::<Tss>() - 1) as u64;
    gdt.entries = [
        0,
        segment(0x9a, 0xa), // kernel code: present, ring 0, exec/read; long mode
        segment(0x92, 0xc), // kernel data: present, ring 0, read/write
        segment(0xf2, 0xc), // user data: ring 3
        segment(0xfa, 0xa), // user code: ring 3, long mode
        // 64-bit available TSS (type 9), two slots.
        (tss_limit & 0xffff)
            | ((tss_addr & 0xff_ffff) << 16)
            | (0x89 << 40)
            | (((tss_addr >> 24) & 0xff) << 56),
        tss_addr >> 32,
    ];

    #[repr(C, packed)]
    struct Pointer {
        limit: u16,
        base: u64,
    }
    let ptr = Pointer {
        limit: (size_of::<Gdt>() - 1) as u16,
        base: gdt.entries.as_ptr() as u64,
    };

    // SAFETY: the descriptors above are valid; the far return reloads CS.
    unsafe {
        asm!(
            "lgdt [{ptr}]",
            "push {code}",
            "lea {tmp}, [rip + 2f]",
            "push {tmp}",
            "retfq",
            "2:",
            "mov ds, {data:x}",
            "mov es, {data:x}",
            "mov ss, {data:x}",
            "xor {tmp:e}, {tmp:e}",
            "mov fs, {tmp:x}",
            "mov gs, {tmp:x}",
            "ltr {tss:x}",
            ptr = in(reg) &ptr,
            code = in(reg) u64::from(KERNEL_CODE),
            data = in(reg) u64::from(KERNEL_DATA),
            tss = in(reg) u64::from(TSS_SELECTOR),
            tmp = out(reg) _,
        );
    }
}

pub(crate) static mut GDTS: [Gdt; crate::percpu::MAX_CPUS] =
    [const { Gdt::new() }; crate::percpu::MAX_CPUS];
pub(crate) static mut TSSES: [Tss; crate::percpu::MAX_CPUS] =
    [const { Tss::new() }; crate::percpu::MAX_CPUS];

/// Set up and load this core's GDT and TSS, with its interrupt stacks.
/// `kernel_top` is where the CPU switches to when an interrupt or a system
/// call arrives from user mode.
pub fn load_this_core(index: u32, ist_tops: [u64; IST_COUNT], kernel_top: u64) {
    use core::ptr::addr_of_mut;
    let i = index as usize;
    crate::percpu::get(index)
        .kernel_stack_top
        .store(kernel_top, core::sync::atomic::Ordering::Relaxed);
    // SAFETY: each core touches only its own slot, once, while starting.
    unsafe {
        let tss = &mut (*addr_of_mut!(TSSES))[i];
        let mut rsp = tss.rsp;
        rsp[0] = kernel_top;
        tss.rsp = rsp;
        let mut ist = tss.ist;
        ist[..ist_tops.len()].copy_from_slice(&ist_tops);
        tss.ist = ist;
        load(&mut (*addr_of_mut!(GDTS))[i], tss);
    }
    // Loading the segment registers cleared GS base; point it back.
    crate::percpu::enter(index);
}
