use std::path::PathBuf;
use std::process::Command;
use std::{env, fs};

/// The Linux test programs (milestone 8b): ordinary static Linux
/// programs built from `user/linux`, carried by the Linux world. A copy
/// goes to `build/linux-tests/<arch>/`, so the same files can be run on a
/// real Linux machine and their output compared (decision 166).
const LINUX_TESTS: [&str; 3] = ["hello", "sandbox", "crash"];

fn linux_tests(dir: &str) {
    let arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap();
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let linux = PathBuf::from(dir).join("..").join("linux");
    println!("cargo:rerun-if-changed={}", linux.display());
    let cc = env::var("CC").unwrap_or_else(|_| "clang".into());
    let copies = PathBuf::from(dir)
        .join("..")
        .join("..")
        .join("build")
        .join("linux-tests")
        .join(&arch);
    let _ = fs::create_dir_all(&copies);
    for name in LINUX_TESTS {
        let elf = out.join(format!("linux-{name}"));
        let status = Command::new(&cc)
            .arg(format!("--target={arch}-linux-gnu"))
            .args([
                "-ffreestanding",
                "-nostdlib",
                "-static",
                "-fno-pie",
                "-fno-stack-protector",
                "-fno-builtin",
                "-O2",
                "-Wall",
                "-Werror",
                "-fuse-ld=lld",
                "-Wl,--build-id=none",
                "-o",
            ])
            .arg(&elf)
            .arg(linux.join("rt").join("rt.c"))
            .arg(linux.join("tests").join(format!("{name}.c")))
            .status()
            .unwrap_or_else(|e| panic!("{cc} (clang and lld build the Linux tests): {e}"));
        assert!(status.success(), "{cc} failed on the Linux test {name}");
        let _ = fs::copy(&elf, copies.join(name));
    }
}

fn main() {
    let dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("none") {
        println!("cargo:rustc-link-arg=-T{dir}/link.ld");
        println!("cargo:rustc-link-arg=--no-dynamic-linker");
        println!("cargo:rustc-link-arg=-zmax-page-size=0x1000");
        linux_tests(&dir);
    }
    println!("cargo:rerun-if-changed=link.ld");
    println!("cargo:rerun-if-env-changed=CC");
}
