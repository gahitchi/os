// SPDX-License-Identifier: GPL-3.0-only
//! The processor wall (milestone 4b, decision 93).
//!
//! Everything that depends on the kind of processor lives behind this
//! wall, in one folder per processor family. The rest of the kernel may
//! only use what every side of the wall provides:
//!
//! - **Interrupts on this core:** `disable_interrupts`,
//!   `enable_interrupts`, `interrupts_enabled`, `wait_for_interrupt`,
//!   `halt_forever`, `spin_hint`.
//! - **Cores:** `set_cpu_base` and `cpu_index` (the per-core data pointer),
//!   `init_this_core` (returns the hardware id), `core_type`, `wake_core`
//!   (look at your run queue again), `stop_core` (stop now, on a panic),
//!   `run_on_stack`.
//! - **Boot:** `boot_before_memory`, `kernel_device_pages`,
//!   `boot_after_memory`, `kernel_only_device_pages`.
//! - **Time** (`timer`): the clock (`now`), its rate (`calibrate`), and a
//!   per-core deadline timer.
//! - **User mode** (`entry`): a thread's registers (`Regs`, read through
//!   named parts such as `call_nr` or `words`), saving and loading them,
//!   returning to user mode, thread-local storage; the `TrapFrame` entries
//!   build.
//! - **Memory maps** (`paging`): what a page-table entry looks like,
//!   loading and flushing maps, tags.
//! - **Vector and floating-point state** (`fpu`).
//! - **Devices:** the device memory guard (`iommu`), `msi_message`.
//! - **Safety:** `speculation_barrier`, `on_space_switch`, `protections`,
//!   `report_protections`.
//! - **Debug output** (`console`) and the deliberate-fault pieces of the
//!   self-tests (`selftest`).
//! - **Kernel-wide state** behind the wall (`state_items`).
//!
//! `tools/wall-check.sh` fails the build if processor-specific code shows
//! up outside this folder.

#[cfg(target_arch = "x86_64")]
mod x86_64;
#[cfg(target_arch = "x86_64")]
pub use self::x86_64::*;
