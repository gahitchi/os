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
| `ALAYA_CPUS` | 1 | Number of virtual cores, for example `ALAYA_CPUS=4 bash alaya.sh run` |
| `ALAYA_TIMEOUT` | 300 | Seconds before `test` gives up |
| `ALAYA_IMAGE_URL` | nightly release | Download the image from somewhere else |
| `ALAYA_DIR` | `~/alaya` | Where the image is kept |

## Images from a pull request

Every CI run, including pull requests, uploads the image as the
`alaya-x86_64` artifact on its run page (you need to be signed in to GitHub to
download it). Unzip it and pass the `.iso` to `alaya.sh run`.

## What you should see

```
Alaya 0.0.1 (x86-64)
boot: Limine 9.6.7
cpu: 1 core(s), bootstrap LAPIC id 0
mem: 253 MiB usable in 3 region(s), 16 map entries
mem: direct map at 0xffff800000000000
alaya: boot ok
```
