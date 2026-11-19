// SPDX-License-Identifier: GPL-3.0-only
//! The kernel side of capabilities: physical memory for `alaya-cap`, the
//! budget that makes long operations interruptible, and the `call`
//! operations on capability tables and untyped memory.

use alaya_abi::{Error, Kind, flags, op, rights};
use alaya_cap::{self as cap, Budget, Cap, Destination, Globals, Memory, Progress, Slot, SlotRef};

use crate::arch::cpu::rdtsc;
use crate::sched::Sys;
use crate::{ipc, objects, percpu, time};

/// Physical memory through the kernel's direct map, plus the scheduling
/// state the delete hook needs.
pub struct Phys<'a> {
    hhdm: u64,
    pub sys: &'a mut Sys,
}

impl<'a> Phys<'a> {
    pub fn new(sys: &'a mut Sys) -> Self {
        Phys {
            hhdm: crate::limine::hhdm_offset().unwrap(),
            sys,
        }
    }
}

impl Memory for Phys<'_> {
    fn read(&self, at: SlotRef) -> Slot {
        // SAFETY: slots live in table objects or the boot table, which are
        // usable RAM covered by the direct map; the caller holds the big
        // kernel lock, so nobody writes them meanwhile.
        unsafe { core::ptr::read((at + self.hhdm) as *const Slot) }
    }

    fn write(&mut self, at: SlotRef, slot: &Slot) {
        // SAFETY: as for `read`.
        unsafe { core::ptr::write((at + self.hhdm) as *mut Slot, *slot) }
    }

    fn zero(&mut self, at: u64, len: u64) {
        // SAFETY: retype zeroes only memory of an untyped above its
        // watermark, which no object uses.
        unsafe { core::ptr::write_bytes((at + self.hhdm) as *mut u8, 0, len as usize) }
    }

    fn on_delete(&mut self, g: &mut Globals, at: SlotRef, cap: &Cap, last: bool) -> bool {
        objects::on_delete(self, g, at, cap, last)
    }
}

/// How long one kernel entry may run an interruptible operation before it
/// returns to user mode (and the call is restarted).
pub const SLICE_US: u64 = 50;

/// The kernel's budget for interruptible operations.
///
/// A revoke must out-pace every other core (decision 51, and
/// `formal/revoke/Revoke.tla`): in one turn of the big kernel lock, each
/// other core can add at most [`alaya_abi::RETYPE_BATCH`] capabilities, so
/// a revoke does at least that many steps for every core before it lets
/// itself be interrupted.
pub struct Slice {
    steps: u64,
    min_steps: u64,
    deadline: u64,
}

impl Slice {
    pub fn new() -> Self {
        Slice {
            steps: 0,
            min_steps: alaya_abi::RETYPE_BATCH * u64::from(percpu::count()) + 1,
            deadline: rdtsc() + time::ns_to_tsc(SLICE_US * 1000),
        }
    }
}

impl Budget for Slice {
    fn step(&mut self) -> bool {
        self.steps += 1;
        self.steps >= self.min_steps && rdtsc() >= self.deadline
    }
}

/// What a `call` returns: results, or a request to restart the call.
pub enum Outcome {
    Done([u64; 4]),
    /// A message call: the thread's registers were already set (or it
    /// waits, and they are set when it wakes).
    Message,
    Restart,
}

/// Run operation `label` on the capability behind `handle` in thread
/// `me`'s table.
pub fn invoke(
    g: &mut Globals,
    sys: &mut Sys,
    me: u64,
    handle: u64,
    label: u64,
    args: [u64; 6],
) -> Result<Outcome, Error> {
    let mut m = Phys::new(sys);
    let at = ipc::lookup(&m, me, handle)?;
    let c = m.read(at).cap;
    // A slot of another table, named by a handle to that table and an index.
    let resolve = |m: &Phys, table: u64, index: u64, need: u8| {
        let t = ipc::lookup(m, me, table)?;
        cap::slot_in(m, t, index, need)
    };
    let done = |r: [u64; 4]| Ok(Outcome::Done(r));
    match (c.kind(), label) {
        (Some(Kind::Table), op::INSPECT) => {
            let s = cap::slot_in(&m, at, args[0], rights::READ)?;
            let (c, children) = cap::inspect(&m, s);
            if !c.is_live() {
                return Err(Error::InvalidHandle);
            }
            let fl = c.flags & flags::REVOKING | if children { flags::HAS_CHILDREN } else { 0 };
            done([
                u64::from(c.kind)
                    | u64::from(c.rights) << 8
                    | u64::from(c.size_bits) << 16
                    | u64::from(fl) << 24,
                c.object,
                c.a,
                c.b,
            ])
        }
        (Some(Kind::Table), op::COPY | op::MINT) => {
            let dst = cap::slot_in(&m, at, args[0], rights::WRITE)?;
            let src = resolve(&m, args[1], args[2], rights::READ)?;
            let mask = u8::try_from(args[3]).map_err(|_| Error::InvalidArgument)?;
            let badge = (label == op::MINT).then_some(args[4]);
            cap::copy(&mut m, src, dst, mask, badge)?;
            done([0; 4])
        }
        (Some(Kind::Table), op::MOVE) => {
            let dst = cap::slot_in(&m, at, args[0], rights::WRITE)?;
            let src = resolve(&m, args[1], args[2], rights::WRITE)?;
            cap::move_cap(&mut m, src, dst)?;
            done([0; 4])
        }
        (Some(Kind::Table), op::DELETE) => {
            let s = cap::slot_in(&m, at, args[0], rights::WRITE)?;
            match cap::delete(&mut m, g, s)? {
                Progress::Done(()) => done([0; 4]),
                Progress::Interrupted => Ok(Outcome::Restart),
            }
        }
        (Some(Kind::Table), op::REVOKE) => {
            let s = cap::slot_in(&m, at, args[0], rights::WRITE)?;
            match cap::revoke(&mut m, g, s, &mut Slice::new())? {
                Progress::Done(()) => done([0; 4]),
                Progress::Interrupted => Ok(Outcome::Restart),
            }
        }
        (Some(Kind::Untyped), op::RETYPE) => {
            let kind = u8::try_from(args[0])
                .ok()
                .and_then(Kind::from_u8)
                .ok_or(Error::InvalidArgument)?;
            let bits = u8::try_from(args[1]).map_err(|_| Error::InvalidArgument)?;
            let t = ipc::lookup(&m, me, args[3])?;
            let tc = m.read(t).cap;
            if !tc.is(Kind::Table) {
                return Err(Error::WrongKind);
            }
            if tc.rights & rights::WRITE == 0 {
                return Err(Error::NoRights);
            }
            let dst = Destination {
                table: tc.object,
                bits: tc.size_bits,
                index: args[4],
            };
            match cap::retype(&mut m, at, kind, bits, args[2], dst, &mut Slice::new())? {
                Progress::Done(n) => done([n, 0, 0, 0]),
                Progress::Interrupted => Ok(Outcome::Restart),
            }
        }
        (Some(Kind::Table | Kind::Untyped), _) => Err(Error::NoSuchCall),
        (Some(Kind::Thread), _) => objects::thread(&mut m, g, me, c, label, args),
        (Some(Kind::SchedContext), _) => objects::sched_context(&mut m, me, c, label, args),
        (Some(Kind::SchedControl), _) => objects::sched_control(&mut m, me, label, args),
        (Some(Kind::AddressSpace), _) => objects::space(&mut m, me, c, label, args),
        (Some(Kind::Page), _) => objects::page(&mut m, me, at, c, label, args),
        _ => Err(Error::WrongKind),
    }
}
