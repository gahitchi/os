# Booting Alaya on an Android phone (Termux)

You can boot Alaya on a phone without building anything. CI publishes a
ready-made bootable image, and QEMU in Termux runs it with plain emulation
(TCG). There is no KVM on a phone, so booting takes longer than on a PC, but
the kernel is small.

## One-time setup

In Termux:

```sh
pkg update
pkg install qemu-system-x86-64-headless curl
curl -fLO https://raw.githubusercontent.com/gahitchi/os/main/tools/termux/alaya.sh
```

The script installs QEMU and curl itself if they are missing.

## Boot it

```sh
bash alaya.sh run
```

This downloads the latest image from the
[nightly release](https://github.com/gahitchi/os/releases/tag/nightly)
into `~/alaya/` (checking its checksum), then boots it. Kernel output appears
on the terminal. When it prints `alaya: boot ok` the kernel has finished:
it has nothing else to run yet, so it waits there.

To quit QEMU, press **Ctrl+A**, let go, then press a lowercase **x**. In
Termux, use CTRL from the extra-keys row, and make sure Shift is off.
If that does not work, press **Ctrl+A** then **c** to reach the `(qemu)`
prompt and type `quit`. As a last resort, open a new Termux session (swipe in
from the left) and run `pkill -f qemu-system`.

To boot and exit on its own instead, use `bash alaya.sh test`.

Other commands:

| Command | What it does |
| --- | --- |
| `bash alaya.sh test` | Boot, and report PASS or FAIL from the kernel's output |
| `bash alaya.sh fetch` | Download the newest image again |
| `bash alaya.sh run path/to/image.iso` | Boot a specific image, for example one from a pull request's CI run |

Settings, given in front of the command:

| Variable | Default | Meaning |
| --- | --- | --- |
| `ALAYA_CPUS` | 1 | Number of virtual cores, for example `ALAYA_CPUS=4 bash alaya.sh run`; `all` uses as many as the device has |
| `ALAYA_ACCEL` | auto | `kvm`, `tcg` (plain emulation) or `auto` (KVM if `/dev/kvm` is usable, which it is not on a phone) |
| `ALAYA_TIMEOUT` | 300 | Seconds before `test` gives up |
| `ALAYA_IMAGE_URL` | nightly release | Download the image from somewhere else |
| `ALAYA_DIR` | `~/alaya` | Where the image is kept |

## Images from a pull request

Every CI run, including pull requests, uploads the image as the
`alaya-x86_64` artifact on its run page (you need to be signed in to GitHub to
download it). Unzip it and pass the `.iso` to `alaya.sh run`.

## What you should see

With `ALAYA_CPUS=2` (the memory map lines are shortened here):

```
Alaya 0.0.1 (x86-64)
boot: Limine 9.6.7
cpu: QEMU Virtual CPU version 2.5+
cpu: x2APIC no, TSC deadline no, invariant TSC no, 1 GiB pages no, running under a hypervisor
cpu: 2 core(s), bootstrap APIC id 0, APIC mode xAPIC
mem: 0x000000001000-0x000000053000       328 KiB  bootloader reclaimable
...
mem: 253 MiB usable
mem: kernel page tables at 0x100000: direct map 255 MiB (...)
mem: kernel took 276 KiB for page tables and stacks of 2 core(s)
cpu 0: online, APIC id 0 (bootstrap)
time: TSC 2100.687 MHz (measured against the PIT), invariant: no
time: this CPU has no TSC-deadline timer (plain emulation?); using the APIC one-shot timer
cpu 1: online, APIC id 1
smp: 2 of 2 cores online
bkl: 2 core(s) x 20000 rounds under the big kernel lock, counter 40000: ok
core 0: ready, APIC id 0, APIC one-shot timer, 20 ticks, worst lateness 288 us
core 1: ready, APIC id 1, APIC one-shot timer, 20 ticks, worst lateness 293 us
init: 904 KiB program, 23 untyped blocks, 251 MiB of untyped memory
init: hello from user mode
init: 23 untyped blocks, 251 MiB, largest 64 MiB, on 2 cores
init: ok  1 boot capabilities: own table and untyped memory
init: ok  2 bad handles, kinds, sizes and calls are refused
init: ok  3 retype: untyped, a table, 100 endpoints in batches
init: ok  4 copy and mint: rights only shrink, one badge, grant needed
init: ok  5 move
init: ok  6 revoke deletes every copy and keeps the original
init: ok  7 delete keeps what was derived revocable
init: revoked 16383 capabilities; the kernel interrupted the revoke 27 times
init: ok  8 a big revoke is interrupted and restarted, and completes
init: revoking the 16 MiB untyped restarted 19 times
init: ok  9 revoking untyped empties unreachable tables, then resets it
init: ok 10 revoked memory is reused, and new tables are empty
init: ok 11 untyped memory runs out, and comes back after revoke
init: ok 12 2 MiB pages are aligned
init: all 12 checks passed (47 restarted calls, 16492 capabilities revoked)
alaya: boot ok
```

There is one `core N: ready` line per core; `alaya.sh test` passes only if
all of them are there. The `init:` lines come from the first program, which
runs in user mode and checks capabilities and memory through system calls;
the kernel prints `alaya: boot ok` only when all of its checks passed. The
numbers of interruptions vary from run to run. A phone has no KVM, so QEMU emulates the CPU and the
kernel uses the APIC one-shot timer instead of the TSC-deadline timer, and
the timer lateness is in the hundreds of microseconds. On a PC with KVM
(see [`testing-on-a-pc.md`](testing-on-a-pc.md)) it uses the real timer.
