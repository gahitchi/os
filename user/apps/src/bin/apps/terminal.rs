// SPDX-License-Identifier: GPL-3.0-only
//! The terminal (milestone 8, step 4): a command line whose tools are
//! small C programs built on Alaya's own C library (`user/libc`), run
//! inside this program like a toolbox.
//!
//! It sees only what it is handed, like any app (decision 71): "pick"
//! asks Alaya's Open window for a document, and from then on tools can
//! use it by name. In a protected task it sees the task's files. Each
//! command and what it printed is also kept in its own file
//! `terminal.log`, so you (and the checks) can read it back.
//!
//! Tools: hello, echo, cat, wc, ls, cp, selftest. Built in: help, pick,
//! clear.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::RefCell;

use alaya_apps::libc::OUT;
use alaya_apps::store::Store;
use alaya_apps::{self as app, Event, Window};
use alaya_programs::files::{APPS, DOCS};
use alaya_sys::abi::StartInfo;
use alaya_sys::rt::Mem;
use alaya_sys::say;
use alaya_ui::Rect;
use alaya_ui::text::Face;
use alaya_ui::widgets::{Key, Ui, theme};

type Main = unsafe extern "C" fn(i32, *mut *mut u8) -> i32;

unsafe extern "C" {
    fn alaya_run_main(main: Main, argc: i32, argv: *mut *mut u8) -> i32;
    fn tool_hello(argc: i32, argv: *mut *mut u8) -> i32;
    fn tool_echo(argc: i32, argv: *mut *mut u8) -> i32;
    fn tool_cat(argc: i32, argv: *mut *mut u8) -> i32;
    fn tool_wc(argc: i32, argv: *mut *mut u8) -> i32;
    fn tool_ls(argc: i32, argv: *mut *mut u8) -> i32;
    fn tool_cp(argc: i32, argv: *mut *mut u8) -> i32;
    fn tool_selftest(argc: i32, argv: *mut *mut u8) -> i32;
}

const TOOLS: [(&str, Main); 7] = [
    ("hello", tool_hello),
    ("echo", tool_echo),
    ("cat", tool_cat),
    ("wc", tool_wc),
    ("ls", tool_ls),
    ("cp", tool_cp),
    ("selftest", tool_selftest),
];

const LOG: &str = "terminal.log";
const KEEP: usize = 300;

/// What the C program printed while it ran.
struct Printed(RefCell<Vec<u8>>);
// SAFETY: one thread.
unsafe impl Sync for Printed {}
static PRINTED: Printed = Printed(RefCell::new(Vec::new()));

fn sink(_fd: i32, b: &[u8]) {
    let mut p = PRINTED.0.borrow_mut();
    if p.len() < 256 * 1024 {
        p.extend_from_slice(b);
    }
}

/// Run a C tool with these words as its arguments: its exit status and
/// what it printed.
fn run_tool(main: Main, words: &[&str]) -> (i32, String) {
    // Each argument as its own zero-ended bytes, and the pointer array
    // (ending in null, as C expects).
    let mut bufs: Vec<Vec<u8>> = words
        .iter()
        .map(|w| {
            let mut b = w.as_bytes().to_vec();
            b.push(0);
            b
        })
        .collect();
    let mut argv: Vec<*mut u8> = bufs.iter_mut().map(|b| b.as_mut_ptr()).collect();
    argv.push(core::ptr::null_mut());
    PRINTED.0.borrow_mut().clear();
    // SAFETY: argv holds argc valid C strings and a null; the tools are
    // C code built against Alaya's C library.
    let code = unsafe { alaya_run_main(main, words.len() as i32, argv.as_mut_ptr()) };
    let out = String::from_utf8_lossy(&PRINTED.0.borrow()).into_owned();
    (code, out)
}

pub fn run(info: &'static StartInfo, mut mem: Mem, store: Store) -> ! {
    OUT.set(sink);
    let mut fonts = app::fonts(info);
    let mut win = Window::open(&mut mem, 640, 400, "Terminal").expect("open a window");
    let log = store.root(APPS).and_then(|a| store.open(a, LOG)).ok();
    let mut lines: Vec<String> = alloc::vec![
        String::from("Alaya terminal. Tools: hello echo cat wc ls cp selftest."),
        String::from("It sees only what you hand it: type \"pick\" to choose a document."),
    ];
    let mut input = String::new();
    loop {
        let mut run_line: Option<String> = None;
        while let Some(e) = win.next_event() {
            match e {
                Event::Key(Key::Char(c)) if input.len() < 200 => input.push(c),
                Event::Key(Key::Backspace) => {
                    input.pop();
                }
                Event::Key(Key::Enter) => run_line = Some(core::mem::take(&mut input)),
                Event::Picked { file: Some(f), .. } => {
                    let name = store
                        .root(DOCS)
                        .ok()
                        .and_then(|d| store.list(d).ok())
                        .and_then(|l| l.into_iter().find(|e| e.file == f))
                        .map_or(String::from("a document"), |e| e.name);
                    lines.push(format!("{name} was handed to the terminal."));
                    say!("terminal: {name} handed over");
                }
                Event::Picked { file: None, .. } => {
                    lines.push(String::from("Nothing was handed over."))
                }
                Event::Close => crate::closed(&win),
                _ => {}
            }
        }
        if let Some(cmd) = run_line {
            let words: Vec<&str> = cmd.split_whitespace().collect();
            lines.push(format!("$ {cmd}"));
            let mut out = String::new();
            let mut code = 0;
            match words.first().copied() {
                None => {}
                Some("help") => {
                    out = String::from(
                        "hello, echo WORDS, cat FILES, wc FILES, ls, cp FROM TO, selftest; pick, clear\n",
                    )
                }
                Some("clear") => lines.clear(),
                Some("pick") => {
                    out = String::from(if win.pick(false, false) {
                        "Pick a document in Alaya's Open window.\n"
                    } else {
                        "Alaya did not show the Open window.\n"
                    })
                }
                Some(name) => match TOOLS.iter().find(|t| t.0 == name) {
                    Some(&(_, main)) => (code, out) = run_tool(main, &words),
                    None => {
                        code = 127;
                        out = format!("{name}: no such tool (try \"help\")\n");
                    }
                },
            }
            for l in out.lines() {
                say!("terminal: {l}");
                lines.push(String::from(l));
            }
            if code != 0 {
                lines.push(format!("[exit {code}]"));
            }
            say!("terminal: {cmd} -> exit {code}");
            // Keep the record: the command, what it printed, its status.
            if let Some(f) = log {
                let mut text = store.read_all(f).unwrap_or_default();
                text.extend_from_slice(format!("$ {cmd}\n{out}[exit {code}]\n").as_bytes());
                let keep = text.len().saturating_sub(64 * 1024);
                let _ = store.write_all(f, &text[keep..]);
                let _ = store.save();
            }
            if lines.len() > KEEP {
                lines.drain(..lines.len() - KEEP);
            }
        }
        {
            let mut cv = win.canvas();
            let (w, h) = (win.width, win.height);
            cv.fill(Rect::new(0, 0, w, h), theme::INK);
            let mut ui = Ui::new(&mut cv, &mut fonts, win.pointer);
            let lh = 17;
            let rows = ((h - 40) / lh).max(1) as usize;
            let first = lines.len().saturating_sub(rows);
            for (i, l) in lines[first..].iter().enumerate() {
                ui.text(10, 8 + i as i32 * lh, Face::Mono, 13, l, theme::WIN);
            }
            ui.text(
                10,
                h - 26,
                Face::Mono,
                13,
                &format!("$ {input}_"),
                theme::WIN,
            );
            ui.finish();
            win.tell(&ui.tree);
            win.pointer.released = false;
        }
        win.damage_all();
        win.wait();
    }
}
