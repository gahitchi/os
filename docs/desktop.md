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

## Step 2: the toolkit and the desktop

**The toolkit** (`ui/`, the crate `alaya-ui`) is what the desktop and every
app are drawn with. Like the store, it is plain logic that does not care
where its pixels come from, so it is tested on the build machine (`cargo
test -p alaya-ui`). It has:

- **Alaya's own font reader** (`font.rs`): reads TrueType files (which
  glyph a character uses, its outline, its width), every read checked, and
  turns outlines into soft-edged letters by adding up how much of each
  pixel the outline covers. Broken or hostile font files give missing
  letters, never a crash (a test flips random bytes and draws everything).
- **Alaya's letters** (decision 105, `fonts/`): Figtree for text, Figtree
  SemiBold for titles and buttons, IBM Plex Mono for code, and Noto Sans
  for alphabets Figtree lacks (Greek, Cyrillic and more). A character no
  font has shows as an empty box. Fonts keep their own free licence (SIL
  Open Font Licence, next to each file).
- **Drawing** (`canvas.rs`): rectangles with soft rounded corners, frames,
  shadows, gradients, pictures; everything clipped, so a wrong size never
  writes outside.
- **The window rules** (`wm.rs`, decisions 34 and 87): front and back,
  moving by the title bar, resizing by any edge (never below a minimum),
  snapping to the left or right half or the whole screen by dragging to an
  edge (with a preview), coming loose again at the old size, and tiling
  (windows share the screen without overlapping; drop one on another to
  swap them). Keys go only to the window in front; the mouse goes to the
  window under it, or to the one that holds it while a button is down.
- **Widgets** (`widgets.rs`): labels, buttons, text boxes (one or many
  lines) and lists, in the mock-up's look. While an app draws, every piece
  is written into the **accessibility tree** with what it is and what it is
  called (decision 42): tests already use it to press buttons by name.
- **The US English keyboard** (decision 106).

**The desktop** (`user/apps/src/bin/desktop.rs`) is Alaya's compositor. It
owns the screen memory and draws the wallpaper, windows, the dock, the top
strip (which no app can draw on: the trusted PIN picture of decision 104
will show there) and the pointer. It draws only what changed, and only
when the screen driver has shown the last frame. Apps call it to open a
window; the desktop makes the window's picture and lends it to the app,
which draws into it with the toolkit and says which part changed. Keys,
the pointer, resizes, focus and the close button reach the app as events.
Super with an arrow key snaps the window in front; Super+T tiles.

**The first app** is a simple text editor (`editor.rs`). Saving to the
store and reopening where you were come in step 3.

Checks 59 to 63: the desktop draws (the first program checks the strip's
colour in the screen memory); the editor opens a window; `cargo xtask test`
drags its title bar to the left edge through QEMU and it snaps to the left
half; typed keys reach the editor; tiling shares the screen and goes back;
the screen driver crashes under the desktop and drawing carries on. The
test saves a screenshot of the desktop in `build/screen-desktop.ppm`.

## Seeing it

`cargo xtask run` opens QEMU's own window: after the boot checks you get
the desktop with the editor; type in it, drag it, snap it (Super+arrows),
tile with Super+T (`--display gtk`, `--display sdl` or `--vnc` to
choose; `--vnc` serves the screen on port 5900, for a VNC viewer app on a
phone).
