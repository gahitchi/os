// SPDX-License-Identifier: GPL-3.0-only
//! The chart maker (a demo app): a bar chart of the numbers in
//! `numbers.txt` (one "label number" per line), drawn again when the file
//! changes, and saved as a picture (`chart.ppm`) and a short summary
//! (`summary.txt`) when you ask.
//!
//! It is an Alaya app that tells its sources (living validity, milestone
//! 6c): when it saves, it tells the store exactly which version of
//! `numbers.txt` both files were made from. The store checks that the
//! chart maker read all of that version, and the file browser marks the
//! picture and the summary "Needs a look" once the numbers change.

use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use alaya_apps::store::{Store, Version};
use alaya_apps::{self as app, Event, Window};
use alaya_programs::files::DOCS;
use alaya_sys::abi::StartInfo;
use alaya_sys::rt::{self, Mem};
use alaya_sys::say;
use alaya_ui::Rect;
use alaya_ui::canvas::Canvas;
use alaya_ui::text::{Face, Fonts};
use alaya_ui::widgets::{Ui, theme};

const NUMBERS: &str = "numbers.txt";
const PICTURE: &str = "chart.ppm";
const SUMMARY: &str = "summary.txt";
const SAMPLE: &str = "Mon 3\nTue 5\nWed 2\nThu 6\nFri 4\n";
const PIC_W: i32 = 480;
const PIC_H: i32 = 300;

/// "label number" lines; lines without a number are skipped.
fn parse(text: &str) -> Vec<(String, u32)> {
    text.lines()
        .filter_map(|l| {
            let mut parts = l.split_whitespace();
            let label = parts.next()?;
            let n = parts.next()?.parse::<u32>().ok()?;
            Some((String::from(label), n.min(1_000_000)))
        })
        .take(24)
        .collect()
}

fn draw_chart(
    cv: &mut Canvas,
    fonts: &mut Fonts,
    r: Rect,
    bars: &[(String, u32)],
    focus: Option<usize>,
) {
    cv.fill(r, theme::WIN);
    let top = bars.iter().map(|b| b.1).max().unwrap_or(0).max(1);
    let base = r.bottom() - 28;
    cv.fill(Rect::new(r.x + 8, base, r.w - 16, 1), theme::LINE);
    if bars.is_empty() {
        let (asc, _) = fonts.metrics(Face::Text, 13);
        fonts.draw(
            cv,
            Face::Text,
            13,
            r.x + 12,
            r.y + 12 + asc,
            "No numbers yet.",
            theme::MUTED,
        );
        return;
    }
    let n = bars.len() as i32;
    let slot = (r.w - 16) / n;
    let bw = (slot * 3 / 5).max(4);
    let (asc, _) = fonts.metrics(Face::Text, 12);
    for (i, (label, v)) in bars.iter().enumerate() {
        let x = r.x + 8 + i as i32 * slot + (slot - bw) / 2;
        let bh = ((base - r.y - 24) as i64 * i64::from(*v) / i64::from(top)) as i32;
        let color = if focus == Some(i) {
            theme::INK
        } else {
            theme::ACCENT
        };
        cv.round_rect(Rect::new(x, base - bh, bw, bh + 4), 4, color);
        cv.fill(Rect::new(x, base, bw, 1), theme::LINE);
        let t = format!("{v}");
        let tw = fonts.width(Face::Strong, 12, &t);
        fonts.draw(
            cv,
            Face::Strong,
            12,
            x + (bw - tw) / 2,
            base - bh - 6,
            &t,
            theme::INK,
        );
        let lw = fonts.width(Face::Text, 12, label);
        fonts.draw(
            cv,
            Face::Text,
            12,
            x + (bw - lw) / 2,
            base + 6 + asc,
            label,
            theme::MUTED,
        );
    }
}

/// The numbers in words.
fn summary(bars: &[(String, u32)]) -> String {
    let total: u64 = bars.iter().map(|b| u64::from(b.1)).sum();
    match bars.iter().max_by_key(|b| b.1) {
        Some((label, top)) => format!(
            "{} numbers from {NUMBERS}, {total} in all. The highest is {label} with {top}.\n",
            bars.len()
        ),
        None => format!("There are no numbers in {NUMBERS} yet.\n"),
    }
}

/// The chart as a picture file (binary PPM: simple, and any viewer reads
/// it).
fn picture(fonts: &mut Fonts, bars: &[(String, u32)]) -> Vec<u8> {
    let mut px = vec![0u32; (PIC_W * PIC_H) as usize];
    {
        let mut cv = Canvas::new(&mut px, PIC_W, PIC_H, PIC_W).expect("picture");
        draw_chart(&mut cv, fonts, Rect::new(0, 0, PIC_W, PIC_H), bars, None);
    }
    let mut out = format!("P6\n{PIC_W} {PIC_H}\n255\n").into_bytes();
    out.reserve(px.len() * 3);
    for p in px {
        out.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, p as u8]);
    }
    out
}

pub fn run(info: &'static StartInfo, mut mem: Mem, store: Store) -> ! {
    let mut fonts = app::fonts(info);
    let docs = store.root(DOCS).expect("documents");
    if store.lookup(docs, NUMBERS).is_err() {
        // Something to start with.
        if let Ok(f) = store.open(docs, NUMBERS) {
            let _ = store.write_all(f, SAMPLE.as_bytes());
        }
    }
    let mut win = Window::open(&mut mem, 460, 340, "Chart maker").expect("open a window");
    let mut bars = Vec::new();
    let mut seen: Option<Version> = None;
    let mut last_look = 0;
    let mut note = String::new();
    // A document handed over (live handoff), and the line it was at.
    let mut source: Option<u64> = None;
    let mut at_line: Option<usize> = None;
    let mut focus: Option<usize> = None;
    loop {
        let mut keys = Vec::new();
        let mut save = false;
        while let Some(e) = win.next_event() {
            match e {
                Event::Key(k) => keys.push(k),
                Event::Shortcut('s') => save = true,
                // Make again (living validity): look at the numbers as
                // they are now, then save the picture and summary again.
                Event::MakeAgain(_) => {
                    save = true;
                    last_look = 0;
                    say!("asked to make it again");
                }
                // Live handoff: chart the document handed over, with the
                // line the other app was at marked.
                Event::HandedOff { file, place, from } => {
                    source = Some(file);
                    last_look = 0;
                    seen = None;
                    at_line = Some(place as usize);
                    say!(
                        "continues a document from {} at line {}",
                        alaya_apps::store::app_name(alaya_programs::files::app_id(from)),
                        place + 1
                    );
                }
                Event::Close => crate::closed(&win),
                _ => {}
            }
        }
        let now = rt::now_ms();
        if last_look == 0 || now - last_look >= 2000 {
            last_look = now;
            if let Ok(f) = source.map_or_else(|| store.lookup(docs, NUMBERS), Ok)
                && let Ok(v) = store.version(f)
                && Some(v) != seen
                && let Ok((b, v)) = store.read_versioned(f)
            {
                seen = Some(v);
                let text = String::from_utf8_lossy(&b);
                bars = parse(&text);
                // The bar of the line handed over: the numbers before it.
                focus = at_line.map(|l| {
                    let before: String = text.lines().take(l).flat_map(|x| [x, "\n"]).collect();
                    parse(&before).len()
                });
                if let Some(i) = focus {
                    say!("marks bar {} of {}", i + 1, bars.len());
                }
                say!(
                    "drew {} bars from {} version {}",
                    bars.len(),
                    NUMBERS,
                    v.version
                );
            }
        }
        {
            let mut cv = win.canvas();
            let (w, h) = (win.width, win.height);
            cv.fill(Rect::new(0, 0, w, h), theme::WIN);
            let mut ui = Ui::new(&mut cv, &mut fonts, win.pointer);
            ui.text(14, 12, Face::Strong, 14, "From numbers.txt", theme::INK);
            ui.key = keys.first().copied();
            let (pressed, _) = ui.button(w - 150, 8, "Save picture", true);
            save |= pressed;
            ui.text(14, h - 28, Face::Text, 12, &note, theme::MUTED);
            ui.finish();
            win.tell(&ui.tree);
            win.pointer.released = false;
            draw_chart(
                &mut cv,
                &mut fonts,
                Rect::new(12, 48, w - 24, h - 86),
                &bars,
                focus,
            );
        }
        if save && let Some(src) = seen {
            let data = picture(&mut fonts, &bars);
            let text = summary(&bars);
            // Each result, then what it was made from: exactly the version
            // drawn.
            let put = |name: &str, bytes: &[u8]| {
                store
                    .open(docs, name)
                    .and_then(|f| store.write_all(f, bytes).map(|_| f))
                    .and_then(|f| store.made_from(f, &[src]))
            };
            let ok = put(PICTURE, &data)
                .and_then(|_| put(SUMMARY, text.as_bytes()))
                .and_then(|_| store.save())
                .is_ok();
            note = if ok {
                say!(
                    "saved {} ({} bytes) and {} from {} version {}",
                    PICTURE,
                    data.len(),
                    SUMMARY,
                    NUMBERS,
                    src.version
                );
                format!("Saved {PICTURE} and {SUMMARY}, made from {NUMBERS}.")
            } else {
                String::from("Could not save the picture.")
            };
        }
        win.damage_all();
        win.wait();
    }
}
