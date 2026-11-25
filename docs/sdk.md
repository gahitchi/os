# Making apps for Alaya: the SDK

Milestone 8, step 3. An app for Alaya is a Rust program built on three
libraries: `alaya-sys` (the runtime: memory, threads, messages, the clock),
`alaya-ui` (drawing, text, widgets, accessibility) and `alaya-apps` (the
SDK proper: windows, events and the store). Postcards
(`user/apps/src/bin/postcards.rs`) is a whole example in under 250 lines.
It is packed into a signed bundle by `cargo xtask` (see [apps.md](apps.md)).

## The calls

| What | Rust (`alaya_apps`) | What Alaya does |
| --- | --- | --- |
| A window | `Window::open`, `next_event`, `canvas`, `damage_all`, `wait` | The desktop draws it, with Alaya's frame |
| Open or save a document | `Window::pick(save, write)` → `Event::Picked` | Alaya's own Open and Save windows; the app gets only what the person picks |
| An answer instead of the document | `Window::ask_answer(question)` → `Event::Picked`, then `Store::answer(file)` | The person picks the document; the app gets one number, never the file |
| Hand a document to another app | `Window::hand_off(to, file, line)` → the other app gets `Event::HandedOff` | Alaya asks the person first, then the other app goes on at the same line |
| Tell the person something | `Window::notify(kind, text)` | The decision inbox: the app's "urgent" is only a wish |
| Keep running with no window | `Window::background()` | Only with the promise line ticked; frozen during a game |
| Files | `Store::open`, `read_all`, `write_all`, `save`, `made_from` | Only the app's own files and what it was handed |
| A job with rules | `Window::ask_job` | The person sees the rules first; the work happens on a side copy |

## Live handoff

The text editor's **Ctrl+H** hands the document it has open to the chart
maker, at the line the cursor is on. Alaya asks *"Continue numbers.txt in
Chart maker?"*: **Continue there** hands it over (until the chart maker
closes, unless it already had it for good) and the chart maker charts that
document with the bar of that line marked; **Stay here** hands over nothing.
An app can only hand over a document it holds itself, and only to an app
that may use files. The editor also goes on at the right line when a
document is handed to it.

## The C interface

`user/apps/include/alaya.h` lists a small set of C functions, implemented in
`user/apps/src/c.rs`: the clock, a log line, the store's whole-file calls
and answers. Every function returns a status (0 is OK) and writes results
through pointers. They go through the same endpoints as Rust apps, so a C
program can do nothing a Rust app cannot. `cargo xtask image` refuses to
build when the header and the Rust side list different functions.

The same header also has `alaya_store_lookup`, `alaya_store_remove` and
`alaya_store_list` (names in a folder, one per line, folders ending in `/`).

## Alaya's own C library

`user/libc` is a small C library of Alaya's own, built with clang by
`user/apps/build.rs` and linked into the apps program (step 8-4; decision
160). It has what most small C programs need:

- `string.h`, `ctype.h`, `stdlib.h` (numbers from text, `qsort`, `bsearch`,
  `rand`, `malloc` and friends on the app's own heap), `errno.h`,
  `setjmp.h`, `assert.h`, `time.h` (`clock`, `time`, `clock_gettime`);
- `stdio.h`: `printf` and its family with flags, width and precision,
  `FILE` streams, `fopen`, `fgets`, `fread`, `fwrite`, `fclose`;
- `unistd.h` and `fcntl.h`: `open`, `read`, `write`, `lseek`, `close`.

Files are whole documents, read when opened and written back when closed:
`name` or `/docs/name` is one of the documents the app may use, and
`/app/name` is one of the app's own files. Anything else is "not found". A
C program can reach exactly what its app may reach, nothing more. What a
program prints goes to the app (the terminal shows it); `exit` and `abort`
end the program, not the app.

### The terminal

The terminal (in the Build look's dock) runs small C tools on that library:
`hello`, `echo`, `cat`, `wc`, `ls`, `cp` and `selftest`, which checks the
library itself. Like a terminal anywhere, it sees all of your documents
(decision 40), never another person's, and reading a private file still
counts. Inside a protected task it sees only the task's files; `all` asks
you to open everything to it until it closes ("Open all" or "Only the
task"), and one undo closes them again (decision 71). `pick` hands it one
document through the Open window; `help` lists everything, `clear` empties
the screen. Every command, what it printed and how it ended (`[exit N]`) is
kept in the terminal's own file `terminal.log`. Boot check 125 types into
it.

## Windows from C and the SDL subset

`alaya.h` also opens one window per C program (`alaya_window_open`), shows
a picture the program draws itself (`alaya_window_present`, 0x00RRGGBB
pixels, scaled up by whole steps) and gives its events
(`alaya_window_event`: keys, closing, size, focus, clicks). On top of it,
`SDL.h` in Alaya's C library is a small part of the SDL 2 interface written
from scratch: `SDL_CreateWindow`, `SDL_GetWindowSurface`,
`SDL_UpdateWindowSurface`, `SDL_FillRect`, `SDL_MapRGB`, `SDL_PollEvent`,
`SDL_WaitEvent`, `SDL_GetTicks` and `SDL_Delay`, with SDL 2's key values.
Drawing is in software until fast graphics (milestone 9). Full screen is the
person's choice (Alaya key + Up), so `SDL_SetWindowFullscreen` says no.

## Saved games

A game asks for its folder with `alaya_saved_games("Name")`: Alaya makes
`Saved games/Name` in the person's documents the first time, and it is the
game's own (decision 161). Saves are documents, so every save stays in the
person's history: `alaya_save_versions` lists each moment a save changed,
newest first, and `alaya_read_past` reads a save as it was at one of them.
Bringing an old save back and saving again keeps the newer ones.

### Glowroot

The first game (`user/libc/games/glowroot.c`), in the Play and Everything
looks: a firefly lights a dark garden, one tile at a time, and may only fly
to a tile that is still dark. Each garden grows from a random walk, so
there is always a way. Arrows fly, U steps back, R starts again, S saves,
H shows every save (Enter brings one back). Boot check 126 plays it.
