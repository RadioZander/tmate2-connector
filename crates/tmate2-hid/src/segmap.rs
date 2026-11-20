//! LCD segment map: human-readable names to output-report bit positions.

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::font;
use crate::output::{OutputReport, SEGMENT_END, SEGMENT_FIRST};
use crate::{Error, Result};

/// Provisional built-in map (see the comments in the file).
pub const DEFAULT_TOML: &str = include_str!("../segmap.default.toml");

/// Position of one segment: byte index in the output report and bit 0..7.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SegAddr {
    pub byte: u8,
    pub bit: u8,
}

impl fmt::Display for SegAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.byte, self.bit)
    }
}

impl FromStr for SegAddr {
    type Err = String;
    fn from_str(s: &str) -> std::result::Result<Self, String> {
        let (byte, bit) = s
            .split_once('.')
            .ok_or_else(|| format!("'{s}' is not \"byte.bit\""))?;
        let byte: u8 = byte.trim().parse().map_err(|_| format!("bad byte in '{s}'"))?;
        let bit: u8 = bit.trim().parse().map_err(|_| format!("bad bit in '{s}'"))?;
        if !(SEGMENT_FIRST..SEGMENT_END).contains(&(byte as usize)) || bit > 7 {
            return Err(format!(
                "'{s}' is outside the segment area (bytes {SEGMENT_FIRST}..{}, bits 0..7)",
                SEGMENT_END - 1
            ));
        }
        Ok(SegAddr { byte, bit })
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct SegFile {
    #[serde(default)]
    segments: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default)]
pub struct SegmentMap {
    map: BTreeMap<String, SegAddr>,
}

impl SegmentMap {
    pub fn builtin() -> Self {
        Self::from_toml(DEFAULT_TOML).expect("built-in segment map is valid")
    }

    pub fn from_toml(text: &str) -> Result<Self> {
        let f: SegFile = toml::from_str(text).map_err(|e| Error::SegMap(e.to_string()))?;
        let mut map = BTreeMap::new();
        for (name, addr) in f.segments {
            let a = addr.parse().map_err(|e| Error::SegMap(format!("{name}: {e}")))?;
            map.insert(name, a);
        }
        Ok(SegmentMap { map })
    }

    /// Serialise as a `[segments]` table, sorted by name.
    pub fn to_toml(&self) -> String {
        let mut s = String::from("[segments]\n");
        for (name, a) in &self.map {
            s.push_str(&format!("{:?} = \"{a}\"\n", name));
        }
        s
    }

    /// Overlay `other` on top of this map (other wins).
    pub fn merge(&mut self, other: &SegmentMap) {
        for (k, v) in &other.map {
            self.map.insert(k.clone(), *v);
        }
    }

    pub fn insert(&mut self, name: impl Into<String>, a: SegAddr) {
        self.map.insert(name.into(), a);
    }

    /// Remove every name that points at `a`; returns the removed names.
    pub fn remove_addr(&mut self, a: SegAddr) -> Vec<String> {
        let names: Vec<String> = self
            .map
            .iter()
            .filter(|(_, v)| **v == a)
            .map(|(k, _)| k.clone())
            .collect();
        for n in &names {
            self.map.remove(n);
        }
        names
    }

    pub fn get(&self, name: &str) -> Option<SegAddr> {
        self.map.get(name).copied()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, SegAddr)> {
        self.map.iter().map(|(k, v)| (k.as_str(), *v))
    }

    /// Number of digits in a group (`main`, `smeter`), found from the map.
    pub fn digit_count(&self, group: &str) -> usize {
        (1..=32)
            .take_while(|n| self.get(&format!("{group}.{n}.a")).is_some())
            .count()
    }

    /// Set one named segment if it exists; returns false if the name is unknown.
    pub fn set(&self, out: &mut OutputReport, name: &str, on: bool) -> bool {
        match self.get(name) {
            Some(a) => {
                out.set_segment(a, on);
                true
            }
            None => false,
        }
    }

    /// Draw a glyph mask on digit `n` (1 = rightmost) of a group.
    pub fn draw_digit(&self, out: &mut OutputReport, group: &str, n: usize, mask: u8) {
        for (i, seg) in font::SEGMENTS.iter().enumerate() {
            if let Some(a) = self.get(&format!("{group}.{n}.{seg}")) {
                out.set_segment(a, mask & (1 << i) != 0);
            }
        }
    }

    /// Right-align `text` on a digit group. Characters with no glyph show
    /// blank; text longer than the group is cut from the left.
    pub fn draw_text(&self, out: &mut OutputReport, group: &str, text: &str) {
        let n = self.digit_count(group);
        let chars: Vec<char> = text.chars().collect();
        for pos in 1..=n {
            let ch = chars.len().checked_sub(pos).map(|i| chars[i]).unwrap_or(' ');
            self.draw_digit(out, group, pos, font::glyph(ch).unwrap_or(0));
        }
    }

    /// Show a frequency in Hz on the main display as M.kkk.hhh: always at
    /// least 7 digits (zero-padded up to the MHz units), with both dots lit.
    pub fn draw_frequency(&self, out: &mut OutputReport, hz: u64) {
        let n = self.digit_count("main").clamp(1, 19) as u32;
        let hz = hz.min(10u64.pow(n) - 1);
        self.draw_text(out, "main", &format!("{hz:07}"));
        self.set(out, "dot.mhz", true);
        self.set(out, "dot.khz", true);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_map_loads() {
        let m = SegmentMap::builtin();
        assert_eq!(m.digit_count("main"), 9);
        assert_eq!(m.digit_count("smeter"), 3);
        assert_eq!(m.get("main.1.a"), Some(SegAddr { byte: 21, bit: 0 }));
    }

    #[test]
    fn addr_parse_rejects_out_of_range() {
        assert!("0.1".parse::<SegAddr>().is_err());
        assert!("33.0".parse::<SegAddr>().is_err());
        assert!("5.8".parse::<SegAddr>().is_err());
        assert_eq!("5.7".parse::<SegAddr>(), Ok(SegAddr { byte: 5, bit: 7 }));
    }

    #[test]
    fn toml_round_trip() {
        let m = SegmentMap::builtin();
        let again = SegmentMap::from_toml(&m.to_toml()).unwrap();
        assert_eq!(m.iter().count(), again.iter().count());
    }

    #[test]
    fn draw_text_right_aligns() {
        let m = SegmentMap::builtin();
        let mut out = OutputReport::default();
        out.clear_segments();
        m.draw_text(&mut out, "main", "1");
        // '1' = segments b and c on digit 1 only.
        assert!(out.segment(m.get("main.1.b").unwrap()));
        assert!(out.segment(m.get("main.1.c").unwrap()));
        assert!(!out.segment(m.get("main.1.a").unwrap()));
        assert!(!out.segment(m.get("main.2.b").unwrap()));
    }
}
