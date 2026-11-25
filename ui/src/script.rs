// SPDX-License-Identifier: GPL-3.0-only WITH AdditionRef-Alaya-system-library-exception
//! Right-to-left text (milestone 7b, step 3): Arabic letters joined, and
//! a line put in the order it is seen, with Hebrew and Arabic running
//! from the right among left-to-right words and numbers.
//!
//! Joining uses Unicode's Arabic presentation forms (the fallback font
//! has them), so no shaping tables are needed. Ordering is a small
//! version of the Unicode bidirectional rules: strong letters set their
//! run's direction, numbers stay left to right, spaces and punctuation
//! take the direction around them, then runs are reversed by level.

use alloc::string::String;
use alloc::vec::Vec;

/// Whether `c` is written from the right (Hebrew, Arabic and their forms).
pub fn is_rtl(c: char) -> bool {
    matches!(c as u32, 0x0590..=0x08FF | 0xFB1D..=0xFDFF | 0xFE70..=0xFEFF)
}

/// Whether a line needs any of this (a fast way out for most text).
pub fn needs_work(s: &str) -> bool {
    s.chars().any(is_rtl)
}

/// Arabic letters 0x0621 to 0x064A: their isolated, final, initial and
/// medial presentation forms (0: the letter has no such form).
const FORMS: [[u16; 4]; 42] = [
    [0xFE80, 0, 0, 0],                // ء hamza
    [0xFE81, 0xFE82, 0, 0],           // آ
    [0xFE83, 0xFE84, 0, 0],           // أ
    [0xFE85, 0xFE86, 0, 0],           // ؤ
    [0xFE87, 0xFE88, 0, 0],           // إ
    [0xFE89, 0xFE8A, 0xFE8B, 0xFE8C], // ئ
    [0xFE8D, 0xFE8E, 0, 0],           // ا
    [0xFE8F, 0xFE90, 0xFE91, 0xFE92], // ب
    [0xFE93, 0xFE94, 0, 0],           // ة
    [0xFE95, 0xFE96, 0xFE97, 0xFE98], // ت
    [0xFE99, 0xFE9A, 0xFE9B, 0xFE9C], // ث
    [0xFE9D, 0xFE9E, 0xFE9F, 0xFEA0], // ج
    [0xFEA1, 0xFEA2, 0xFEA3, 0xFEA4], // ح
    [0xFEA5, 0xFEA6, 0xFEA7, 0xFEA8], // خ
    [0xFEA9, 0xFEAA, 0, 0],           // د
    [0xFEAB, 0xFEAC, 0, 0],           // ذ
    [0xFEAD, 0xFEAE, 0, 0],           // ر
    [0xFEAF, 0xFEB0, 0, 0],           // ز
    [0xFEB1, 0xFEB2, 0xFEB3, 0xFEB4], // س
    [0xFEB5, 0xFEB6, 0xFEB7, 0xFEB8], // ش
    [0xFEB9, 0xFEBA, 0xFEBB, 0xFEBC], // ص
    [0xFEBD, 0xFEBE, 0xFEBF, 0xFEC0], // ض
    [0xFEC1, 0xFEC2, 0xFEC3, 0xFEC4], // ط
    [0xFEC5, 0xFEC6, 0xFEC7, 0xFEC8], // ظ
    [0xFEC9, 0xFECA, 0xFECB, 0xFECC], // ع
    [0xFECD, 0xFECE, 0xFECF, 0xFED0], // غ
    [0, 0, 0, 0],                     // 063B to 063F: not used
    [0, 0, 0, 0],
    [0, 0, 0, 0],
    [0, 0, 0, 0],
    [0, 0, 0, 0],
    [0, 0, 0, 0],                     // 0640 tatweel: handled apart
    [0xFED1, 0xFED2, 0xFED3, 0xFED4], // ف
    [0xFED5, 0xFED6, 0xFED7, 0xFED8], // ق
    [0xFED9, 0xFEDA, 0xFEDB, 0xFEDC], // ك
    [0xFEDD, 0xFEDE, 0xFEDF, 0xFEE0], // ل
    [0xFEE1, 0xFEE2, 0xFEE3, 0xFEE4], // م
    [0xFEE5, 0xFEE6, 0xFEE7, 0xFEE8], // ن
    [0xFEE9, 0xFEEA, 0xFEEB, 0xFEEC], // ه
    [0xFEED, 0xFEEE, 0, 0],           // و
    [0xFEEF, 0xFEF0, 0, 0],           // ى
    [0xFEF1, 0xFEF2, 0xFEF3, 0xFEF4], // ي
];

const TATWEEL: char = '\u{0640}';

fn forms(c: char) -> Option<[u16; 4]> {
    let i = (c as u32).checked_sub(0x0621)? as usize;
    FORMS.get(i).copied().filter(|f| f[0] != 0)
}

/// Joins to the letter after it (it has an initial form, or is tatweel).
fn joins_on(c: char) -> bool {
    c == TATWEEL || forms(c).is_some_and(|f| f[2] != 0)
}

/// Joins to the letter before it.
fn joins_back(c: char) -> bool {
    c == TATWEEL || forms(c).is_some_and(|f| f[1] != 0)
}

/// Marks drawn on a letter (harakat): they do not break a join.
fn is_mark(c: char) -> bool {
    matches!(c as u32, 0x064B..=0x065F | 0x0670 | 0x06D6..=0x06ED)
}

/// Lam with alef: one ligature, isolated or final.
fn lam_alef(alef: char) -> Option<(u16, u16)> {
    Some(match alef {
        '\u{0622}' => (0xFEF5, 0xFEF6),
        '\u{0623}' => (0xFEF7, 0xFEF8),
        '\u{0625}' => (0xFEF9, 0xFEFA),
        '\u{0627}' => (0xFEFB, 0xFEFC),
        _ => return None,
    })
}

/// Arabic letters in their joined forms (still in reading order).
pub fn join_arabic(s: &str) -> String {
    let cs: Vec<char> = s.chars().collect();
    // The letter before and after each one, skipping marks.
    let prev = |i: usize| cs[..i].iter().rev().copied().find(|&c| !is_mark(c));
    let next = |i: usize| cs[i + 1..].iter().copied().find(|&c| !is_mark(c));
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < cs.len() {
        let c = cs[i];
        let Some(f) = forms(c) else {
            out.push(c);
            i += 1;
            continue;
        };
        let before = prev(i).is_some_and(joins_on) && f[1] != 0;
        // Lam and alef make one sign.
        if c == '\u{0644}'
            && let Some(j) = (i + 1..cs.len()).find(|&j| !is_mark(cs[j]))
            && let Some((iso, fin)) = lam_alef(cs[j])
        {
            out.push(char::from_u32(u32::from(if before { fin } else { iso })).unwrap_or(c));
            out.extend(cs[i + 1..j].iter());
            i = j + 1;
            continue;
        }
        let after = next(i).is_some_and(joins_back) && f[2] != 0;
        let form = match (before, after) {
            (true, true) => f[3],
            (true, false) => f[1],
            (false, true) => f[2],
            (false, false) => f[0],
        };
        out.push(char::from_u32(u32::from(form)).unwrap_or(c));
        i += 1;
    }
    out
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Class {
    Left,
    Right,
    Number,
    Neutral,
}

fn class(c: char) -> Class {
    if is_rtl(c) {
        Class::Right
    } else if c.is_ascii_digit() {
        Class::Number
    } else if c.is_alphabetic() || (c as u32) >= 0x2E80 {
        Class::Left
    } else {
        Class::Neutral
    }
}

/// A bracket's mirror, for brackets inside right-to-left runs.
fn mirror(c: char) -> char {
    match c {
        '(' => ')',
        ')' => '(',
        '[' => ']',
        ']' => '[',
        '{' => '}',
        '}' => '{',
        '<' => '>',
        '>' => '<',
        '«' => '»',
        '»' => '«',
        c => c,
    }
}

/// The line as it is seen, left to right: Arabic joined, right-to-left
/// runs turned round, numbers and left-to-right words kept as they are.
/// The paragraph goes the way of its first strong letter.
pub fn visual(s: &str) -> String {
    if !needs_work(s) {
        return String::from(s);
    }
    let cs: Vec<char> = join_arabic(s).chars().collect();
    let classes: Vec<Class> = cs.iter().map(|&c| class(c)).collect();
    let rtl_par = classes
        .iter()
        .find(|c| matches!(c, Class::Left | Class::Right))
        == Some(&Class::Right);
    let base: u8 = rtl_par.into();
    // Levels: right-to-left letters odd, the others even, above the base.
    let strong_level = |c: Class| -> Option<u8> {
        match c {
            Class::Right => Some(1),
            Class::Left => Some(if rtl_par { 2 } else { 0 }),
            // Numbers read left to right, also among Arabic or Hebrew.
            Class::Number => Some(if rtl_par { 2 } else { 0 }),
            Class::Neutral => None,
        }
    };
    let mut levels: Vec<u8> = classes
        .iter()
        .map(|&c| strong_level(c).unwrap_or(255))
        .collect();
    // Numbers after right-to-left letters, in a left-to-right line, sit
    // in that run's place but still read left to right.
    if !rtl_par {
        for i in 0..cs.len() {
            if classes[i] == Class::Number {
                let before = classes[..i]
                    .iter()
                    .rev()
                    .find(|c| matches!(c, Class::Left | Class::Right));
                if before == Some(&Class::Right) {
                    levels[i] = 2;
                }
            }
        }
    }
    // Neutrals: the direction on both sides when it is the same, else
    // the paragraph's.
    let dir = |l: u8| l % 2;
    for i in 0..cs.len() {
        if levels[i] != 255 {
            continue;
        }
        let before = levels[..i].iter().rev().find(|&&l| l != 255).copied();
        let after = levels[i + 1..].iter().find(|&&l| l != 255).copied();
        levels[i] = match (before, after) {
            (Some(a), Some(b)) if dir(a) == dir(b) => a.min(b),
            _ => base,
        };
    }
    // Reverse every run at each level, from the highest down to 1.
    let mut order: Vec<usize> = (0..cs.len()).collect();
    let top = levels.iter().copied().max().unwrap_or(0);
    for level in (1..=top).rev() {
        let mut i = 0;
        while i < order.len() {
            if levels[order[i]] >= level {
                let start = i;
                while i < order.len() && levels[order[i]] >= level {
                    i += 1;
                }
                order[start..i].reverse();
            } else {
                i += 1;
            }
        }
    }
    order
        .iter()
        .map(|&i| {
            if levels[i] % 2 == 1 {
                mirror(cs[i])
            } else {
                cs[i]
            }
        })
        .collect()
}
