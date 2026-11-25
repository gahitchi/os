// SPDX-License-Identifier: GPL-3.0-only
//! Alaya's small C library and the C tools built into the terminal
//! (milestone 8, step 4): compiled with clang for the target and linked
//! into the `apps` program only.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::{env, fs};

fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    let mut v: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
    v.sort();
    for p in v {
        if matches!(p.extension().and_then(|e| e.to_str()), Some("c" | "S")) {
            out.push(p);
        }
    }
}

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let libc = manifest.join("..").join("libc");
    println!("cargo:rerun-if-changed={}", libc.display());
    println!("cargo:rerun-if-env-changed=CC");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("none") {
        return;
    }
    let arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap();
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let cc = env::var("CC").unwrap_or_else(|_| "clang".into());
    let mut flags: Vec<&str> = vec![
        "-ffreestanding",
        "-nostdlibinc",
        "-fno-pic",
        "-fno-pie",
        "-fno-stack-protector",
        "-fno-builtin",
        "-O2",
        "-g",
        "-std=c11",
        "-Wall",
        "-Werror",
    ];
    match arch.as_str() {
        "x86_64" => flags.extend([
            "--target=x86_64-unknown-none-elf",
            "-mno-red-zone",
            "-mgeneral-regs-only",
            "-mcmodel=small",
        ]),
        "aarch64" => flags.extend(["--target=aarch64-unknown-none-elf", "-mgeneral-regs-only"]),
        a => panic!("no C build for {a}"),
    }
    let include = libc.join("include");
    let mut list = Vec::new();
    sources(&libc.join("src"), &mut list);
    sources(&libc.join("src").join(&arch), &mut list);
    sources(&libc.join("tools"), &mut list);
    for src in list {
        let name = src
            .strip_prefix(&libc)
            .unwrap()
            .to_string_lossy()
            .replace('/', "_");
        let obj = out.join(format!("{name}.o"));
        let status = Command::new(&cc)
            .args(&flags)
            .arg("-I")
            .arg(&include)
            .arg("-c")
            .arg(&src)
            .arg("-o")
            .arg(&obj)
            .status()
            .unwrap_or_else(|e| panic!("{cc} (clang is needed to build Alaya's C library): {e}"));
        assert!(status.success(), "{cc} failed on {}", src.display());
        println!("cargo:rustc-link-arg-bin=apps={}", obj.display());
    }
}
