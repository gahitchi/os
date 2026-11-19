// SPDX-License-Identifier: GPL-3.0-only
//! Architecture-specific code. Only x86-64 exists today.

#[cfg(target_arch = "x86_64")]
mod x86_64;
#[cfg(target_arch = "x86_64")]
pub use self::x86_64::*;
