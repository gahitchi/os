// SPDX-License-Identifier: GPL-3.0-only
//! `cargo xtask`: builds the kernel, makes the bootable image and runs it in
//! QEMU. Kept free of dependencies so it builds anywhere Rust does.
//!
//! Host tools needed: git, make, a C compiler (to build Limine's small
//! installer), xorriso, and qemu-system-x86_64.

use std::env;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

/// Limine release used to boot Alaya. Bump deliberately, never float.
const LIMINE_TAG: &str = "v9.6.7-binary";
const LIMINE_REPO: &str = "https://github.com/limine-bootloader/limine.git";

const KERNEL_TARGET: &str = "x86_64-unknown-none";
const IMAGE_NAME: &str = "alaya-x86_64.iso";

/// Lines the kernel prints on serial; keep in sync with `kernel/src/main.rs`.
const BOOT_OK_MARKER: &str = "alaya: boot ok";
const PANIC_MARKER: &str = "alaya: panic";

const USAGE: &str = "\
usage: cargo xtask <command> [options]

commands:
  build             build the kernel
  image             build the kernel and the bootable ISO (build/alaya-x86_64.iso)
  run               build the image and boot it in QEMU, serial on this terminal
  test              boot in QEMU and pass if the kernel reports a clean boot

options:
  --debug           build the kernel without optimisations
  --cpus <n>        number of virtual cores (default 1)
  --uefi            boot with UEFI firmware (OVMF) instead of BIOS
  --timeout <secs>  give up after this long (test only, default 60)
  --image <path>    boot this image instead of building one (run and test)

environment:
  ALAYA_OVMF        path to the OVMF firmware file for --uefi
  ALAYA_QEMU_ARGS   extra arguments passed to QEMU";

struct Options {
    release: bool,
    cpus: u32,
    uefi: bool,
    timeout: u64,
    image: Option<PathBuf>,
}

fn main() -> ExitCode {
    let mut args = env::args().skip(1);
    let Some(command) = args.next() else {
        eprintln!("{USAGE}");
        return ExitCode::FAILURE;
    };
    let mut opts = Options {
        release: true,
        cpus: 1,
        uefi: false,
        timeout: 60,
        image: None,
    };
    while let Some(arg) = args.next() {
        let mut value = |name: &str| {
            args.next()
                .unwrap_or_else(|| fail(&format!("{name} needs a value")))
        };
        match arg.as_str() {
            "--debug" => opts.release = false,
            "--uefi" => opts.uefi = true,
            "--cpus" => {
                opts.cpus = value("--cpus")
                    .parse()
                    .unwrap_or_else(|_| fail("bad --cpus"))
            }
            "--timeout" => {
                opts.timeout = value("--timeout")
                    .parse()
                    .unwrap_or_else(|_| fail("bad --timeout"))
            }
            "--image" => opts.image = Some(PathBuf::from(value("--image"))),
            "-h" | "--help" => {
                println!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            other => fail(&format!("unknown option {other}\n\n{USAGE}")),
        }
    }

    let result = match command.as_str() {
        "build" => build_kernel(opts.release).map(|_| ()),
        "image" => build_image(opts.release).map(|p| println!("image: {}", p.display())),
        "run" => image_for(&opts).and_then(|image| run(&image, &opts)),
        "test" => image_for(&opts).and_then(|image| test(&image, &opts)),
        "-h" | "--help" | "help" => {
            println!("{USAGE}");
            Ok(())
        }
        other => Err(format!("unknown command {other}\n\n{USAGE}")),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("xtask: {e}");
            ExitCode::FAILURE
        }
    }
}

fn fail(msg: &str) -> ! {
    eprintln!("xtask: {msg}");
    std::process::exit(2)
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

fn sh(cmd: &mut Command) -> Result<(), String> {
    let status = cmd
        .status()
        .map_err(|e| format!("cannot start {cmd:?}: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{cmd:?} failed with {status}"))
    }
}

fn build_kernel(release: bool) -> Result<PathBuf, String> {
    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let mut cmd = Command::new(cargo);
    cmd.current_dir(root()).args([
        "build",
        "--package",
        "alaya-kernel",
        "--target",
        KERNEL_TARGET,
    ]);
    if release {
        cmd.arg("--release");
    }
    sh(&mut cmd)?;
    let profile = if release { "release" } else { "debug" };
    Ok(root()
        .join("target")
        .join(KERNEL_TARGET)
        .join(profile)
        .join("alaya-kernel"))
}

/// Fetch the pinned Limine binary release and build its host installer.
fn limine() -> Result<PathBuf, String> {
    let dir = root().join("build").join(format!("limine-{LIMINE_TAG}"));
    if !dir.join("limine-bios.sys").exists() {
        let _ = fs::remove_dir_all(&dir);
        sh(Command::new("git")
            .args([
                "-c",
                "advice.detachedHead=false",
                "clone",
                "--quiet",
                "--depth",
                "1",
                "--branch",
                LIMINE_TAG,
                LIMINE_REPO,
            ])
            .arg(&dir))?;
    }
    if !dir.join("limine").exists() {
        sh(Command::new("make").arg("--quiet").arg("-C").arg(&dir))?;
    }
    Ok(dir)
}

fn build_image(release: bool) -> Result<PathBuf, String> {
    let kernel = build_kernel(release)?;
    let limine = limine()?;
    let build = root().join("build");
    let iso_root = build.join("iso_root");
    let _ = fs::remove_dir_all(&iso_root);

    let copy = |from: &Path, to: &Path| {
        fs::create_dir_all(to.parent().unwrap()).map_err(|e| e.to_string())?;
        fs::copy(from, to)
            .map(|_| ())
            .map_err(|e| format!("copy {} -> {}: {e}", from.display(), to.display()))
    };
    copy(&kernel, &iso_root.join("boot/alaya-kernel"))?;
    copy(
        &root().join("boot/limine.conf"),
        &iso_root.join("boot/limine/limine.conf"),
    )?;
    for file in [
        "limine-bios.sys",
        "limine-bios-cd.bin",
        "limine-uefi-cd.bin",
    ] {
        copy(&limine.join(file), &iso_root.join("boot/limine").join(file))?;
    }
    copy(
        &limine.join("BOOTX64.EFI"),
        &iso_root.join("EFI/BOOT/BOOTX64.EFI"),
    )?;

    let image = build.join(IMAGE_NAME);
    sh(Command::new("xorriso")
        .args(["-as", "mkisofs", "-quiet", "-R", "-r", "-J"])
        .args([
            "-b",
            "boot/limine/limine-bios-cd.bin",
            "-no-emul-boot",
            "-boot-load-size",
            "4",
            "-boot-info-table",
        ])
        .args(["-hfsplus", "-apm-block-size", "2048"])
        .args([
            "--efi-boot",
            "boot/limine/limine-uefi-cd.bin",
            "-efi-boot-part",
            "--efi-boot-image",
        ])
        .arg("--protective-msdos-label")
        .arg(&iso_root)
        .arg("-o")
        .arg(&image))?;
    sh(Command::new(limine.join("limine"))
        .arg("bios-install")
        .arg("--quiet")
        .arg(&image))?;
    Ok(image)
}

fn image_for(opts: &Options) -> Result<PathBuf, String> {
    match &opts.image {
        Some(path) => Ok(path.clone()),
        None => build_image(opts.release),
    }
}

fn ovmf() -> Result<PathBuf, String> {
    if let Ok(path) = env::var("ALAYA_OVMF") {
        return Ok(PathBuf::from(path));
    }
    [
        "/usr/share/ovmf/OVMF.fd",
        "/usr/share/OVMF/OVMF.fd",
        "/usr/share/qemu/OVMF.fd",
        "/usr/share/edk2/x64/OVMF.4m.fd",
        "/usr/share/edk2-ovmf/x64/OVMF.fd",
    ]
    .iter()
    .map(PathBuf::from)
    .find(|p| p.exists())
    .ok_or_else(|| "no OVMF firmware found; install OVMF or set ALAYA_OVMF".into())
}

fn qemu(image: &Path, opts: &Options, serial: &str) -> Result<Command, String> {
    let mut cmd = Command::new("qemu-system-x86_64");
    cmd.args(["-machine", "q35", "-m", "256M", "-no-reboot"])
        .args(["-smp", &opts.cpus.to_string()])
        .args(["-display", "none", "-serial", serial, "-monitor", "none"])
        .arg("-cdrom")
        .arg(image);
    if opts.uefi {
        cmd.arg("-bios").arg(ovmf()?);
    }
    if let Ok(extra) = env::var("ALAYA_QEMU_ARGS") {
        cmd.args(extra.split_whitespace());
    }
    Ok(cmd)
}

fn run(image: &Path, opts: &Options) -> Result<(), String> {
    eprintln!(
        "xtask: booting {} on {} core(s); press Ctrl+A then x (lowercase) to quit",
        image.display(),
        opts.cpus
    );
    // `mon:stdio` lets Ctrl+A x quit QEMU from the terminal.
    sh(&mut qemu(image, opts, "mon:stdio")?)
}

/// Boot, echo the serial output, and decide from it whether the boot passed.
fn test(image: &Path, opts: &Options) -> Result<(), String> {
    let mode = if opts.uefi { "UEFI" } else { "BIOS" };
    eprintln!(
        "xtask: boot test, {mode}, {} core(s), timeout {}s",
        opts.cpus, opts.timeout
    );
    let mut child = qemu(image, opts, "stdio")?
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot start qemu-system-x86_64: {e}"))?;

    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });

    let deadline = Instant::now() + Duration::from_secs(opts.timeout);
    let outcome = loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(left) {
            Ok(line) => {
                let line = line.trim_end_matches('\r');
                println!("  | {line}");
                if line.contains(BOOT_OK_MARKER) {
                    break Ok(());
                }
                if line.contains(PANIC_MARKER) {
                    break Err("kernel panicked".to_string());
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                break Err(format!("no boot marker within {}s", opts.timeout));
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                break Err("QEMU exited before the kernel booted".into());
            }
        }
    };
    let _ = child.kill();
    let _ = child.wait();
    match &outcome {
        Ok(()) => eprintln!("xtask: PASS ({mode}, {} core(s))", opts.cpus),
        Err(e) => eprintln!("xtask: FAIL ({mode}, {} core(s)): {e}", opts.cpus),
    }
    outcome
}
