# Alaya on any device

Written 20 November 2026, after your question: *can Alaya tell what it boots on (a phone, a board like a Raspberry Pi or an Arduino, a laptop, a desktop) and adapt to it, including to whichever processor it finds?*

Short answer: **yes for almost everything with a "real" processor, no for Arduino-sized chips.** It splits into two jobs that work very differently:

1. **Adapting to the processor** (the chip that runs everything). A program is written in one processor's language, so this cannot be done by detection alone. Alaya has to be built once for each processor family. The trick is to carry all those builds on one stick and let the computer pick the right one.
2. **Adapting to the kind of device** (laptop, desktop, handheld, tablet, board, phone). This *is* detection: Alaya looks at what the device tells it and sets sensible defaults. It is cheap and fits the plan now.

Nothing below is decided yet. The three choices are at the end (section 8).

---

## 1. The processor families, in plain words

A processor family is like a language. A laptop's Intel chip "speaks" x86-64. A Raspberry Pi, almost every phone and Apple's newer Macs "speak" ARM. A few new boards speak RISC-V. A program written for one cannot be read by another, the same way a book in Japanese can't be read by someone who only reads English. So "adapt to the processor" really means **"carry a translation for each one."**

| Family | Devices | Real for Alaya? |
| --- | --- | --- |
| x86-64 | Laptops, desktops, gaming handhelds (Steam Deck style), most PCs | Yes. Alaya runs here today |
| ARM (64-bit) | Raspberry Pi 4 and 5, phones, tablets, some laptops, Apple Macs | Yes, as a second build. Real work (section 5) |
| RISC-V | New small boards (VisionFive, Milk-V) | Possible later. Few devices, no games, no good graphics yet. Parked |
| Microcontrollers | Arduino Uno, ESP32, Raspberry Pi Pico | **No** (section 7) |

## 2. One stick that starts on any processor

The good news: modern computers already have a way to do this. Their start-up firmware (UEFI) looks in one fixed folder on a USB stick and picks **the file named for its own processor**: one file for x86-64, one for ARM, one for RISC-V. A PC takes the x86-64 file, an ARM board with UEFI takes the ARM file, and neither ever looks at the other.

So the design is:

- **One Alaya image, several builds inside.** The stick holds a kernel and system for x86-64 and one for ARM. The computer's firmware chooses; nobody has to pick a download.
- **Your files and history are the same everywhere.** Alaya's store (decision 15) is written in a processor-neutral format, so a disk or backup made on a PC opens on an ARM device and back.
- **The Raspberry Pi gets its own little door on the same stick.** A Pi doesn't use standard UEFI by default; it reads its own start-up files. Those few files sit on the same stick, so the Pi also boots from it. (The Pi 4 has a community UEFI firmware; the Pi 5's is less complete, so Alaya would bring a small Pi starter.)
- **Phones are the exception.** Phones can't start from a USB stick, and most lock their start-up. A phone needs its own install image per phone model, written to it once.
- **Limine already supports this.** The start-up program Alaya uses today (decision 6) supports x86-64, ARM and RISC-V. Alaya's own start-up program (decision 76) would be built twice too.
- **Cost:** the image is roughly twice as big (still small, well under 1 GB for the system itself).

## 3. What the engine needs to support another processor

Most of Alaya is written so it doesn't care about the processor. Today's numbers:

- The kernel is about **8,150 lines**. About **1,330** of them are already in a separate "x86 corner" (`kernel/src/arch/x86_64`).
- But about **15 other kernel files** still reach directly into x86 details (the timer, memory maps, system calls, the driver fence, starting other cores). Those need to move behind a clean wall first. **This is cheapest now**, while the kernel is small; every milestone that passes makes it more expensive.
- For ARM, the corner needs its own version of: memory maps, the "something happened" entry points (interrupts and faults), the interrupt controller, the timer, starting the other cores, the driver fence (ARM calls it SMMU instead of IOMMU), and reading a board's description of itself (a "device tree"). Roughly as big as milestones 1 and 2 together.
- **Everything above the kernel is recompiled, not rewritten**: the capability system, the keeper, the virtio drivers, the store, the desktop, all Rust. The language we use (decision 4) supports ARM and RISC-V out of the box.
- **Some things fit ARM naturally.** Phones and the Pi 5's successors mix fast and slow cores, which is exactly what decision 54 (core types) already handles.

## 4. Telling what kind of device it is

A small part of Alaya, the **device sense**, collects facts every device gives away, and makes one guess with reasons:

| Fact | Where it comes from | What it suggests |
| --- | --- | --- |
| "What kind of box am I?" | The firmware's own answer (laptop, desktop, tablet, convertible, handheld, server) | The strongest single clue |
| A battery | The power controller | Laptop, tablet, handheld or phone |
| A lid switch | The power controller | Laptop |
| A touchscreen, touchpad, joystick buttons | The input devices found | Tablet/phone, laptop, handheld |
| The screen's real size | The screen reports it in centimetres | Phone vs tablet vs laptop vs desktop |
| A tilt sensor | The sensor hub | Tablet or convertible |
| A board description naming the board | ARM boards (device tree) | Raspberry Pi and similar |
| Pin headers, no screen | The board description | A maker board, maybe with no screen at all |
| "I am a virtual machine" | The processor says so | QEMU, VirtualBox (useful for our own tests) |

From that it picks one **form**: desktop, laptop, convertible, tablet, handheld, phone, board, or virtual machine. It always keeps the reasons, so "Why does Alaya think this is a laptop?" has a plain answer.

### What the form changes

The form changes **defaults only**. It never changes what an app is allowed to do, never touches history, and never weakens a protection.

| Form | Defaults it sets |
| --- | --- |
| Desktop | Full speed, mouse and keyboard layout, no battery rules |
| Laptop | Balanced on battery (decision 45), touchpad gestures, closing the lid sleeps, stricter background rules on battery (decision 46) |
| Convertible | Laptop defaults; switches to the touch layout when folded |
| Tablet / phone | Touch-first layout, bigger targets, on-screen keyboard (decision 50), strong battery saving |
| Handheld | The Play look suggested first, controller navigation, game-friendly power limits |
| Board | A simple layout or none at all (it can run without a screen), the pins offered as devices you grant to apps (like Bluetooth devices in decision 49) |
| Virtual machine | No battery or heat rules; honest note that speed numbers are not real |

### How it fits the looks (decision 90)

**Form and look are two separate dials.** The *form* is what the device is (Alaya guesses it). The *look* is what you use it for (you pick it). At first setup Alaya says something like "This looks like a laptop" with a "That's not right" button, and then offers the looks, with a suggestion based on the form (Play on a handheld, Everyday on a tablet, Build on a board). You can still pick any look on any device.

### It adapts while running, not only at start

A laptop plugged into a dock with a big screen behaves like a desktop while docked. A convertible folded flat switches to touch. Unplugging power switches to battery rules. Each change shows a one-line note the first time it happens, so it never feels like the computer is doing things behind your back.

## 5. Battle-test: what can go wrong

**1. The guess is wrong.** A desktop with a backup battery (UPS) can look like a laptop. A laptop with its battery removed can look like a desktop. A 2-in-1 is both. *Answer:* the firmware's own answer counts most, a battery counts less, the person can correct it once and Alaya remembers it for that device, and since the form only changes defaults, a wrong guess can never hurt safety or data.

**2. Hidden bugs that only show on ARM.** x86 chips do memory steps in the order the program wrote them. ARM chips may reorder them for speed. Code that shares memory between cores (our message passing, the keeper, the scheduler) can be correct on x86 and broken on ARM, and **QEMU running on an x86 laptop will not show these bugs**, because the laptop's chip keeps everything in order. *Answer:* the engine's shared-memory code gets a review for ARM's rules, and ARM tests also run where the chip really reorders:
- on GitHub's free ARM computers (QEMU inside them, in the cloud, still no real-hardware boot of yours);
- on **your Android phone**, which is an ARM chip: the same Termux script you already use can run the ARM build in QEMU.
Both keep your rule: only QEMU, nothing boots on bare metal.

**3. Games are x86.** Almost every PC game is built for x86. On ARM they need a live translator (projects like FEX and Box64 do this, and Valve uses FEX for its own ARM headset). It works for many games but costs speed. *Answer:* the gaming target stays x86 PCs and handhelds. On ARM, Play is honest about it: "this game runs through translation and may be slower". Deep anti-cheat games stay "not yet" everywhere.

**4. Apps must exist for each processor.** Alaya's own apps are simply built twice; app bundles (decision 37) carry both builds, and the installer takes the right one. The Linux world (decisions 66 and 92) runs ARM Linux apps on ARM devices directly (Claude Code has an ARM Linux version), and x86 Linux apps through the same translator.

**5. Not every device has the same safety hardware.** Disk encryption (decision 29) uses a security chip (TPM) the Pi doesn't have. Some small boards also lack the driver fence that keeps a broken driver away from your memory (to be checked per board). *Answer:* a new rule: **when a device is missing safety hardware, Alaya says so in plain words** ("This board has no security chip, so your disk key is protected by your PIN only") and uses the best fallback. Decision 90's promise ("protections work the same in every look") stays true; across devices Alaya promises honesty, not identical hardware.

**6. Phones are hard for real.** Most phone makers keep their drivers secret and lock the start-up. Even the Linux phone community supports only a handful of phones after years of work. *Answer:* phones only one specific open model at a time, much later, each with its own image.

**7. Twice the testing.** Every milestone would have to pass on two processors. *Answer:* depends on the choice in section 8. The cheap version keeps only the engine tests on ARM until the full ARM milestone.

**8. Privacy.** Device facts could identify you (serial numbers, exact models). *Answer:* the device sense keeps facts on the device; apps see only the form and the kinds of input ("touch available"), never serial numbers.

**9. Moving an Alaya stick between computers.** If one stick boots a desktop on Monday and a laptop on Tuesday, it should remember each machine's settings. Works naturally (settings per device), but the security chip key (decision 29) is tied to one machine, so a portable stick would use PIN plus recovery key. *Answer:* noted, parked as an idea for later; not needed now.

## 6. What it costs, and where it could go

| Piece | Size | When (if you choose the recommended path) |
| --- | --- | --- |
| **A. The processor wall**: move all x86 details behind one wall in the kernel | Small | Now, before milestone 5 |
| **B. Alaya on ARM in QEMU**: the engine (milestones 0 to 4 tests) starts and passes on ARM, kept passing in every milestone after | Medium, then about 10 to 15% extra per milestone | Right after A |
| **C. Device sense, basic**: firmware facts, the form guess, "This looks like a laptop" at setup, fake devices for QEMU tests | Small | Milestone 6 (with the setup question of decision 90) |
| **D. Device sense, full**: battery, lid, docking, power switching live | Medium | Milestone 11 in QEMU, finished at the first real boot (15) |
| **E. Touch and handheld layouts** | Medium | Touch with milestone 7b; handheld with the Play look in 14 |
| **F. Alaya on a Raspberry Pi** (its USB, network, pins, graphics) | Very large | New milestone 17, after the first real boot |
| **G. x86 games and apps on ARM** (borrowing a translator like FEX) | Very large | After F, if wanted |
| **H. One open phone** | Very large, per phone | Parked |
| **I. RISC-V** | Large | Parked |

Why now for A and B: today the kernel is 8,000 lines. By milestone 9 it may be several times bigger, with x86 details in many more places. Building the wall later is like adding a second staircase to a finished house.

## 7. Arduino and other microcontrollers: no, and why

An Arduino Uno has **2 kilobytes** of memory. Alaya's kernel alone needs thousands of times more, and Alaya's whole safety model (programs fenced off from each other) needs a piece of hardware called a memory manager that these chips simply don't have. Bigger ones (ESP32, Raspberry Pi Pico) are still far too small and also have no memory manager.

What *is* possible, later: Alaya **talking to** an Arduino plugged in over USB, as a device you grant to an app (like a game controller). That needs no Alaya on the Arduino. Your project rule says no microcontroller work for now, so this stays a parked idea.

## 8. The choices

Answered 20 November 2026: (1) groundwork now, ARM devices later (decision 93); (2) one open phone first (decision 94); (3) guess, confirm once, then adapt (decision 95).

1. **How far, and when?** Recommended: **build the wall and the ARM engine now; full ARM devices after the first real boot.** Alternatives: ARM fully alongside every milestone (everything slows by roughly a third), or PCs only until Alaya is everyday (much harder to add later).
2. **Which ARM device first?** Recommended: **Raspberry Pi** (cheap, well documented, starts from a stick, good for makers). Alternatives: one open phone (hardest), an ARM laptop (secret drivers, very hard).
3. **How does Alaya use its device guess?** Recommended: **it guesses, shows the guess once at setup with a "that's not right" button, then adapts by itself (dock, fold, unplug) with a one-line note the first time.** Alternatives: always ask before changing anything, or adapt silently.

## 9. What doesn't change

- The laptop stays the reference machine and the gaming target stays x86 (decision 3 becomes "x86-64 first").
- Testing stays QEMU only; nothing boots on real hardware until Alaya is stable and you agree (decision 75).
- History, grants, one undo, privacy and quiet driver crashes are the same on every processor and every device.
