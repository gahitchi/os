# The desktop (milestone 6)

Milestone 6 gives Alaya a screen, a keyboard and a mouse, and then a simple
desktop on top of them. It comes in three steps (6a, 6b, 6c); the plan is in
the design notes, and the choices behind it are decisions 104 to 106 in
[`design/decisions.md`](design/decisions.md).

## Step 1: the screen, the keyboard and the mouse

QEMU now has three more virtual devices, all behind the device memory guard
(IOMMU) like the disk:

- a **screen card** (`virtio-gpu-pci`, 1280 by 800 pixels), and QEMU's
  standard VGA card is switched off (`-vga none`), so Alaya's screen is the
  only one;
- a **keyboard** (`virtio-keyboard-pci`);
- a **mouse** of the "tablet" kind (`virtio-tablet-pci`): it reports where
  the pointer is, so Alaya's pointer follows the host's exactly, also over a
  remote screen.

Each has its own driver, an ordinary program started from the boot image
(`user/programs/src/bin/gpu.rs` and `input.rs`, sharing
`user/programs/src/virtio.rs`). The first program, acting as device
manager, finds the devices on the PCI bus, locks each to its own memory and
starts the drivers (`user/init/src/m6.rs`).

**The driver owns nothing.** The screen memory belongs to the program that
draws (for now the first program, next the compositor). It is lent to the
screen card only: the driver never even sees the pixels. The driver tells
the card where the screen memory is, and copies changed parts to the screen
when asked. If the driver crashes, a new one starts, describes the same
memory to the card again and shows what is there: the picture comes back
without anyone drawing it again. The keyboard and mouse drivers write their
events into a ring in a page their client owns; a new instance carries on
after the last event.

QEMU's memory went from 256 MiB to 512 MiB by default: on UEFI with 8 cores
the firmware leaves too little in one piece for the screen.

## How it is tested

Five new boot checks (54 to 58), on PCs and ARM:

- the screen driver starts, locked to its own memory, and finds the size;
- a test picture drawn into the screen memory reaches the screen: `cargo
  xtask test` takes a screenshot through QEMU's control socket (QMP) and
  checks the colour of every square and of the background;
- the keyboard and mouse drivers start and tell which is which;
- `cargo xtask test` types "a" and clicks the middle of the screen through
  QMP; the key and the click must arrive, at the right place;
- every driver is crashed on purpose and must come back; a second
  screenshot must show the same picture.

The phone script (`tools/termux/alaya.sh`) adds the same devices; with no
one typing, the input check waits ten seconds and says it was skipped.

## Seeing it

`cargo xtask run` now opens QEMU's own window and shows the test picture
after the boot checks (`--display gtk`, `--display sdl` or `--vnc` to
choose; `--vnc` serves the screen on port 5900, for a VNC viewer app on a
phone).
