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

**The first app** was a simple text editor; step 3 makes it save, and adds
more apps (below).

## Step 3 (6b): people and apps

**First setup and login** are drawn by the desktop itself
(`ui/src/login.rs`, `user/apps/src/bin/desktop.rs`). First setup asks for a
name, a PIN (twice), a picture and what Alaya will be used for (two looks:
"Just the basics", or "Everything on" with the chart maker and the group
labels; decision 90). The login screen has a tile per person (decision 30)
and a PIN box. Keys and the mouse go to the login screen and to nothing
else.

**The trusted PIN box (decision 104).** While the real PIN box is open, the
person's picture shows in the top strip, which only the desktop draws: no
window can cover it, and no app ever sees the picture (it lives in the
machine's own files, which no app has a badge for; the login tiles show an
initial instead). A fake login screen drawn by an app cannot show it.

**From PIN to key.** The PIN goes through a slow step that needs memory
(`pin_key` in the new `crypto/` crate: scrypt's ROMix with SHA-256, 2 MiB
per guess here), salted per machine and per person. The key opens the
person's files in the store; a wrong PIN opens nothing. The TPM, which
limits guesses for real, comes with milestone 13; until then the key of the
machine's own files is in the image, and the screen says so.

**The login parts (decision 74).** The first program fingerprints the
programs that draw the login screen and hold the keys (the desktop and the
store) and hands the fingerprint to the desktop, which compares it with the
last login's and says on the login screen what it found: first login, the
same, or changed. It is labelled "checked by Alaya, not yet by the chip".

**The apps** are one program image (`user/apps/src/bin/apps/`) started once
per app, each with its own memory, its own badge on the desktop and on the
store, and its own transfer pages:
- **Text editor**: `Notes.txt` in the person's documents; Ctrl+S saves.
  After every change it writes its return point (the text, saved or not,
  the cursor and the first line shown); the store makes it safe within five
  seconds, so the editor comes back exactly where you were after it is
  closed, crashes or is stopped.
- **Files**: the documents, with sizes and versions (the living-validity
  marks come in step 4, 6c).
- **Chart maker**: a bar chart of `numbers.txt`, redrawn when it changes;
  "Save picture" (or Ctrl+S) saves `chart.ppm`.
- **Settings**: the look; the desktop follows at once.
- **The clock** in the strip counts time since start until a real-time clock
  driver exists.

The dock starts an app that is not running and brings a running one to the
front. **Loose groups** (decision 85): drop a window on a group label left of
the dock and it is in that group (a label in its title bar, nothing more);
clicking the label brings the group's windows to the front.

**Looking after.** After its checks the first program keeps running: it
starts the apps the dock asks for and starts again any driver or app that
crashes. The desktop's calls that steer it (lock, inject input, close an
app's windows, ask its state) answer only the first program.

## How the desktop is tested

Checks 59 to 71, on PCs and ARM: first setup typed through QEMU (name, PIN
twice, picture, look); the editor opens and is dragged to the left half;
typing and tiling; the screen driver crashes under the desktop; the lock
screen shows the picture in the strip, refuses a wrong PIN and opens with
the right one, and the picture is gone after; the editor saves, is stopped
in the middle of typing and comes back with its text and cursor; the file
browser and the chart maker open and a chart is saved as a picture; a window
dropped on a group label joins the group; an app cannot use the desktop's
own calls or draw on the strip; Settings changes the look; the keyboard
driver crashes and typing carries on. `cargo xtask test` saves screenshots
in `build/screen-setup.ppm`, `screen-login.ppm`, `screen-apps.ppm` and
`screen-desktop.ppm`. Without a host typing (the phone, or
`ALAYA_NO_HOST_INPUT=1`), the first program sends the same keys and mouse
moves to the desktop itself.

## Seeing it

`cargo xtask run` opens QEMU's own window. It boots an image with the word
`desk`: the checks run first (about a minute), but the desktop's own checks
do not type for themselves. When the log says `init: desktop: it is yours
now`, first setup is waiting for you: your name, a PIN twice, your picture
and the look. The text editor opens after you log in; open the other apps
from the dock, type, drag, snap (Super+arrows), tile with Super+T (`--display gtk`, `--display sdl` or `--vnc` to
choose; `--vnc` serves the screen on port 5900, for a VNC viewer app on a
phone).
