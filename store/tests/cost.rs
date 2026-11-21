// SPDX-License-Identifier: GPL-3.0-only
//! What recording reads costs on a file-heavy workload (milestone 5 goal:
//! at most 3 % extra time). Timing is only checked in release builds.

use alaya_store::disk::MemDisk;
use alaya_store::{Class, Options, Store};
use std::time::Instant;

const MACHINE: [u8; 32] = [7; 32];
const ALICE: [u8; 32] = [1; 32];

/// Five apps open, read and change documents, saving every few seconds'
/// worth of work: the kind of use the history is for.
fn workload(track: bool) -> std::time::Duration {
    let mut s = Store::format(MemDisk::new(16_384), &MACHINE, Options::default()).unwrap();
    s.add_person(1, &ALICE).unwrap();
    s.commit(0).unwrap();
    s.track_reads = track;
    let docs = s.root(Class::Documents);
    let mut files = Vec::new();
    let data: Vec<u8> = (0..16_384u32).map(|i| (i * 31 + 7) as u8).collect();
    for i in 0..200u32 {
        let f = s
            .create(1, 1, docs, format!("doc-{i}").as_bytes(), false)
            .unwrap();
        s.write(1, 1, f, 0, &data).unwrap();
        files.push(f);
    }
    s.commit(1).unwrap();
    let mut buf = vec![0u8; 16_384];
    let mut rng = 0x1234_5678_9abc_def0u64;
    let mut next = move || {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        rng
    };
    let t = Instant::now();
    for session in 0..20u64 {
        for app in 1..=5u64 {
            for _ in 0..40 {
                let f = files[(next() % files.len() as u64) as usize];
                s.read(1, app, f, 0, &mut buf).unwrap();
            }
            for _ in 0..4 {
                let f = files[(next() % files.len() as u64) as usize];
                s.write(1, app, f, next() % 12_000, &data[..2048]).unwrap();
            }
        }
        s.commit(2 + session).unwrap();
    }
    t.elapsed()
}

#[test]
fn recording_reads_costs_little() {
    let mut best = [std::time::Duration::MAX; 2];
    for round in 0..10 {
        let track = round % 2 == 1;
        best[track as usize] = best[track as usize].min(workload(track));
    }
    let extra = best[1].as_secs_f64() / best[0].as_secs_f64() - 1.0;
    println!(
        "recording reads: {:.1} % extra ({:?} without, {:?} with)",
        extra * 100.0,
        best[0],
        best[1]
    );
    if !cfg!(debug_assertions) {
        assert!(extra <= 0.03, "recording reads cost {:.1} %", extra * 100.0);
    }
}
