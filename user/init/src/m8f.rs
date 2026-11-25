// SPDX-License-Identifier: GPL-3.0-only
//! Milestone 8b checks, step one: Linux programs in the Linux world
//! (decisions 163 to 166).

use core::sync::atomic::{AtomicU64, Ordering::SeqCst};

use alaya_programs::desktop::role;
use alaya_programs::linux as lx;
use alaya_sys::abi::rights;
use alaya_sys::{Caps, Kind, Msg, say};

use crate::m3::Kit;
use crate::m6::sc;
use crate::m6b::Session;

static WORLD_EP: AtomicU64 = AtomicU64::new(0);
static WORLD_SCS: [AtomicU64; lx::SCS as usize] = [const { AtomicU64::new(0) }; lx::SCS as usize];

/// The Linux world's endpoint and its programs' scheduling contexts, made
/// once. The first program keeps the endpoint (badge 0: Alaya asking).
pub(crate) fn world_handles(k: &mut Kit) -> (u64, [u64; lx::SCS as usize]) {
    if WORLD_EP.load(SeqCst) == 0 {
        let ep = k.make(Kind::Endpoint, 0);
        let mine = k.slot();
        alaya_sys::copy(k.at(mine), k.at(ep), rights::ALL).unwrap();
        WORLD_EP.store(mine, SeqCst);
        // Its programs share one core's worth of time at most each.
        for s in &WORLD_SCS {
            s.store(sc(k, 1000, 1000), SeqCst);
        }
        // The copy given to the world.
        WORLD_SCS_EP.store(ep, SeqCst);
    }
    let mut scs = [0; lx::SCS as usize];
    for (d, s) in scs.iter_mut().zip(&WORLD_SCS) {
        *d = s.load(SeqCst);
    }
    (WORLD_SCS_EP.load(SeqCst), scs)
}

static WORLD_SCS_EP: AtomicU64 = AtomicU64::new(0);

/// Ask the Linux world to run test program `which`; wait for it to end.
/// Returns (exit code, bytes printed, requests, unknown requests).
fn run_linux(which: u64) -> (u64, u64, u64, u64) {
    let m = alaya_sys::ipc_call(
        WORLD_EP.load(SeqCst),
        &Msg::new(lx::RUN, &[which]),
        Caps::default(),
    )
    .expect("the Linux world");
    assert!(
        m.label() == lx::DONE,
        "the Linux world answered {}",
        m.label()
    );
    let w = m.words;
    (w[0], w[1], w[3], w[4])
}

pub(crate) fn run(x: &mut Session<'_>, pass: &mut dyn FnMut(&str)) {
    x.start_app(role::LINUX);

    // ---- 127. A Linux program runs unchanged.
    let (code, printed, requests, unknown) = run_linux(lx::HELLO);
    say!("hello: exit {code}, {printed} bytes, {requests} requests, {unknown} unknown");
    assert!(code == 0, "hello ended with {code}");
    assert!(printed > 150, "hello printed only {printed} bytes");
    assert!(
        unknown == 0,
        "hello asked for {unknown} things the world does not know"
    );
    pass(
        "a Linux program runs unchanged in the Linux world: its memory, clock, thread pointer and output answered as Linux would",
    );

    // ---- 128. Only Linux answers reach a Linux program.
    let (code, _, requests, unknown) = run_linux(lx::SANDBOX);
    assert!(code == 3, "sandbox ended with {code}");
    // Three of its six unknown calls are numbers Linux could have; the
    // other three are beyond any Linux call and the kernel refuses them
    // without asking anyone.
    assert!(
        unknown == 3,
        "sandbox: {unknown} unknown requests reached the world"
    );
    assert!(requests >= 6, "sandbox: only {requests} requests");
    pass(
        "a Linux program reaches nothing of Alaya's: every call it makes is a Linux request, and unknown ones get Linux's own answer",
    );

    // ---- 129. A crash is told in plain words, and nothing else minds.
    let (code, printed, _, _) = run_linux(lx::CRASH);
    assert!(code == 128 + 11, "crash ended with {code}");
    assert!(printed > 0, "crash printed nothing before it crashed");
    let (code, _, _, _) = run_linux(lx::HELLO);
    assert!(code == 0, "the Linux world did not go on after a crash");
    pass(
        "a Linux program that crashes leaves a flight record in plain words, and the Linux world goes on",
    );
}
