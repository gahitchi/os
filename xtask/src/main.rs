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

/// The processor family to build for and emulate.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Arch {
    X86_64,
    Aarch64,
}

impl Arch {
    fn name(self) -> &'static str {
        match self {
            Arch::X86_64 => "x86_64",
            Arch::Aarch64 => "aarch64",
        }
    }
    /// The kernel is built without floating point (it saves programs'
    /// registers itself).
    fn kernel_target(self) -> &'static str {
        match self {
            Arch::X86_64 => "x86_64-unknown-none",
            Arch::Aarch64 => "aarch64-unknown-none-softfloat",
        }
    }
    fn user_target(self) -> &'static str {
        match self {
            Arch::X86_64 => "x86_64-unknown-none",
            Arch::Aarch64 => "aarch64-unknown-none",
        }
    }
    fn qemu(self) -> &'static str {
        match self {
            Arch::X86_64 => "qemu-system-x86_64",
            Arch::Aarch64 => "qemu-system-aarch64",
        }
    }
}

/// Lines the kernel prints on serial; keep in sync with the kernel
/// (`main.rs`, `selftest.rs` and each `arch/*/trap.rs`).
const BOOT_OK_MARKER: &str = "alaya: boot ok";
const PANIC_MARKER: &str = "alaya: panic";
/// The kernel fuzzer prints this when its run ended without a crash.
const FUZZ_DONE_MARKER: &str = "fuzz: done";
const FAULT_MARKER: &str = "alaya: fault: ";
/// One line per core once it passed its self-tests: `core <n>: ready, ...`.
const CORE_READY: (&str, &str) = ("core ", ": ready");

/// Deliberate crashes the kernel can be asked for (`test-fault=` on its
/// command line), and what the fault report must say for each, on each
/// processor family.
const FAULTS: [(&str, [&[&str]; 2]); 6] = [
    (
        "page",
        [
            &["page fault (#PF", "writing to 0x00000000dead0000"],
            &[
                "data abort",
                "writing to 0x00000000dead0000: page not present",
            ],
        ],
    ),
    (
        "stack",
        [
            &["double fault (#DF", "stack overflow"],
            &["data abort", "stack overflow"],
        ],
    ),
    (
        "invalid-opcode",
        [&["invalid opcode (#UD"], &["undefined instruction"]],
    ),
    (
        "other-core-page",
        [
            &["page fault (#PF", "on cpu LAST"],
            &["data abort", "on cpu LAST"],
        ],
    ),
    (
        "smap",
        [
            &[
                "page fault (#PF",
                "reading from 0x0000000000010000: protection violation, kernel mode",
            ],
            &[
                "data abort",
                "reading from 0x0000000000010000: protection violation, kernel mode",
            ],
        ],
    ),
    (
        "smep",
        [
            &[
                "page fault (#PF",
                "executing at 0x0000000000010000: protection violation, kernel mode",
            ],
            &[
                "instruction abort",
                "executing at 0x0000000000010000: protection violation, kernel mode",
            ],
        ],
    ),
];

const USAGE: &str = "\
usage: cargo xtask <command> [options]

commands:
  build             build the kernel and the first program
  image             build the kernel and the bootable ISO (build/alaya-<arch>.iso)
  run               build the image and boot it in QEMU, serial on this terminal
  test              boot in QEMU and pass if the kernel reports a clean boot

options:
  --arch <a>        x86_64 (default) or aarch64 (ARM64, always UEFI)
  --debug           build the kernel without optimisations
  --cpus <n>        number of virtual cores (default 1)
  --uefi            boot with UEFI firmware (OVMF) instead of BIOS
  --accel <a>       kvm, tcg or auto (default auto: KVM when /dev/kvm is usable)
  --memory <size>   guest memory, QEMU syntax (default 256M, 512M on ARM)
  --fault <kind>    test only: crash on purpose and check the fault report;
                    kinds: page, stack, invalid-opcode, other-core-page,
                    smap (kernel reads a user page), smep (runs one)
  --fuzz <secs>     test only: after the boot checks, run the kernel fuzzer
                    for this long (random calls from several cores)
  --long            test only: the long stress runs (10 minutes of random
                    driver kills instead of 8 seconds)
  --plug <n>        test only: pull the plug n times. Each boot opens the
                    store the last boot left, checks what survived, keeps
                    saving, and QEMU is killed at a random moment
                    (with image: build the pull-the-plug image instead)
  --timeout <secs>  give up after this long (test only, default 180, or
                    the fuzz time plus 180, or 15 minutes with --long)
  --image <path>    boot this image instead of building one (run and test)

environment:
  ALAYA_OVMF        path to the OVMF firmware file for --uefi
  ALAYA_QEMU_ARGS   extra arguments passed to QEMU";

struct Options {
    arch: Arch,
    release: bool,
    cpus: u32,
    uefi: bool,
    timeout: u64,
    image: Option<PathBuf>,
    accel: String,
    memory: String,
    fault: Option<String>,
    fuzz: Option<u64>,
    long: bool,
    plug: Option<u32>,
}

fn main() -> ExitCode {
    let mut args = env::args().skip(1);
    let Some(command) = args.next() else {
        eprintln!("{USAGE}");
        return ExitCode::FAILURE;
    };
    let mut opts = Options {
        arch: Arch::X86_64,
        release: true,
        cpus: 1,
        uefi: false,
        timeout: 180,
        image: None,
        accel: "auto".into(),
        memory: "256M".into(),
        fault: None,
        fuzz: None,
        long: false,
        plug: None,
    };
    let mut timeout_set = false;
    let mut memory_set = false;
    while let Some(arg) = args.next() {
        let mut value = |name: &str| {
            args.next()
                .unwrap_or_else(|| fail(&format!("{name} needs a value")))
        };
        match arg.as_str() {
            "--debug" => opts.release = false,
            "--arch" => {
                opts.arch = match value("--arch").as_str() {
                    "x86_64" | "x86-64" | "pc" => Arch::X86_64,
                    "aarch64" | "arm64" | "arm" => Arch::Aarch64,
                    other => fail(&format!("unknown --arch {other} (use x86_64 or aarch64)")),
                }
            }
            "--uefi" => opts.uefi = true,
            "--cpus" => {
                opts.cpus = value("--cpus")
                    .parse()
                    .unwrap_or_else(|_| fail("bad --cpus"))
            }
            "--timeout" => {
                timeout_set = true;
                opts.timeout = value("--timeout")
                    .parse()
                    .unwrap_or_else(|_| fail("bad --timeout"))
            }
            "--fuzz" => {
                opts.fuzz = Some(
                    value("--fuzz")
                        .parse()
                        .unwrap_or_else(|_| fail("bad --fuzz")),
                )
            }
            "--long" => opts.long = true,
            "--plug" => {
                opts.plug = Some(
                    value("--plug")
                        .parse()
                        .unwrap_or_else(|_| fail("bad --plug")),
                )
            }
            "--image" => opts.image = Some(PathBuf::from(value("--image"))),
            "--accel" => opts.accel = value("--accel"),
            "--memory" => {
                memory_set = true;
                opts.memory = value("--memory")
            }
            "--fault" => {
                let kind = value("--fault");
                if !FAULTS.iter().any(|(k, _)| *k == kind) {
                    fail(&format!("unknown fault kind {kind}"));
                }
                opts.fault = Some(kind);
            }
            "-h" | "--help" => {
                println!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            other => fail(&format!("unknown option {other}\n\n{USAGE}")),
        }
    }

    if let (Some(secs), false) = (opts.fuzz, timeout_set) {
        opts.timeout = secs + 180;
    }
    if opts.arch == Arch::Aarch64 {
        // ARM machines start from UEFI firmware; there is no BIOS. The
        // firmware keeps more of the memory for itself than a PC's.
        opts.uefi = true;
        if !memory_set {
            opts.memory = "512M".into();
        }
    }
    if opts.long && !timeout_set {
        opts.timeout = 15 * 60;
    }

    let result = match command.as_str() {
        "build" => build_kernel(opts.arch, opts.release)
            .and_then(|_| build_init(opts.arch, opts.release))
            .map(|_| ()),
        // `image --plug 1` makes the pull-the-plug image the phone script
        // downloads (alaya-<arch>-plug.iso), so phones can run that test.
        "image" if opts.plug.is_some() => image_for(&opts).and_then(|test| {
            let plug = test.with_file_name(format!("alaya-{}-plug.iso", opts.arch.name()));
            fs::rename(&test, &plug).map_err(|e| format!("rename image: {e}"))?;
            println!("image: {}", plug.display());
            Ok(())
        }),
        "image" => {
            build_image(opts.arch, opts.release, None).map(|p| println!("image: {}", p.display()))
        }
        "run" => image_for(&opts).and_then(|image| run(&image, &opts)),
        "test" if opts.plug.is_some() => image_for(&opts).and_then(|image| plug(&image, &opts)),
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

fn build_kernel(arch: Arch, release: bool) -> Result<PathBuf, String> {
    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let mut cmd = Command::new(cargo);
    cmd.current_dir(root()).args([
        "build",
        "--package",
        "alaya-kernel",
        "--target",
        arch.kernel_target(),
    ]);
    if release {
        cmd.arg("--release");
    }
    sh(&mut cmd)?;
    let profile = if release { "release" } else { "debug" };
    Ok(root()
        .join("target")
        .join(arch.kernel_target())
        .join(profile)
        .join("alaya-kernel"))
}

/// Programs the boot image carries besides the first one, all from the
/// `alaya-programs` crate.
const PROGRAMS: [&str; 5] = ["keeper", "blk1", "blk2", "disktest", "store"];

/// Build the first program and the programs it starts. They live in the
/// lower half, so they are built without the kernel's code model
/// (`.cargo/config.toml`), in a target directory of their own so the two
/// builds do not invalidate each other. Returns the first program's path.
fn build_init(arch: Arch, release: bool) -> Result<PathBuf, String> {
    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let mut cmd = Command::new(cargo);
    cmd.current_dir(root())
        .env("RUSTFLAGS", USER_RUSTFLAGS)
        .args([
            "build",
            "--package",
            "alaya-init",
            "--package",
            "alaya-programs",
            "--bins",
            "--target",
            arch.user_target(),
        ])
        .arg("--target-dir")
        .arg(root().join("target").join("user"));
    if release {
        cmd.arg("--release");
    }
    sh(&mut cmd)?;
    Ok(user_binary(arch, release, "init"))
}

fn user_binary(arch: Arch, release: bool, name: &str) -> PathBuf {
    let profile = if release { "release" } else { "debug" };
    root()
        .join("target/user")
        .join(arch.user_target())
        .join(profile)
        .join(name)
}

/// Size of the virtual disk: the first MiB for the disk tests of
/// milestone 4, the rest for the store (milestone 5).
const DISK_BYTES: u64 = 64 << 20;

/// The virtual disk the disk driver works on, made once (and grown if an
/// older, smaller one is there).
fn disk_image() -> Result<PathBuf, String> {
    let path = root().join("build").join("disk.img");
    fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
    let len = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    if len < DISK_BYTES {
        let f = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .map_err(|e| format!("disk image: {e}"))?;
        f.set_len(DISK_BYTES)
            .map_err(|e| format!("disk image: {e}"))?;
    }
    Ok(path)
}

/// Code generation flags for programs (the kernel's are in
/// `.cargo/config.toml`).
const USER_RUSTFLAGS: &str = "-C relocation-model=static";

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

/// Build the bootable ISO. `cmdline` makes a test variant whose kernel
/// command line is set, kept apart from the normal image.
fn build_image(arch: Arch, release: bool, cmdline: Option<&str>) -> Result<PathBuf, String> {
    let kernel = build_kernel(arch, release)?;
    let init = build_init(arch, release)?;
    let limine = limine()?;
    let build = root().join("build");
    let iso_root = build.join(format!("iso_root-{}", arch.name()));
    let _ = fs::remove_dir_all(&iso_root);

    let copy = |from: &Path, to: &Path| {
        fs::create_dir_all(to.parent().unwrap()).map_err(|e| e.to_string())?;
        fs::copy(from, to)
            .map(|_| ())
            .map_err(|e| format!("copy {} -> {}: {e}", from.display(), to.display()))
    };
    copy(&kernel, &iso_root.join("boot/alaya-kernel"))?;
    copy(&init, &iso_root.join("boot/init"))?;
    let mut conf = fs::read_to_string(root().join("boot/limine.conf"))
        .map_err(|e| format!("read boot/limine.conf: {e}"))?;
    for name in PROGRAMS {
        copy(
            &user_binary(arch, release, name),
            &iso_root.join("boot").join(name),
        )?;
        conf.push_str(&format!("    module_path: boot():/boot/{name}\n"));
    }
    if let Some(cmdline) = cmdline {
        conf.push_str(&format!("    cmdline: {cmdline}\n"));
    }
    let conf_path = iso_root.join("boot/limine/limine.conf");
    fs::create_dir_all(conf_path.parent().unwrap()).map_err(|e| e.to_string())?;
    fs::write(&conf_path, conf).map_err(|e| format!("write limine.conf: {e}"))?;
    for file in [
        "limine-bios.sys",
        "limine-bios-cd.bin",
        "limine-uefi-cd.bin",
    ] {
        copy(&limine.join(file), &iso_root.join("boot/limine").join(file))?;
    }
    let efi = match arch {
        Arch::X86_64 => "BOOTX64.EFI",
        Arch::Aarch64 => "BOOTAA64.EFI",
    };
    copy(&limine.join(efi), &iso_root.join("EFI/BOOT").join(efi))?;

    let image = match cmdline {
        None => build.join(format!("alaya-{}.iso", arch.name())),
        Some(_) => build.join(format!("alaya-{}-test.iso", arch.name())),
    };
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
    if let Some(kind) = &opts.fault {
        if opts.image.is_some() {
            return Err("--fault builds its own image; leave out --image".into());
        }
        return build_image(opts.arch, opts.release, Some(&format!("test-fault={kind}")));
    }
    let mut words = Vec::new();
    if let Some(secs) = opts.fuzz {
        words.push(format!("fuzz={secs}"));
    }
    if opts.long {
        words.push("long".to_string());
    }
    if opts.plug.is_some() {
        words.push("store-soak".to_string());
    }
    if !words.is_empty() {
        if opts.image.is_some() {
            return Err(
                "--fuzz, --long and --plug build their own image; leave out --image".into(),
            );
        }
        return build_image(opts.arch, opts.release, Some(&words.join(" ")));
    }
    match &opts.image {
        Some(path) => Ok(path.clone()),
        None => build_image(opts.arch, opts.release, None),
    }
}

/// Whether to use KVM: asked for, or `auto` and /dev/kvm is usable.
fn use_kvm(opts: &Options) -> Result<bool, String> {
    let usable = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/kvm")
        .is_ok();
    match opts.accel.as_str() {
        "tcg" => Ok(false),
        "auto" => Ok(usable),
        "kvm" if usable => Ok(true),
        "kvm" => Err(
            "--accel kvm: /dev/kvm is missing or not accessible (are you in the kvm group?)".into(),
        ),
        other => Err(format!(
            "unknown accelerator {other} (use kvm, tcg or auto)"
        )),
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
    if opts.arch == Arch::Aarch64 {
        return qemu_arm(image, opts, serial);
    }
    let mut cmd = Command::new("qemu-system-x86_64");
    cmd.args(["-machine", "q35", "-m", &opts.memory, "-no-reboot"])
        .args(["-smp", &opts.cpus.to_string()])
        .args(["-display", "none", "-serial", serial, "-monitor", "none"])
        // The device memory guard (decision 77). Interrupt remapping is not
        // used yet.
        .args(["-device", "intel-iommu,aw-bits=48,intremap=off"])
        // The virtual disk, behind the IOMMU (its driver may reach only
        // the memory its keeper maps for it).
        .arg("-drive")
        .arg(format!(
            "file={},if=none,id=disk0,format=raw",
            disk_image()?.display()
        ))
        .args([
            "-device",
            "virtio-blk-pci,drive=disk0,disable-legacy=on,iommu_platform=on",
        ])
        .arg("-cdrom")
        .arg(image);
    if use_kvm(opts)? {
        // The host CPU as it is: TSC-deadline timer, x2APIC. QEMU hides the
        // invariant-TSC flag by default (it blocks live migration); show it.
        cmd.args(["-accel", "kvm", "-cpu", "host,+invtsc"]);
    } else {
        // The most complete emulated CPU: it has the safety switches
        // (SMEP, SMAP, UMIP), tagged memory maps (PCID) and XSAVE, so their
        // code runs under emulation too.
        cmd.args(["-accel", "tcg", "-cpu", "max"]);
    }
    if opts.uefi {
        cmd.arg("-bios").arg(ovmf()?);
    }
    if let Ok(extra) = env::var("ALAYA_QEMU_ARGS") {
        cmd.args(extra.split_whitespace());
    }
    Ok(cmd)
}

/// UEFI firmware for ARM machines (QEMU's EDK2 build).
fn aavmf() -> Result<PathBuf, String> {
    if let Ok(path) = env::var("ALAYA_AAVMF") {
        return Ok(PathBuf::from(path));
    }
    [
        "/usr/share/qemu-efi-aarch64/QEMU_EFI.fd",
        "/usr/share/edk2/aarch64/QEMU_EFI.fd",
        "/usr/share/AAVMF/AAVMF_CODE.fd",
        "/usr/share/qemu/edk2-aarch64-code.fd",
        "/opt/homebrew/share/qemu/edk2-aarch64-code.fd",
        "/data/data/com.termux/files/usr/share/qemu/edk2-aarch64-code.fd",
    ]
    .iter()
    .map(PathBuf::from)
    .find(|p| p.exists())
    .ok_or_else(|| "no ARM UEFI firmware found; install qemu-efi-aarch64 or set ALAYA_AAVMF".into())
}

/// QEMU's generic ARM board: GICv3 interrupt controller, the SMMUv3 device
/// memory guard, and the same virtual disk as on a PC.
fn qemu_arm(image: &Path, opts: &Options, serial: &str) -> Result<Command, String> {
    let mut cmd = Command::new("qemu-system-aarch64");
    cmd.args(["-machine", "virt,gic-version=3,iommu=smmuv3"])
        .args(["-m", &opts.memory, "-no-reboot"])
        .args(["-smp", &opts.cpus.to_string()])
        .args(["-display", "none", "-serial", serial, "-monitor", "none"])
        .arg("-drive")
        .arg(format!(
            "file={},if=none,id=disk0,format=raw",
            disk_image()?.display()
        ))
        .args([
            "-device",
            "virtio-blk-pci,drive=disk0,disable-legacy=on,iommu_platform=on",
        ])
        .arg("-drive")
        .arg(format!(
            "file={},if=none,id=cd0,format=raw,readonly=on",
            image.display()
        ))
        .args(["-device", "virtio-scsi-pci,id=scsi0"])
        .args(["-device", "scsi-cd,drive=cd0,bus=scsi0.0,bootindex=0"]);
    let firmware = aavmf()?;
    // Firmware images built for flash must be padded to 64 MiB; the plain
    // QEMU_EFI.fd works with -bios.
    cmd.arg("-bios").arg(firmware);
    if use_kvm(opts)? {
        cmd.args(["-accel", "kvm", "-cpu", "host"]);
    } else {
        // The most complete emulated CPU: it has PAN (the ARM twin of
        // SMAP), 16-bit address-space tags and the newer atomics.
        cmd.args(["-accel", "tcg", "-cpu", "max,pauth-impdef=on"]);
    }
    if let Ok(extra) = env::var("ALAYA_QEMU_ARGS") {
        cmd.args(extra.split_whitespace());
    }
    Ok(cmd)
}

fn run(image: &Path, opts: &Options) -> Result<(), String> {
    eprintln!(
        "xtask: booting {} on {} core(s) with {}; press Ctrl+A then x (lowercase) to quit",
        image.display(),
        opts.cpus,
        if use_kvm(opts)? {
            "KVM"
        } else {
            "emulation (TCG)"
        }
    );
    // `mon:stdio` lets Ctrl+A x quit QEMU from the terminal.
    sh(&mut qemu(image, opts, "mon:stdio")?)
}

/// Boot, echo the serial output, and decide from it whether the boot passed.
///
/// A normal boot passes when every core reported ready and the kernel
/// printed the boot marker. A `--fault` boot passes when the kernel printed
/// a fault report with the expected details and then stopped.
fn test(image: &Path, opts: &Options) -> Result<(), String> {
    let mode = if opts.uefi { "UEFI" } else { "BIOS" };
    let accel = if use_kvm(opts)? { "KVM" } else { "TCG" };
    let what = match &opts.fault {
        Some(kind) => format!("fault test '{kind}'"),
        None => match opts.fuzz {
            Some(secs) => format!("boot test with {secs}s of fuzzing"),
            None => "boot test".into(),
        },
    };
    eprintln!(
        "xtask: {what}, {mode}, {accel}, {} core(s), timeout {}s",
        opts.cpus, opts.timeout
    );
    let mut child = qemu(image, opts, "stdio")?
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot start {}: {e}", opts.arch.qemu()))?;

    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });

    let expected: Vec<String> = match &opts.fault {
        Some(kind) => FAULTS.iter().find(|(k, _)| k == kind).unwrap().1[opts.arch as usize]
            .iter()
            .map(|s| s.replace("LAST", &(opts.cpus.max(2) - 1).to_string()))
            .collect(),
        None => Vec::new(),
    };
    let mut ready = vec![false; opts.cpus as usize];
    let mut fault_report = String::new();
    let mut fuzz_done = false;

    let deadline = Instant::now() + Duration::from_secs(opts.timeout);
    let outcome = loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(left) {
            Ok(line) => {
                let line = line.trim_end_matches('\r');
                println!("  | {line}");
                if let Some(n) = line
                    .strip_prefix(CORE_READY.0)
                    .and_then(|rest| rest.split_once(CORE_READY.1))
                    .and_then(|(n, _)| n.parse::<usize>().ok())
                {
                    match ready.get_mut(n) {
                        Some(r) => *r = true,
                        None => {
                            break Err(format!(
                                "core {n} reported, but only {} were started",
                                opts.cpus
                            ));
                        }
                    }
                }
                if line.contains(FAULT_MARKER) || !fault_report.is_empty() {
                    fault_report.push_str(line);
                    fault_report.push('\n');
                }
                if line.contains(FUZZ_DONE_MARKER) {
                    fuzz_done = true;
                }
                if line.contains(BOOT_OK_MARKER) {
                    if opts.fault.is_some() {
                        break Err("the kernel booted instead of faulting".into());
                    }
                    if opts.fuzz.is_some() && !fuzz_done {
                        break Err("the fuzzer never finished".into());
                    }
                    let missing: Vec<String> = ready
                        .iter()
                        .enumerate()
                        .filter(|(_, r)| !**r)
                        .map(|(i, _)| i.to_string())
                        .collect();
                    if missing.is_empty() {
                        break Ok(());
                    }
                    break Err(format!(
                        "these cores never reported ready: {}",
                        missing.join(", ")
                    ));
                }
                if line.contains(PANIC_MARKER) {
                    if opts.fault.is_none() {
                        break Err("kernel panicked".to_string());
                    }
                    let absent: Vec<&String> = expected
                        .iter()
                        .filter(|e| !fault_report.contains(e.as_str()))
                        .collect();
                    if fault_report.is_empty() {
                        break Err("the kernel panicked without a fault report".into());
                    }
                    if absent.is_empty() {
                        break Ok(());
                    }
                    break Err(format!("the fault report does not mention {absent:?}"));
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
    let label = format!("{mode}, {accel}, {} core(s)", opts.cpus);
    match &outcome {
        Ok(()) => eprintln!("xtask: PASS ({label})"),
        Err(e) => eprintln!("xtask: FAIL ({label}): {e}"),
    }
    outcome
}

/// Pull the plug `--plug` times (milestone 5). Each boot of a `store-soak`
/// image opens the store left on the virtual disk, prints what it found
/// ("soak: found C"), then keeps changing two files and saving, printing
/// "soak: saving C" before each save and "soak: promised C" once the save
/// is safe. The host kills QEMU at a random moment and boots again. Every
/// boot must find a counter at least the last one promised and at most the
/// last one being saved: nothing promised lost, nothing invented. (Killing
/// QEMU loses what the virtual machine had not handed to the disk; a host
/// power cut, which also loses the disk's own cache, is what the store's
/// host tests cut at every write.)
fn plug(image: &Path, opts: &Options) -> Result<(), String> {
    let boots = opts.plug.unwrap_or(1);
    let mut seed = env::var("ALAYA_PLUG_SEED")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos() as u64)
                .unwrap_or(1)
                | 1
        });
    eprintln!("xtask: pulling the plug {boots} times (ALAYA_PLUG_SEED={seed})");
    let mut rand = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let (mut promised, mut saving) = (None::<u64>, None::<u64>);
    for boot in 0..boots {
        // Kill after a random number of saves, at a random moment after
        // that save started: inside the save or between saves.
        let saves = 1 + rand() % 12;
        let extra = Duration::from_millis(rand() % 400);
        let mut child = qemu(image, opts, "stdio")?
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .spawn()
            .map_err(|e| format!("cannot start {}: {e}", opts.arch.qemu()))?;
        let stdout = child.stdout.take().unwrap();
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            // Only whole lines: QEMU killed in the middle of printing
            // "soak: saving 400" leaves "soak: saving 40", which must not
            // count as a number that was saved.
            let mut reader = BufReader::new(stdout);
            let mut line = String::new();
            while matches!(reader.read_line(&mut line), Ok(n) if n > 0) {
                if !line.ends_with('\n') {
                    break;
                }
                line.pop();
                if tx.send(std::mem::take(&mut line)).is_err() {
                    break;
                }
            }
        });
        let deadline = Instant::now() + Duration::from_secs(opts.timeout);
        let mut started = 0u64;
        let mut kill_at: Option<Instant> = None;
        let outcome: Result<(), String> = loop {
            if let Some(t) = kill_at
                && Instant::now() >= t
            {
                break Ok(());
            }
            let wait = match kill_at {
                Some(t) => t.saturating_duration_since(Instant::now()),
                None => deadline.saturating_duration_since(Instant::now()),
            };
            let line = match rx.recv_timeout(wait) {
                Ok(line) => line,
                Err(mpsc::RecvTimeoutError::Timeout) if kill_at.is_some() => continue,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    break Err(format!("boot {boot}: no progress within {}s", opts.timeout));
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    break Err(format!("boot {boot}: QEMU exited"));
                }
            };
            let line = line.trim_end_matches('\r');
            let number = |prefix: &str| {
                line.find(prefix)
                    .and_then(|i| line[i + prefix.len()..].split_whitespace().next())
                    .and_then(|n| n.parse::<u64>().ok())
            };
            if line.contains(PANIC_MARKER) || line.contains("FAILED") {
                println!("  | {line}");
                break Err(format!("boot {boot}: {line}"));
            }
            if let Some(c) = number("soak: found ") {
                println!("  | {line}");
                if let Some(p) = promised
                    && c < p
                {
                    break Err(format!("boot {boot}: found {c}, but {p} was promised"));
                }
                if let Some(s) = saving
                    && c > s
                {
                    break Err(format!(
                        "boot {boot}: found {c}, but only {s} was ever saved"
                    ));
                }
                eprintln!(
                    "xtask: boot {boot}: found {c} (promised {}, saving {}): good",
                    promised.map_or("-".into(), |p| p.to_string()),
                    saving.map_or("-".into(), |s| s.to_string()),
                );
                promised = Some(c);
                saving = Some(c);
            } else if let Some(c) = number("soak: saving ") {
                saving = Some(c);
                started += 1;
                if started == saves {
                    kill_at = Some(Instant::now() + extra);
                }
            } else if let Some(c) = number("soak: promised ") {
                promised = Some(c);
            }
        };
        let _ = child.kill();
        let _ = child.wait();
        // Drain what QEMU printed before it died.
        while let Ok(line) = rx.try_recv() {
            let after = |prefix: &str| {
                line.find(prefix)
                    .and_then(|i| line[i + prefix.len()..].trim().parse::<u64>().ok())
            };
            if let Some(c) = after("soak: promised ") {
                promised = Some(c);
            } else if let Some(c) = after("soak: saving ") {
                saving = Some(c);
            }
        }
        if let Err(e) = outcome {
            eprintln!("xtask: FAIL: {e}");
            return Err(e);
        }
        eprintln!(
            "xtask: boot {boot}: plug pulled (promised {}, saving {})",
            promised.map_or("-".into(), |p| p.to_string()),
            saving.map_or("-".into(), |s| s.to_string()),
        );
    }
    eprintln!("xtask: PASS: pulled the plug {boots} times; nothing promised was lost");
    Ok(())
}
