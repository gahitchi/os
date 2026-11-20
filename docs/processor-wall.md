# The processor wall (milestone 4b, step 1)

Alaya started on PCs (x86-64). Decision 93 makes room for ARM too, the kind
of processor in phones, the Raspberry Pi and many new laptops, without
slowing the PC path. The cheapest moment to do that is now, while the
kernel is about 8,000 lines.

Step 1 (this page) builds the **wall**: every piece of the kernel that only
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
| Device interrupts | MSI address and data for the APIC | `msi_message(core, irq)` |
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
