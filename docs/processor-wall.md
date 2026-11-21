# The processor wall (milestone 4b)

Alaya started on PCs (x86-64). Decision 93 makes room for ARM too, the kind
of processor in phones, the Raspberry Pi and many new laptops, without
slowing the PC path. The cheapest moment to do that is now, while the
kernel is about 8,000 lines.

Step 1 builds the **wall**: every piece of the kernel that only
makes sense on a PC now lives in one folder, `kernel/src/arch/x86_64/`, and
the rest of the kernel talks to it through a short, fixed list of things
every processor must provide. Nothing changes on a PC: the same 43 boot
checks, fault reports, fuzzer and driver tests pass as before. Step 2 adds a
second folder, `kernel/src/arch/aarch64/`, and runs the same tests on ARM in
QEMU (decisions 97 to 99).

## What sits behind the wall

| Area | On a PC | What the rest of the kernel sees |
| --- | --- | --- |
| Clock and timer | TSC, TSC-deadline timer (APIC one-shot under plain emulation), PIT to measure | `time::now()` in clock ticks, `time::hz()`, a deadline per core |
| Cores | APIC ids, GS base, NMI to stop a core, IPIs | a hardware id per core, `wake_core`, `stop_core`, `cpu_index` |
| Entering the kernel | `syscall`/`sysret`, IDT, GDT, TSS | `arch::Regs` with named parts (call number, handle, info, data words) and a `TrapFrame` |
| Memory maps | x86 page-table bits, CR3, PCID | the kernel's own attribute bits; the 4-level walk itself is shared |
| Vector registers | XSAVE / FXSAVE | save and restore a thread's area |
| Device guard | Intel VT-d | `arch::iommu` with the same calls on every processor |
| Device interrupts | MSI address and data for the APIC | `msi_message(core, irq, device)`, `msi_release(irq)` |
| Safety | SMEP, SMAP, UMIP, speculation guards | `speculation_barrier`, `on_space_switch`, a protections report |
| Debug output | COM1 serial port | `arch::console` |

The full list is at the top of `kernel/src/arch/mod.rs`.

## How it stays a wall

`tools/wall-check.sh` runs in CI and fails the build if processor-specific
code (assembly, x86 register names, APIC or TSC use) appears in the kernel
outside `kernel/src/arch/`. The one exception is `limine.rs`, which mirrors
the bootloader's per-processor structures.

What programs see changed in one place only: the clock is now called
`alaya_sys::clock()` (it was `rdtsc()`) and the boot information carries
`clock_hz`, since on ARM the clock is not a TSC. The layout of the boot
information is unchanged.

## Step 2: ARM64

`kernel/src/arch/aarch64/` is the second side of the wall. The baseline is
ARMv8.0 (chips from about 2016 on, decision 98); newer features are used
only where the chip has them.

| Area | On ARM64 |
| --- | --- |
| Clock and timer | the generic timer (virtual count and timer, PPI 27) |
| Cores | MPIDR affinity as the hardware id; SGI 1 to wake a core, SGI 2 to stop it |
| Interrupt controller | GICv3: distributor, one redistributor per core, system-register CPU interface |
| Entering the kernel | one exception vector table; `svc #0` for system calls |
| Memory maps | kernel in the upper half (TTBR1), each program in the lower half (TTBR0) tagged with an ASID |
| Vector registers | NEON (32 registers of 128 bits) |
| Device guard | SMMUv3, stage-1 tables in the CPU's own format, a two-level stream table |
| Device interrupts | the GICv3 ITS: devices write an event to its doorbell, it raises an LPI |
| Safety | PXN (like SMEP), PAN when present (like SMAP), programs never see kernel registers (like UMIP), CSDB |
| Debug output | the PL011 serial port the firmware's SPCR table names |

System calls keep the same roles, in other registers:

| Role | PC | ARM64 |
| --- | --- | --- |
| call number in, status out | rax | x8 |
| handle in, badge out | rdi | x0 |
| info / label | rsi | x1 |
| six data words (first four are results) | rdx r10 r8 r9 r12 r13 | x2 to x7 |
| capability slots | r14 | x9 |
| reply handle | r15 | x10 |

The kernel is built for `aarch64-unknown-none-softfloat` (it never uses
vector registers itself), programs for `aarch64-unknown-none`. Programs keep
their own processor bits in one file too: `user/sys/src/arch.rs`.

Two things programs see changed: `IRQ_MSI` now takes the device's PCI
function, because ARM's interrupt translator tells devices apart by it; and
on ARM the device memory guard maps the interrupt doorbell into every I/O
space at `0xfee0_0000` (where a PC's devices send interrupts too), so
drivers cannot map memory there.

Testing is in QEMU only (decision 99): `cargo xtask test --arch aarch64`
boots QEMU's `virt` board with GICv3, the ITS and the SMMUv3; GitHub's ARM
machines run the same 43 checks, the fault reports and the fuzzer on every
change; and the Termux script boots the ARM image with `ALAYA_ARCH=aarch64`.
