// SPDX-License-Identifier: GPL-3.0-only
//! "Show the difference" (living validity, milestone 7): what changed
//! between two versions of a text, line by line, simply.

use alloc::vec::Vec;

/// A line of a difference: kept, taken out or put in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Line<'a> {
    Same(&'a [u8]),
    Out(&'a [u8]),
    In(&'a [u8]),
}

/// The lines that differ, simply: what both share at the start and the
/// end stays, the middle is shown as taken out, then put in. At most
/// `max` lines.
pub fn diff_lines<'a>(then: &'a [u8], now: &'a [u8], max: usize) -> Vec<Line<'a>> {
    if then == now {
        return Vec::new();
    }
    let a: Vec<&[u8]> = then.split(|&b| b == b'\n').collect();
    let b: Vec<&[u8]> = now.split(|&b| b == b'\n').collect();
    let start = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let end = a[start..]
        .iter()
        .rev()
        .zip(b[start..].iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    let mut out = Vec::new();
    // One line of what stays around the change.
    if start > 0 {
        out.push(Line::Same(a[start - 1]));
    }
    out.extend(a[start..a.len() - end].iter().map(|l| Line::Out(l)));
    out.extend(b[start..b.len() - end].iter().map(|l| Line::In(l)));
    if end > 0 {
        out.push(Line::Same(a[a.len() - end]));
    }
    out.truncate(max);
    out
}

#[cfg(test)]
mod t {
    use super::*;

    #[test]
    fn shows_the_lines_that_changed() {
        assert_eq!(
            diff_lines(
                b"apples 3\npears 4\nplums 5\n",
                b"apples 3\npears 6\nplums 5\n",
                10
            ),
            alloc::vec![
                Line::Same(b"apples 3"),
                Line::Out(b"pears 4"),
                Line::In(b"pears 6"),
                Line::Same(b"plums 5"),
            ]
        );
        assert_eq!(
            diff_lines(b"a\nb", b"a\nb\nc", 10),
            alloc::vec![Line::Same(b"b"), Line::In(b"c")]
        );
        assert!(diff_lines(b"same", b"same", 10).is_empty());
        assert_eq!(diff_lines(b"x", b"y", 1), alloc::vec![Line::Out(b"x")]);
    }
}
