// SPDX-License-Identifier: GPL-3.0-only
//! Bad spots on the disk: every single block is damaged in turn (and made
//! unreadable in turn). The store must never return wrong data; with spare
//! copies of its map it must always open and find every file, and fix the
//! damaged copy.

use std::collections::BTreeMap;

use alaya_store::disk::MemDisk;
use alaya_store::{Class, Copies, Error, Options, Store};

const MACHINE: [u8; 32] = [7; 32];
const ALICE: [u8; 32] = [1; 32];

fn content(i: usize) -> Vec<u8> {
    (0..(i * 3000 + 100)).map(|j| (i * 31 + j) as u8).collect()
}

fn build(copies: Copies) -> (MemDisk, BTreeMap<(u8, String), Vec<u8>>) {
    let mut s = Store::format(MemDisk::new(1024), &MACHINE, Options { copies }).unwrap();
    s.add_person(1, &ALICE).unwrap();
    let mut want = BTreeMap::new();
    for i in 0..12 {
        let c = Class::ALL[i % 3];
        let name = format!("file{i}");
        let f = s.create(1, 1, s.root(c), name.as_bytes(), false).unwrap();
        s.write(1, 1, f, 0, &content(i)).unwrap();
        want.insert((c as u8, name), content(i));
        if i % 4 == 3 {
            s.commit(i as u64).unwrap();
        }
    }
    s.commit(100).unwrap();
    s.commit(101).unwrap();
    s.commit(102).unwrap();
    (s.into_disk(), want)
}

/// Files that read back, files that reported damage, blocks repaired.
type ReadBack = (usize, Vec<(u8, String)>, u64);

/// Open and read everything: files that read back, files that reported
/// damage. Panics on wrong data.
fn read_back(disk: MemDisk, want: &BTreeMap<(u8, String), Vec<u8>>) -> Result<ReadBack, Error> {
    let mut s = Store::open(disk, &MACHINE)?;
    s.unlock(1, &ALICE)?;
    let mut good = 0;
    let mut damaged = Vec::new();
    for ((c, name), data) in want {
        let class = Class::from(*c as u64).unwrap();
        let f = s.lookup(1, s.root(class), name.as_bytes())?;
        let mut buf = vec![0u8; data.len()];
        match s.read(1, 1, f, 0, &mut buf) {
            Ok(n) => {
                assert_eq!(n, data.len());
                assert!(buf == *data, "wrong data returned for {name}");
                good += 1;
            }
            Err(Error::Damaged(_)) => damaged.push((*c, name.clone())),
            Err(e) => return Err(e),
        }
    }
    // Read everything else too (history, old moments): fixes more.
    s.commit(200)?;
    let _ = s.check();
    Ok((good, damaged, s.dev.stats.repaired))
}

fn every_block(copies: Copies, unreadable: bool) -> (u64, u64) {
    let (disk, want) = build(copies);
    let used: Vec<u64> = disk.raw().map(|(a, _)| *a).collect();
    let mut repaired = 0;
    let mut tried = 0;
    for a in used {
        let mut d = disk.clone();
        if unreadable {
            d.bad.insert(a);
        } else {
            d.damage(a, (a as usize * 97) % 4096);
        }
        tried += 1;
        match read_back(d, &want) {
            Ok((_, damaged, rep)) => {
                repaired += rep;
                for (c, name) in &damaged {
                    match copies {
                        Copies::None => {}
                        // Only a file's own contents can be lost, never the map.
                        Copies::Map => {}
                        Copies::MapAndDocuments => assert_ne!(
                            *c,
                            Class::Documents as u8,
                            "document {name} lost despite its spare copy"
                        ),
                    }
                }
                assert!(
                    damaged.len() <= 1,
                    "one bad block hurt {} files",
                    damaged.len()
                );
            }
            Err(e) => {
                assert_eq!(
                    copies,
                    Copies::None,
                    "block {a}: the map should survive one bad block: {e:?}"
                );
                assert!(matches!(e, Error::Damaged(_) | Error::NotAStore), "{e:?}");
            }
        }
    }
    (tried, repaired)
}

#[test]
fn damaged_blocks_with_map_copies() {
    let (tried, repaired) = every_block(Copies::Map, false);
    assert!(tried > 50 && repaired > 5, "{tried} {repaired}");
}

#[test]
fn unreadable_blocks_with_map_copies() {
    every_block(Copies::Map, true);
}

#[test]
fn damaged_blocks_with_document_copies() {
    every_block(Copies::MapAndDocuments, false);
}

#[test]
fn damaged_blocks_without_copies_never_lie() {
    every_block(Copies::None, false);
}

#[test]
fn repaired_blocks_stay_repaired() {
    let (disk, want) = build(Copies::Map);
    let mut d = disk.clone();
    // Damage the first copy of everything in the first half of the disk.
    let first_half: Vec<u64> = d
        .raw()
        .map(|(a, _)| *a)
        .filter(|a| *a >= 2 && *a < 512)
        .collect();
    for a in &first_half {
        d.damage(*a, 100);
    }
    // Files in the first half had no spare: some are lost, but no lies.
    let mut s = Store::open(&mut d, &MACHINE).unwrap();
    s.unlock(1, &ALICE).unwrap();
    let _ = s.check();
    assert!(s.dev.stats.repaired > 0);
    drop(s);
    let (good, damaged, repaired) = read_back(d, &want).unwrap();
    assert_eq!(repaired, 0, "repairs did not stick");
    assert_eq!(good + damaged.len(), want.len());
}
