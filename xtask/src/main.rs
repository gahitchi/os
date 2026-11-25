// SPDX-License-Identifier: GPL-3.0-only
//! `cargo xtask`: builds the kernel, makes the bootable image and runs it in
//! QEMU. Kept free of dependencies so it builds anywhere Rust does.
//!
//! Host tools needed: git, make, a C compiler (to build Limine's small
//! installer), xorriso, and qemu-system-x86_64.

mod bundles;

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
  --memory <size>   guest memory, QEMU syntax (default 1G)
  --display <kind>  QEMU display for run (gtk, sdl, vnc=:0, none); default:
                    QEMU's own window for run, none for tests
  --vnc             run: show the screen over VNC on port 5900 (phones)
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
  --timeout <secs>  give up after this long (test only, default 300, or
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
    /// QEMU's window: `None` lets `run` open one and keeps tests without.
    display: Option<String>,
    /// Start with an empty backup stick (the boot test does).
    fresh_stick: bool,
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
        // A whole boot test now ends with a desktop session. On a shared
        // machine that runs more virtual cores than it has, one run can
        // take three minutes (the big kernel lock waits on cores the host
        // has paused), so the limit leaves room for that.
        timeout: 300,
        image: None,
        accel: "auto".into(),
        memory: "1G".into(),
        fault: None,
        fuzz: None,
        long: false,
        plug: None,
        display: None,
        fresh_stick: false,
    };
    let mut timeout_set = false;
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
            "--display" => opts.display = Some(value("--display")),
            "--vnc" => opts.display = Some("vnc=:0".into()),
            "--image" => opts.image = Some(PathBuf::from(value("--image"))),
            "--accel" => opts.accel = value("--accel"),
            "--memory" => opts.memory = value("--memory"),
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
        // ARM machines start from UEFI firmware; there is no BIOS.
        opts.uefi = true;
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
        "run" => run_image(&opts).and_then(|image| run(&image, &opts)),
        "test" if opts.plug.is_some() => image_for(&opts).and_then(|image| plug(&image, &opts)),
        "test" => {
            opts.fresh_stick = true;
            image_for(&opts).and_then(|image| test(&image, &opts))
        }
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
const PROGRAMS: [&str; 10] = [
    "keeper", "blk1", "blk2", "disktest", "store", "gpu", "input", "desktop", "apps", "linux",
];

/// Font files shipped in the boot image: text, strong, mono, fallback.
const FONTS: [&str; 4] = [
    "Figtree-Regular.ttf",
    "Figtree-SemiBold.ttf",
    "IBMPlexMono-Regular.ttf",
    "NotoSans-Alaya.ttf",
];

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
            "--package",
            "alaya-apps",
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

/// The backup stick (milestone 7): a second virtual disk, shown to Alaya
/// the way a USB stick will be (real USB comes in milestone 11). The boot
/// test starts each run with an empty one.
const STICK_BYTES: u64 = 64 << 20;

fn stick_image(fresh: bool) -> Result<PathBuf, String> {
    let path = root().join("build").join("stick.img");
    fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
    if fresh {
        let _ = fs::remove_file(&path);
    }
    let len = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    if len < STICK_BYTES {
        let f = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .map_err(|e| format!("stick image: {e}"))?;
        f.set_len(STICK_BYTES)
            .map_err(|e| format!("stick image: {e}"))?;
    }
    Ok(path)
}

/// The stick's QEMU arguments: after every other device, so the disk
/// comes first on the bus.
fn stick_args(opts: &Options) -> Result<Vec<String>, String> {
    Ok(vec![
        "-drive".into(),
        format!(
            "file={},if=none,id=stick0,format=raw",
            stick_image(opts.fresh_stick)?.display()
        ),
        "-device".into(),
        "virtio-blk-pci,drive=stick0,disable-legacy=on,iommu_platform=on".into(),
    ])
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
/// The C interface's header lists exactly the functions `c.rs` has
/// (milestone 8, step 3).
fn check_c_interface() -> Result<(), String> {
    let dir = root().join("user").join("apps");
    let read = |p: PathBuf| fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()));
    let names = |text: &str, before: &str| -> Vec<String> {
        let mut v: Vec<String> = text
            .lines()
            .filter_map(|l| {
                let at = l.find("alaya_")?;
                let rest = &l[at..];
                let end = rest.find('(')?;
                (l[..at].trim_end().ends_with(before) || before.is_empty())
                    .then(|| rest[..end].to_string())
            })
            .filter(|n| n.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
            .collect();
        v.sort();
        v.dedup();
        v
    };
    let header = read(dir.join("include").join("alaya.h"))?;
    let rust = read(dir.join("src").join("c.rs"))?;
    let h = names(
        &header
            .lines()
            .filter(|l| !l.trim_start().starts_with('*') && !l.trim_start().starts_with("/*"))
            .collect::<Vec<_>>()
            .join("\n"),
        "",
    );
    let r = names(&rust, "fn");
    if h != r {
        return Err(format!(
            "alaya.h and c.rs differ: the header has {h:?}, c.rs has {r:?}"
        ));
    }
    Ok(())
}

fn build_image(arch: Arch, release: bool, cmdline: Option<&str>) -> Result<PathBuf, String> {
    check_c_interface()?;
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
    // The example apps (milestone 8), signed, in one module.
    let postcards = fs::read(user_binary(arch, release, "postcards"))
        .map_err(|e| format!("read postcards: {e}"))?;
    let postcards = bundles::loaded_only(&postcards)?;
    let examples = bundles::pack(&bundles::postcards(&postcards))?;
    fs::write(iso_root.join("boot/example-apps"), examples)
        .map_err(|e| format!("write example-apps: {e}"))?;
    conf.push_str("    module_path: boot():/boot/example-apps\n");
    // Alaya's letters (decision 105), in the order the desktop expects.
    for name in FONTS {
        copy(
            &root().join("fonts").join(name),
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
        Some("desk") => build.join(format!("alaya-{}-desk.iso", arch.name())),
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

/// `run` boots an image that leaves the desktop to the person at the
/// keyboard (the `desk` word), unless `--image` names another.
fn run_image(opts: &Options) -> Result<PathBuf, String> {
    if opts.image.is_some() || opts.fault.is_some() || opts.fuzz.is_some() || opts.long {
        return image_for(opts);
    }
    build_image(opts.arch, opts.release, Some("desk"))
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

/// `run` opens QEMU's own window (or what `--display` names); tests and
/// plug runs have none, and take screenshots through QMP instead.
fn display_args(opts: &Options, serial: &str) -> Vec<String> {
    match (&opts.display, serial == "mon:stdio") {
        (Some(d), _) => vec!["-display".into(), d.clone()],
        (None, true) => vec![],
        (None, false) => vec!["-display".into(), "none".into()],
    }
}

/// The screen card, keyboard and mouse (milestone 6), behind the IOMMU
/// like the disk. The mouse is a "tablet": it reports where the pointer
/// is, so it follows the host's pointer exactly.
const SEAT_DEVICES: [&str; 3] = [
    "virtio-gpu-pci,id=gpu0,disable-legacy=on,iommu_platform=on,xres=1280,yres=800,max_outputs=2",
    "virtio-keyboard-pci,id=kbd0,disable-legacy=on,iommu_platform=on",
    "virtio-tablet-pci,id=tablet0,disable-legacy=on,iommu_platform=on",
];

fn qemu(image: &Path, opts: &Options, serial: &str) -> Result<Command, String> {
    if opts.arch == Arch::Aarch64 {
        return qemu_arm(image, opts, serial);
    }
    let mut cmd = Command::new("qemu-system-x86_64");
    // No standard VGA card: the virtio screen is the only screen.
    cmd.args([
        "-machine",
        "q35",
        "-m",
        &opts.memory,
        "-no-reboot",
        "-vga",
        "none",
    ])
    .args(["-smp", &opts.cpus.to_string()])
    .args(display_args(opts, serial))
    .args(["-serial", serial, "-monitor", "none"])
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
    .args(SEAT_DEVICES.iter().flat_map(|d| ["-device", d]))
    .args(stick_args(opts)?)
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
        .args(display_args(opts, serial))
        .args(["-serial", serial, "-monitor", "none"])
        .arg("-drive")
        .arg(format!(
            "file={},if=none,id=disk0,format=raw",
            disk_image()?.display()
        ))
        .args([
            "-device",
            "virtio-blk-pci,drive=disk0,disable-legacy=on,iommu_platform=on",
        ])
        .args(SEAT_DEVICES.iter().flat_map(|d| ["-device", d]))
        .arg("-drive")
        .arg(format!(
            "file={},if=none,id=cd0,format=raw,readonly=on",
            image.display()
        ))
        .args(["-device", "virtio-scsi-pci,id=scsi0"])
        .args(["-device", "scsi-cd,drive=cd0,bus=scsi0.0,bootindex=0"])
        .args(stick_args(opts)?);
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
    eprintln!(
        "xtask: the checks run first (about a minute); then the desktop is yours: \
         set up a person and use the apps from the dock"
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
    let qmp_path = root().join(format!("build/qmp-{}.sock", std::process::id()));
    let _ = fs::remove_file(&qmp_path);
    let mut qmp: Option<Qmp> = None;
    let mut pictures = 0;
    let mut typed = false;
    let mut child = qemu(image, opts, "stdio")?
        .arg("-qmp")
        .arg(format!("unix:{},server=on,wait=off", qmp_path.display()))
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

    let mut deadline = Instant::now() + Duration::from_secs(opts.timeout);
    // ALAYA_NO_HOST_INPUT=1: type and click nothing, as on the phone; the
    // first program then sends the keys and mouse moves itself.
    let no_input = env::var("ALAYA_NO_HOST_INPUT").is_ok_and(|v| v == "1");
    let mut booted = false;
    // What each Linux test program printed in Alaya (decision 166).
    let mut linux_out: Vec<(String, String)> = Vec::new();
    let outcome = loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(left) {
            Ok(line) => {
                let line = line.trim_end_matches('\r');
                println!("  | {line}");
                if let Some((name, text)) = line
                    .strip_prefix(LINUX_OUT_MARKER)
                    .and_then(|rest| rest.split_once("> "))
                    .filter(|(name, _)| !name.contains(' '))
                {
                    linux_out.push((name.to_string(), format!("{text}\n")));
                }
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
                // Milestone 6: look at the screen, then type and click.
                if line.contains(PICTURE_SHOWN_MARKER) || line.contains(PICTURE_BACK_MARKER) {
                    let q = match qmp.take() {
                        Some(q) => Ok(q),
                        None => Qmp::connect(&qmp_path),
                    };
                    let step = q.and_then(|mut q| {
                        let shot = root().join(format!("build/screen-{}.ppm", pictures + 1));
                        check_test_picture(&mut q, &shot)?;
                        println!("  * screenshot {} matches the test picture", pictures + 1);
                        Ok(q)
                    });
                    match step {
                        Ok(q) => {
                            qmp = Some(q);
                            pictures += 1;
                        }
                        Err(e) => break Err(format!("screen check: {e}")),
                    }
                }
                if line.contains(INPUT_WAITING_MARKER) && !no_input {
                    let step = match qmp.as_mut() {
                        Some(q) => send_key_and_click(q),
                        None => Err("no control socket".into()),
                    };
                    if let Err(e) = step {
                        break Err(format!("input: {e}"));
                    }
                    println!("  * typed 'a' and clicked the middle of the screen");
                }
                if let Some(rest) = line.split(DRAG_MARKER).nth(1).filter(|_| !no_input) {
                    let n: Vec<i64> = rest
                        .split_whitespace()
                        .filter_map(|w| w.parse().ok())
                        .collect();
                    let step = match (qmp.as_mut(), n.as_slice()) {
                        (Some(q), [x0, y0, x1, y1]) => drag(q, (*x0, *y0), (*x1, *y1)),
                        _ => Err(format!("cannot drag: {rest}")),
                    };
                    if let Err(e) = step {
                        break Err(format!("drag: {e}"));
                    }
                    println!("  * dragged from {} {} to {} {}", n[0], n[1], n[2], n[3]);
                }
                if let Some(spec) = line.split(KEYS_MARKER).nth(1).filter(|_| !no_input) {
                    let step = match qmp.as_mut() {
                        Some(q) => type_keys(q, spec),
                        None => Err("no control socket".into()),
                    };
                    if let Err(e) = step {
                        break Err(format!("typing: {e}"));
                    }
                    println!("  * typed {spec}");
                }
                if let Some(name) = line.split(SCREENSHOT_MARKER).nth(1)
                    && let Some(q) = qmp.as_mut()
                {
                    // Pictures of the screen for people to look at.
                    let name: String = name
                        .chars()
                        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
                        .collect();
                    let shot = root().join(format!("build/screen-{name}.ppm"));
                    let _ = q.command(&format!(
                        r#"{{"execute":"screendump","arguments":{{"filename":"{}"}}}}"#,
                        shot.display()
                    ));
                    println!("  * saved a screenshot in {}", shot.display());
                }
                if line.contains(DESKTOP_DRAWN_MARKER) {
                    // A picture of the desktop for people to look at.
                    if let Some(q) = qmp.as_mut() {
                        let shot = root().join("build/screen-desktop.ppm");
                        let _ = q.command(&format!(
                            r#"{{"execute":"screendump","arguments":{{"filename":"{}"}}}}"#,
                            shot.display()
                        ));
                        println!(
                            "  * saved a screenshot of the desktop in {}",
                            shot.display()
                        );
                    }
                }
                if line.contains(TYPED_MARKER) {
                    typed = true;
                }
                // The timeline's speed gate (milestone 7): under 100 ms a
                // moment, measured on real speed (KVM). Plain emulation
                // runs many times slower, so there the number is shown.
                if let Some(rest) = line.split(TIMELINE_MARKER).nth(1)
                    && let Some(us) = rest
                        .split(" at most ")
                        .nth(1)
                        .and_then(|r| r.split(' ').next())
                        .and_then(|n| n.parse::<u64>().ok())
                {
                    // Real speed means KVM with a host core for every
                    // core Alaya has: with more, the host pauses Alaya's
                    // cores (one may hold the kernel lock), which measures
                    // the host, not Alaya. Emulated, a moment must still
                    // open within two seconds.
                    let own_cores = opts.cpus <= host_cpus();
                    let limit = if accel == "KVM" { 100_000 } else { 2_000_000 };
                    if own_cores && us >= limit {
                        break Err(format!(
                            "the timeline's gate: a moment took {us} us (must be under {} ms)",
                            limit / 1000
                        ));
                    }
                    println!(
                        "  * timeline gate: {} ms a moment at most ({})",
                        us / 1000,
                        if !own_cores {
                            "more cores than the host has: not checked, the gate needs one host core each"
                        } else if accel == "KVM" {
                            "under 100 ms: passed"
                        } else {
                            "emulated: under 2 s, passed; the 100 ms gate is checked with KVM"
                        }
                    );
                }
                if line.contains(DESKTOP_SKIPPED_MARKER) {
                    break Err("the drag never reached the desktop".into());
                }
                if line.contains(INPUT_SKIPPED_MARKER) && !no_input {
                    break Err("the key and click never reached Alaya".into());
                }
                if booted && line.contains(SERVING_MARKER) {
                    break Ok(());
                }
                if line.contains(BOOT_OK_MARKER) {
                    if opts.fault.is_some() {
                        break Err("the kernel booted instead of faulting".into());
                    }
                    if pictures < 2 {
                        break Err("the screen was never checked (no test picture)".into());
                    }
                    if !typed && !no_input {
                        break Err("the editor never showed the typed text".into());
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
                        // Then the first program must go on looking after
                        // the drivers and the apps.
                        booted = true;
                        deadline = deadline.min(Instant::now() + Duration::from_secs(30));
                        continue;
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
            Err(mpsc::RecvTimeoutError::Timeout) if booted => {
                break Err("the first program stopped after its checks".into());
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                break Err(format!("no boot marker within {}s", opts.timeout));
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                break Err("QEMU exited before the kernel booted".into());
            }
        }
    };
    drop(qmp);
    let _ = child.kill();
    let _ = child.wait();
    let _ = fs::remove_file(&qmp_path);
    let label = format!("{mode}, {accel}, {} core(s)", opts.cpus);
    let outcome = outcome.and_then(|()| two_worlds(opts.arch, &linux_out));
    match &outcome {
        Ok(()) => eprintln!("xtask: PASS ({label})"),
        Err(e) => eprintln!("xtask: FAIL ({label}): {e}"),
    }
    outcome
}

/// A line a Linux test program printed in Alaya's Linux world:
/// `linux: <program>> <text>`.
const LINUX_OUT_MARKER: &str = "linux: ";

/// The two-world check (decision 166): the Linux test programs Alaya
/// built are run on this machine too, when it is a Linux machine with the
/// same processor, with the same arguments and environment; what they
/// print must be exactly what they printed in Alaya's Linux world (every
/// run there, as some run more than once).
fn two_worlds(arch: Arch, seen: &[(String, String)]) -> Result<(), String> {
    if seen.is_empty() {
        return Ok(());
    }
    if !cfg!(target_os = "linux") || env::consts::ARCH != arch.name() {
        eprintln!(
            "xtask: two-world check skipped (this machine cannot run {} Linux programs)",
            arch.name()
        );
        return Ok(());
    }
    let dir = root().join("build").join("linux-tests").join(arch.name());
    let mut names: Vec<&str> = seen.iter().map(|(n, _)| n.as_str()).collect();
    names.sort();
    names.dedup();
    for name in names {
        let alaya: String = seen
            .iter()
            .filter(|(n, _)| n == name)
            .map(|(_, t)| t.as_str())
            .collect();
        let out = Command::new(dir.join(name))
            .args(["one", "two"])
            .env_clear()
            .envs([("LANG", "C.UTF-8"), ("HOME", "/home"), ("PATH", "/bin")])
            .output()
            .map_err(|e| format!("two-world check: run {name} here: {e}"))?;
        let linux = String::from_utf8_lossy(&out.stdout).into_owned();
        // A program may run more than once in Alaya: each run must match.
        if linux.is_empty()
            || !alaya.len().is_multiple_of(linux.len())
            || alaya != linux.repeat(alaya.len() / linux.len())
        {
            return Err(format!(
                "two-world check: {name} printed differently.\n--- on Linux:\n{linux}--- in Alaya:\n{alaya}"
            ));
        }
        eprintln!(
            "xtask: two-world check: {name} printed the same on Linux and in Alaya ({} bytes)",
            linux.len()
        );
    }
    Ok(())
}

/// Lines the first program prints when the test picture is on the screen
/// (`user/init/src/m6.rs`).
const PICTURE_SHOWN_MARKER: &str = "init: screen: test picture shown";
const PICTURE_BACK_MARKER: &str = "init: screen: back after a crash";
const INPUT_WAITING_MARKER: &str = "init: input: waiting for a key and a click";
const INPUT_SKIPPED_MARKER: &str = "init: input: nobody pressed a key";
const DRAG_MARKER: &str = "init: desktop: drag from";
const DESKTOP_SKIPPED_MARKER: &str = "init: desktop: nobody dragged";
const DESKTOP_DRAWN_MARKER: &str = "init: desktop: drawn after a screen driver crash";
const KEYS_MARKER: &str = "init: keys: ";
/// How many cores the host has (for the timeline's gate).
fn host_cpus() -> u32 {
    thread::available_parallelism().map_or(1, |n| n.get() as u32)
}

const TIMELINE_MARKER: &str = "init: timeline: each of";
const SCREENSHOT_MARKER: &str = "init: screenshot ";
/// After its checks the first program looks after drivers and apps.
const SERVING_MARKER: &str = "init: looking after the drivers and the apps";
const TYPED_MARKER: &str = "editor: text ends with: Welcome to Alaya./Type here.hi";

/// The test picture of `m6.rs`: background, and (centre x, centre y,
/// colour) of each square. Keep in sync.
const PICTURE_BACKGROUND: u32 = 0x20_26_34;
const PICTURE_SQUARES: [(usize, usize, u32); 4] = [
    (150, 150, 0xe0_53_3d),
    (350, 150, 0x3f_b9_50),
    (550, 150, 0x38_8b_fd),
    (750, 150, 0xff_ff_ff),
];

/// A connection to QEMU's control socket (QMP).
struct Qmp {
    out: std::os::unix::net::UnixStream,
    lines: BufReader<std::os::unix::net::UnixStream>,
}

impl Qmp {
    fn connect(path: &Path) -> Result<Qmp, String> {
        let deadline = Instant::now() + Duration::from_secs(10);
        let stream = loop {
            match std::os::unix::net::UnixStream::connect(path) {
                Ok(s) => break s,
                Err(e) if Instant::now() > deadline => {
                    return Err(format!("cannot reach QEMU's control socket: {e}"));
                }
                Err(_) => thread::sleep(Duration::from_millis(100)),
            }
        };
        stream
            .set_read_timeout(Some(Duration::from_secs(30)))
            .map_err(|e| e.to_string())?;
        let lines = BufReader::new(stream.try_clone().map_err(|e| e.to_string())?);
        let mut q = Qmp { out: stream, lines };
        q.read_line()?;
        q.command(r#"{"execute":"qmp_capabilities"}"#)?;
        Ok(q)
    }

    fn read_line(&mut self) -> Result<String, String> {
        let mut line = String::new();
        match self.lines.read_line(&mut line) {
            Ok(0) => Err("QEMU closed its control socket".into()),
            Ok(_) => Ok(line),
            Err(e) => Err(format!("reading QEMU's control socket: {e}")),
        }
    }

    /// Send one command and wait for its answer (skipping events).
    fn command(&mut self, json: &str) -> Result<String, String> {
        use std::io::Write;
        writeln!(self.out, "{json}").map_err(|e| e.to_string())?;
        loop {
            let line = self.read_line()?;
            if line.contains("\"return\"") {
                return Ok(line);
            }
            if line.contains("\"error\"") {
                return Err(format!("QEMU refused {json}: {}", line.trim()));
            }
        }
    }
}

/// Take a screenshot and check it shows the test picture.
fn check_test_picture(q: &mut Qmp, path: &Path) -> Result<(), String> {
    let _ = fs::remove_file(path);
    q.command(&format!(
        r#"{{"execute":"screendump","arguments":{{"filename":"{}"}}}}"#,
        path.display()
    ))?;
    let data = fs::read(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let (w, h, px) = parse_ppm(&data).ok_or("the screenshot is not a PPM picture")?;
    if (w, h) != (1280, 800) {
        return Err(format!("the screen is {w} by {h}, not 1280 by 800"));
    }
    let at = |x: usize, y: usize| {
        let i = (y * w + x) * 3;
        u32::from(px[i]) << 16 | u32::from(px[i + 1]) << 8 | u32::from(px[i + 2])
    };
    let mut wrong = Vec::new();
    for &(x, y, c) in PICTURE_SQUARES.iter() {
        if at(x, y) != c {
            wrong.push(format!("({x}, {y}) is {:06x}, not {c:06x}", at(x, y)));
        }
    }
    for (x, y) in [(10, 10), (640, 600), (1270, 790)] {
        if at(x, y) != PICTURE_BACKGROUND {
            wrong.push(format!(
                "({x}, {y}) is {:06x}, not the background",
                at(x, y)
            ));
        }
    }
    if wrong.is_empty() {
        Ok(())
    } else {
        Err(format!("the screenshot is wrong: {}", wrong.join("; ")))
    }
}

/// Width, height and RGB bytes of a binary PPM (P6, 8 bits).
fn parse_ppm(data: &[u8]) -> Option<(usize, usize, &[u8])> {
    let mut fields = Vec::new();
    let mut i = 0;
    while fields.len() < 4 {
        while data.get(i)?.is_ascii_whitespace() {
            i += 1;
        }
        if data[i] == b'#' {
            while data.get(i)? != &b'\n' {
                i += 1;
            }
            continue;
        }
        let start = i;
        while !data.get(i)?.is_ascii_whitespace() {
            i += 1;
        }
        fields.push(std::str::from_utf8(&data[start..i]).ok()?);
    }
    if fields[0] != "P6" || fields[3] != "255" {
        return None;
    }
    let (w, h): (usize, usize) = (fields[1].parse().ok()?, fields[2].parse().ok()?);
    let px = data.get(i + 1..i + 1 + w * h * 3)?;
    Some((w, h, px))
}

/// Type "a" on the virtual keyboard, then move the virtual mouse to the
/// middle of the screen and click.
fn send_key_and_click(q: &mut Qmp) -> Result<(), String> {
    // No device named: QEMU sends events to the input devices that were
    // switched on last, which are Alaya's virtio keyboard and mouse.
    let send = |q: &mut Qmp, _device: &str, events: &str| {
        q.command(&format!(
            r#"{{"execute":"input-send-event","arguments":{{"events":[{events}]}}}}"#
        ))
    };
    let key = |down: bool| {
        format!(r#"{{"type":"key","data":{{"down":{down},"key":{{"type":"qcode","data":"a"}}}}}}"#)
    };
    send(q, "kbd0", &key(true))?;
    send(q, "kbd0", &key(false))?;
    let abs = |axis: &str| format!(r#"{{"type":"abs","data":{{"axis":"{axis}","value":16384}}}}"#);
    let btn = |down: bool| format!(r#"{{"type":"btn","data":{{"down":{down},"button":"left"}}}}"#);
    send(q, "tablet0", &format!("{},{}", abs("x"), abs("y")))?;
    send(q, "tablet0", &btn(true))?;
    send(q, "tablet0", &btn(false))?;
    Ok(())
}

/// Pointer positions in QEMU's tablet units for a 1280 by 800 screen.
fn abs_event(x: i64, y: i64) -> String {
    let ax = (x.clamp(0, 1279) * 32768 / 1280).min(32767);
    let ay = (y.clamp(0, 799) * 32768 / 800).min(32767);
    format!(
        r#"{{"type":"abs","data":{{"axis":"x","value":{ax}}}}},{{"type":"abs","data":{{"axis":"y","value":{ay}}}}}"#
    )
}

fn input_events(q: &mut Qmp, events: &str) -> Result<String, String> {
    q.command(&format!(
        r#"{{"execute":"input-send-event","arguments":{{"events":[{events}]}}}}"#
    ))
}

/// Press the left button at `from`, move to `to` in steps, let go.
fn drag(q: &mut Qmp, from: (i64, i64), to: (i64, i64)) -> Result<(), String> {
    let btn = |down: bool| format!(r#"{{"type":"btn","data":{{"down":{down},"button":"left"}}}}"#);
    input_events(q, &abs_event(from.0, from.1))?;
    thread::sleep(Duration::from_millis(200));
    input_events(q, &btn(true))?;
    for i in 1..=10 {
        let x = from.0 + (to.0 - from.0) * i / 10;
        let y = from.1 + (to.1 - from.1) * i / 10;
        input_events(q, &abs_event(x, y))?;
        thread::sleep(Duration::from_millis(50));
    }
    thread::sleep(Duration::from_millis(200));
    input_events(q, &btn(false))?;
    Ok(())
}

/// Type keys on the virtual keyboard, as the first program spells them
/// (`alaya_programs::desktop::keys`): characters of a US keyboard, and
/// `{tab}`, `{enter}`, `{left}`, `{right}`, `{up}`, `{down}`,
/// `{backspace}`, `{escape}` and `{ctrl+x}`.
fn type_keys(q: &mut Qmp, spec: &str) -> Result<(), String> {
    let key = |q: &mut Qmp, code: &str, down: bool| {
        input_events(
            q,
            &format!(
                r#"{{"type":"key","data":{{"down":{down},"key":{{"type":"qcode","data":"{code}"}}}}}}"#
            ),
        )
        .map(|_| thread::sleep(Duration::from_millis(30)))
    };
    let press = |q: &mut Qmp, codes: &[&str]| -> Result<(), String> {
        for c in codes {
            key(q, c, true)?;
        }
        for c in codes.iter().rev() {
            key(q, c, false)?;
        }
        Ok(())
    };
    let mut rest = spec;
    while let Some(c) = rest.chars().next() {
        if c == '{'
            && let Some(end) = rest.find('}')
        {
            let name = &rest[1..end];
            rest = &rest[end + 1..];
            let code = match name {
                "enter" => "ret".to_string(),
                "backspace" => "backspace".to_string(),
                "escape" => "esc".to_string(),
                "compose" => "compose".to_string(),
                "space" => "spc".to_string(),
                n if n.starts_with("super+")
                    || n.starts_with("ctrl+")
                    || n.starts_with("shift+") =>
                {
                    // Modifiers may chain: `{super+shift+right}`.
                    let mut k = n;
                    let mut held: Vec<&str> = Vec::new();
                    loop {
                        if let Some(r) = k.strip_prefix("super+") {
                            (k, held) = (r, [held, vec!["meta_l"]].concat());
                        } else if let Some(r) = k.strip_prefix("ctrl+") {
                            (k, held) = (r, [held, vec!["ctrl"]].concat());
                        } else if let Some(r) = k.strip_prefix("shift+") {
                            (k, held) = (r, [held, vec!["shift"]].concat());
                        } else {
                            break;
                        }
                    }
                    let (code, shift) = match k {
                        "up" | "down" | "left" | "right" | "tab" | "delete" => {
                            (k.to_string(), false)
                        }
                        "enter" => ("ret".to_string(), false),
                        "space" => ("spc".to_string(), false),
                        "escape" => ("esc".to_string(), false),
                        _ => {
                            let c = k.chars().next().ok_or("a modifier with what?")?;
                            qcode(c).ok_or(format!("no key for {c}"))?
                        }
                    };
                    if shift && !held.contains(&"shift") {
                        held.push("shift");
                    }
                    held.push(&code);
                    press(q, &held)?;
                    continue;
                }
                n => n.to_string(),
            };
            press(q, &[&code])?;
            continue;
        }
        rest = &rest[c.len_utf8()..];
        let (code, shift) = qcode(c).ok_or(format!("no key for {c:?}"))?;
        if shift {
            press(q, &["shift", &code])?;
        } else {
            press(q, &[&code])?;
        }
    }
    Ok(())
}

/// QEMU's name for the key that types `c` on a US keyboard, and whether
/// it needs Shift.
fn qcode(c: char) -> Option<(String, bool)> {
    const PLAIN: &str = "1234567890-=[];'`\\,./";
    const NAMES: [&str; 21] = [
        "1",
        "2",
        "3",
        "4",
        "5",
        "6",
        "7",
        "8",
        "9",
        "0",
        "minus",
        "equal",
        "bracket_left",
        "bracket_right",
        "semicolon",
        "apostrophe",
        "grave_accent",
        "backslash",
        "comma",
        "dot",
        "slash",
    ];
    if c == ' ' {
        return Some(("spc".into(), false));
    }
    if c.is_ascii_lowercase() || c.is_ascii_digit() {
        return Some((c.to_string(), false));
    }
    if c.is_ascii_uppercase() {
        return Some((c.to_ascii_lowercase().to_string(), true));
    }
    if let Some(i) = PLAIN.find(c) {
        return Some((NAMES[i].into(), false));
    }
    // Shifted symbols, in the order of the plain ones.
    let order = "!@#$%^&*()_+{}:\"~|<>?";
    let base = "1234567890-=[];'`\\,./";
    let i = order.find(c)?;
    let plain = base.chars().nth(order[..i].chars().count())?;
    PLAIN.find(plain).map(|j| (NAMES[j].into(), true))
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
