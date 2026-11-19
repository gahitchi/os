# Testing Alaya on a Linux PC with KVM

On a Linux PC, QEMU can run Alaya with KVM: the virtual cores are real cores
of the host CPU, so the kernel meets the real TSC-deadline timer, x2APIC and
as many cores as the machine has. This is the closest thing to real hardware
without booting Alaya on the machine itself, and nothing on the PC changes.

The steps use Arch Linux; other distributions need the same packages under
their own names.

## One-time setup

```sh
sudo pacman -S --needed qemu-desktop edk2-ovmf curl git
sudo usermod -aG kvm "$USER"
```

Log out and back in (or reboot) so the `kvm` group applies, then check:

```sh
ls -l /dev/kvm          # should show group kvm with rw for the group
id -nG | grep -w kvm    # should print a line containing kvm
```

If `/dev/kvm` is missing, turn on virtualization (Intel VT-x) in the
firmware settings.

## Option 1: boot the image CI built (nothing to compile)

```sh
curl -fLO https://raw.githubusercontent.com/gahitchi/os/main/tools/termux/alaya.sh
ALAYA_CPUS=all bash alaya.sh test     # boot on every core, report PASS or FAIL
ALAYA_CPUS=all bash alaya.sh run      # boot and watch; Ctrl+A then x quits
```

The script uses KVM automatically when `/dev/kvm` is usable and says so
("with KVM"). `ALAYA_ACCEL=kvm` makes it fail instead of falling back to
emulation, which is useful to be sure. `bash alaya.sh fetch` downloads the
newest image again.

This boots the [nightly release](https://github.com/gahitchi/os/releases/tag/nightly),
which follows `main`. To test a pull request before it is merged, download
the `alaya-x86_64` artifact from the pull request's CI run (signed in to
GitHub), unzip it, and pass the image:

```sh
ALAYA_CPUS=all bash alaya.sh test ~/Downloads/alaya-x86_64.iso
```

## Option 2: build from source

Needs the Rust toolchain and the image tools as well:

```sh
sudo pacman -S --needed base-devel rustup xorriso
rustup default stable
git clone https://github.com/gahitchi/os
cd os
cargo xtask test --accel kvm --cpus "$(nproc)"          # BIOS boot
cargo xtask test --accel kvm --cpus "$(nproc)" --uefi   # UEFI boot
cargo xtask run  --accel kvm --cpus "$(nproc)"          # watch it; Ctrl+A then x quits
```

The first build downloads the pinned Rust version and Limine. To test a
branch, add `--branch <name>` to `git clone`.

The fault reports can be checked too: each of these crashes the kernel on
purpose and passes when the report says the right thing.

```sh
cargo xtask test --accel kvm --cpus 4 --fault page
cargo xtask test --accel kvm --cpus 4 --fault stack
cargo xtask test --accel kvm --cpus 4 --fault invalid-opcode
cargo xtask test --accel kvm --cpus 4 --fault other-core-page
```

## What a good run looks like

With KVM on an Intel CPU the kernel should report, among other lines:

```
cpu: x2APIC yes, TSC deadline yes, invariant TSC yes, ...
cpu: 20 core(s), bootstrap APIC id 0, APIC mode x2APIC
time: TSC ... MHz (CPUID leaf 0x15), invariant: yes
time: timer is the TSC-deadline timer
smp: 20 of 20 cores online
bkl: 20 core(s) x 20000 rounds under the big kernel lock, counter 400000: ok
core 0: ready, APIC id 0, TSC deadline timer, 20 ticks, worst lateness ... us
...
alaya: boot ok
```

The exact CPU name, core count, TSC source and numbers depend on the
machine. What matters: one `core N: ready` line for every core, the
TSC-deadline timer in use, and `alaya: boot ok` at the end. If something
fails, the last lines of the output (a fault report or panic message) say
what and where.
