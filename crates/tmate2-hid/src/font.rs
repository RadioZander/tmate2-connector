//! Seven-segment font, in the usual segment naming:
//!
//! ```text
//!    aaa
//!   f   b
//!    ggg
//!   e   c
//!    ddd
//! ```
//!
//! Each glyph is a bitmask with bit 0 = a, ... bit 6 = g.

pub const SEGMENTS: [char; 7] = ['a', 'b', 'c', 'd', 'e', 'f', 'g'];

const A: u8 = 1 << 0;
const B: u8 = 1 << 1;
const C: u8 = 1 << 2;
const D: u8 = 1 << 3;
const E: u8 = 1 << 4;
const F: u8 = 1 << 5;
const G: u8 = 1 << 6;

/// Glyph for `ch`, or `None` if it can't be shown on seven segments.
/// Letters are case-insensitive; each uses whichever case's shape reads best
/// on seven segments (b, d, n, r, t are lower-case shapes; X and K look like H).
/// The exception is `o`: lower case is the small o in the lower half, so it
/// can't be read as a zero; `O` is the full-height shape.
pub fn glyph(ch: char) -> Option<u8> {
    if ch == 'o' {
        return Some(C | D | E | G);
    }
    Some(match ch.to_ascii_uppercase() {
        ' ' => 0,
        '0' | 'O' => A | B | C | D | E | F,
        '1' | 'I' => B | C,
        '2' | 'Z' => A | B | D | E | G,
        '3' => A | B | C | D | G,
        '4' => B | C | F | G,
        '5' | 'S' => A | C | D | F | G,
        '6' => A | C | D | E | F | G,
        '7' => A | B | C,
        '8' => A | B | C | D | E | F | G,
        '9' => A | B | C | D | F | G,
        'B' => C | D | E | F | G,
        'D' => B | C | D | E | G,
        'G' => A | C | D | E | F,
        'A' => A | B | C | E | F | G,
        'C' => A | D | E | F,
        'E' => A | D | E | F | G,
        'F' => A | E | F | G,
        'H' | 'X' | 'K' => B | C | E | F | G,
        'J' => B | C | D | E,
        'L' => D | E | F,
        'N' => C | E | G,
        'P' => A | B | E | F | G,
        'Q' => A | B | C | F | G,
        'R' => E | G,
        'T' => D | E | F | G,
        'U' | 'V' | 'W' => B | C | D | E | F,
        'Y' => B | C | D | F | G,
        '-' => G,
        '_' => D,
        '=' => D | G,
        '\'' => B,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digits_have_expected_segment_counts() {
        let counts: Vec<u32> = "0123456789"
            .chars()
            .map(|c| glyph(c).unwrap().count_ones())
            .collect();
        assert_eq!(counts, vec![6, 2, 5, 5, 4, 5, 6, 3, 7, 6]);
        assert_eq!(glyph('~'), None);
    }

    #[test]
    fn small_o_is_not_a_zero() {
        assert_eq!(glyph('o'), Some(C | D | E | G));
        assert_eq!(glyph('O'), glyph('0'));
        assert_ne!(glyph('o'), glyph('0'));
    }
}
