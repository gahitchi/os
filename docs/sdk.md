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

The first C programs, the POSIX layer on musl and SDL come in step 8-4, and
build on this.
