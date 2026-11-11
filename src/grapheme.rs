//! Extended grapheme boundaries, Unicode 18 UAX #29 rules GB3–GB13.
use crate::{grapheme_data as d, unicode::contains};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Class {
    Other,
    Cr,
    Lf,
    Control,
    Extend,
    Zwj,
    Spacing,
    Prepend,
    Ri,
    L,
    V,
    T,
    Lv,
    Lvt,
}

fn class(c: char) -> Class {
    use Class::*;
    let cp = c as u32;
    if c == '\r' {
        return Cr;
    }
    if c == '\n' {
        return Lf;
    }
    if cp < 0x20 || cp == 0x7f {
        return Control;
    }
    if cp < 0x80 {
        return Other;
    }
    if c == '\u{200d}' {
        return Zwj;
    }
    if (0x1f1e6..=0x1f1ff).contains(&cp) {
        return Ri;
    }
    if (0xac00..=0xd7a3).contains(&cp) {
        return if (cp - 0xac00).is_multiple_of(28) {
            Lv
        } else {
            Lvt
        };
    }
    if contains(d::EXTEND, c) {
        Extend
    } else if contains(d::SPACINGMARK, c) {
        Spacing
    } else if contains(d::PREPEND, c) {
        Prepend
    } else if contains(d::CONTROL, c) {
        Control
    } else if contains(d::L, c) {
        L
    } else if contains(d::V, c) {
        V
    } else if contains(d::T, c) {
        T
    } else {
        Other
    }
}

/// Byte length of the first extended grapheme; zero for empty text.
#[inline]
pub(crate) fn first_len(text: &str) -> usize {
    let bytes = text.as_bytes();
    match bytes {
        [] => return 0,
        [one] if one.is_ascii() => return 1,
        [first, next, ..] if first.is_ascii() && next.is_ascii() => {
            return if *first == b'\r' && *next == b'\n' {
                2
            } else {
                1
            };
        }
        _ => {}
    }
    first_unicode_len(text)
}

fn first_unicode_len(text: &str) -> usize {
    use Class::*;
    let mut chars = text.char_indices();
    let Some((_, first)) = chars.next() else {
        return 0;
    };
    if text.len() == first.len_utf8() {
        return text.len();
    }
    if text.as_bytes().get(1).is_some_and(u8::is_ascii) && first.is_ascii() {
        return if text.as_bytes().starts_with(b"\r\n") {
            2
        } else {
            1
        };
    }
    let mut prev = class(first);
    let mut ri = usize::from(prev == Ri);
    let mut pictographic_chain = contains(d::EXTENDED_PICTOGRAPHIC, first);
    let mut zwj_after_picture = false;
    let mut conjunct = contains(d::INCB_LINKER, first);
    for (at, c) in chars {
        let current = class(c);
        let consonant = contains(d::INCB_CONSONANT, c);
        let picture = contains(d::EXTENDED_PICTOGRAPHIC, c);
        let joins = if prev == Cr && current == Lf {
            true
        } else if matches!(prev, Cr | Lf | Control) || matches!(current, Cr | Lf | Control) {
            false
        } else {
            (prev == L && matches!(current, L | V | Lv | Lvt))
                || (matches!(prev, Lv | V) && matches!(current, V | T))
                || (matches!(prev, Lvt | T) && current == T)
                || matches!(current, Extend | Zwj | Spacing)
                || prev == Prepend
                || (conjunct && consonant)
                || (prev == Zwj && zwj_after_picture && picture)
                || (prev == Ri && current == Ri && ri % 2 == 1)
        };
        if !joins {
            return at;
        }
        if current == Ri {
            ri += 1;
        } else {
            ri = 0;
        }
        zwj_after_picture = current == Zwj && pictographic_chain;
        pictographic_chain = picture || (current == Extend && pictographic_chain);
        conjunct = contains(d::INCB_LINKER, c) || (conjunct && contains(d::INCB_EXTEND, c));
        prev = current;
    }
    text.len()
}
