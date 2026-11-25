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
sudo pacman -S --needed base-devel rustup xorriso clang
rustup default stable
git clone https://github.com/gahitchi/os
cd os
cargo xtask test --accel kvm --cpus "$(nproc)"          # BIOS boot
cargo xtask test --accel kvm --cpus "$(nproc)" --uefi   # UEFI boot
cargo xtask run  --accel kvm --cpus "$(nproc)"          # watch it; Ctrl+A then x quits
```

The first build downloads the pinned Rust version and Limine. To test a
branch, add `--branch <name>` to `git clone`.

The kernel fuzzer runs the boot checks, then has threads on every core
throw random system calls at the kernel for the given number of seconds:

```sh
cargo xtask test --accel kvm --cpus "$(nproc)" --fuzz 300
```

The fault reports can be checked too: each of these crashes the kernel on
purpose and passes when the report says the right thing.

```sh
cargo xtask test --accel kvm --cpus 4 --fault page
cargo xtask test --accel kvm --cpus 4 --fault stack
cargo xtask test --accel kvm --cpus 4 --fault invalid-opcode
cargo xtask test --accel kvm --cpus 4 --fault other-core-page
```

Milestone 4 adds a long run: the boot checks, then the disk driver is
crashed or frozen at random every half second or so for ten minutes while
an app reads and writes the disk through the driver's keeper. It passes
when the app never got a wrong answer:

```sh
cargo xtask test --accel kvm --cpus "$(nproc)" --long
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
init: hello from user mode
init: ok  1 boot capabilities: own table and untyped memory
...
init: revoked 16383 capabilities; the kernel interrupted the revoke ... times
init: ok  8 a big revoke is interrupted and restarted, and completes
...
init: ok 12 2 MiB pages are aligned
init: ok 13 threads start, and a budgeted thread wakes every period
...
init: a 2 ms / 10 ms budget used ... µs of ... µs (20%)
...
init: ipc: call and reply round trip: ... cycles (... ns) between programs, ... cycles (... ns) inside one
...
init: ok 25 revoking takes back running threads, address spaces and all
...
init: chaos: ... restarts, ... right answers, 0 wrong
...
init: ok 43 the core-type helper puts games on performance cores and background work on efficiency cores
init: all 43 checks passed (...)
alaya: boot ok
```

The exact CPU name, core count, TSC source and numbers depend on the
machine. What matters: one `core N: ready` line for every core, the
TSC-deadline timer in use, forty-three `init: ok` lines from the first program,
and `alaya: boot ok` at the end. The `ipc:` line is the cost of a call and
its answer between two programs; with KVM it is the real number. With
`--fuzz`, `fuzz: done` comes before the end. If something
fails, the last lines of the output (a fault report or panic message) say
what and where. Lines from the keeper such as `keeper: driver version 1
crashed` are expected: the tests crash the driver on purpose.

## On a Windows PC (WSL2)

A Windows PC can run the same tests inside WSL2, Windows' built-in Linux.
WSL2 passes the processor's virtualization through, so QEMU there can use
KVM too. This is a helpful second machine; the reference laptop's runs stay
the official ones.

### Setup

1. Turn on virtualization (Intel VT-x) in the PC's firmware settings, if
   it is not already on. Task Manager, Performance, CPU shows
   "Virtualization: Enabled" when it is.
2. In PowerShell as administrator:

   ```powershell
   wsl --install -d Ubuntu
   ```

   Restart when asked, then open Ubuntu from the Start menu and choose a
   user name and password.
3. Tell WSL2 to pass virtualization through. In Windows, create (or edit)
   the file `%UserProfile%\.wslconfig` with:

   ```ini
   [wsl2]
   nestedVirtualization=true
   ```

   Then in PowerShell: `wsl --shutdown`, and open Ubuntu again.
4. In Ubuntu:

   ```sh
   sudo apt update
   sudo apt install qemu-system-x86 ovmf curl git build-essential xorriso clang
   sudo usermod -aG kvm "$USER"
   ```

   Close Ubuntu, run `wsl --shutdown` in PowerShell, open Ubuntu again, and
   check with `ls -l /dev/kvm`.

After that, Option 1 and Option 2 above work the same way (for Option 2,
install Rust with the command on [rustup.rs](https://rustup.rs) instead of
pacman). Keep the clone inside Ubuntu's own home folder (`~/os`), not under
`/mnt/c`: building on the Windows drive is many times slower.

### VirtualBox and WSL2 together

WSL2 runs on Windows' own virtualization layer (Hyper-V). Once it is on,
VirtualBox can no longer use the processor's virtualization directly and
runs its machines through Hyper-V instead, which is slower and sometimes
shows a green turtle icon in the VirtualBox window. Both keep working, just
VirtualBox more slowly.

For Alaya, use WSL2, not VirtualBox: QEMU inside WSL2 gets KVM, and the
tests use QEMU's device memory guard and virtual disk, which a VirtualBox
machine does not offer in the same way. If you need VirtualBox at full
speed for something else, Hyper-V can be switched off for a while
(`bcdedit /set hypervisorlaunchtype off` as administrator and a restart;
`auto` instead of `off` turns it back on), but WSL2 does not run while it
is off.
