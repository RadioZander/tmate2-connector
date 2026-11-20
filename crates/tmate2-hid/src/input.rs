//! Input report parsing and edge detection.

use crate::{Error, Result};

/// Byte offsets inside the input report as hidapi returns it. Kept in one
/// place so hardware probing can correct it.
#[derive(Debug, Clone, Copy)]
pub struct InputLayout {
    /// Meaningful bytes at the start of the report. The device sends 64-byte
    /// reports; everything after this is stale buffer contents, ignore it.
    pub len: usize,
    /// Status byte. Confirmed on HW1.1/FW1.5: alternates 0x00/0x01.
    pub flags: usize,
    pub main: usize,
    pub e2: usize,
    pub e1: usize,
    /// Start of the 16-bit little-endian button word (9 bits used).
    pub buttons: usize,
}

/// Confirmed on HW1.1/FW1.5: flags byte, 3 x u16 LE encoder counters, then
/// the u16 button word.
pub const LAYOUT: InputLayout = InputLayout {
    len: 9,
    flags: 0,
    main: 1,
    e2: 3,
    e1: 5,
    buttons: 7,
};

/// Mask of the button bits in the button word.
pub const BUTTON_MASK: u16 = 0x01ff;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Encoder {
    Main,
    E1,
    E2,
}

impl Encoder {
    pub const ALL: [Encoder; 3] = [Encoder::Main, Encoder::E1, Encoder::E2];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Button {
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    MainPush,
    E1Push,
    E2Push,
}

impl Button {
    pub const ALL: [Button; 9] = [
        Button::F1,
        Button::F2,
        Button::F3,
        Button::F4,
        Button::F5,
        Button::F6,
        Button::MainPush,
        Button::E1Push,
        Button::E2Push,
    ];

    /// Bit index in the 16-bit button word (confirmed on HW1.1/FW1.5).
    pub const fn bit(self) -> u16 {
        match self {
            Button::F1 => 0,
            Button::F2 => 1,
            Button::F3 => 2,
            Button::F4 => 3,
            Button::F5 => 4,
            Button::F6 => 5,
            Button::MainPush => 6,
            Button::E2Push => 7,
            Button::E1Push => 8,
        }
    }
}

/// One decoded input report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawInput {
    pub main: u16,
    pub e1: u16,
    pub e2: u16,
    /// Raw button word as sent (active-low: a 0 bit means pressed).
    pub buttons_raw: u16,
    /// Status byte (purpose not yet known; not used for events).
    pub flags: u8,
}

impl RawInput {
    /// Parse a report as returned by hidapi. Only the first [`InputLayout::len`]
    /// bytes are meaningful; any trailing bytes are ignored.
    pub fn parse(buf: &[u8]) -> Result<Self> {
        let l = LAYOUT;
        if buf.len() < l.len {
            return Err(Error::ShortReport(buf.len()));
        }
        let u16_at = |i: usize| u16::from_le_bytes([buf[i], buf[i + 1]]);
        Ok(RawInput {
            main: u16_at(l.main),
            e1: u16_at(l.e1),
            e2: u16_at(l.e2),
            buttons_raw: u16_at(l.buttons),
            flags: buf[l.flags],
        })
    }

    /// The bytes that matter, for logging / change detection.
    pub fn significant(buf: &[u8]) -> &[u8] {
        &buf[..buf.len().min(LAYOUT.len)]
    }

    pub fn encoder(&self, e: Encoder) -> u16 {
        match e {
            Encoder::Main => self.main,
            Encoder::E1 => self.e1,
            Encoder::E2 => self.e2,
        }
    }

    pub fn is_pressed(&self, b: Button) -> bool {
        self.buttons_raw & (1 << b.bit()) == 0
    }
}

/// An edge-triggered event derived from consecutive reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputEvent {
    /// Positive delta = clockwise (see [`InputTracker::invert`]).
    Turned {
        encoder: Encoder,
        delta: i32,
    },
    Pressed(Button),
    Released(Button),
}

/// Turns a stream of absolute reports into encoder deltas and button edges.
#[derive(Debug, Clone, Default)]
pub struct InputTracker {
    last: Option<RawInput>,
    /// Per-encoder direction inversion (Main, E1, E2). Confirmed on HW1.1/FW1.5:
    /// the main counter decreases clockwise (so it is inverted), E1/E2 increase.
    /// E1/E2 give 2 counts per detent; the main knob has no detents.
    pub invert: [bool; 3],
}

impl InputTracker {
    pub fn new() -> Self {
        InputTracker {
            last: None,
            invert: [true, false, false],
        }
    }

    /// Forget the baseline (e.g. after reconnect).
    pub fn reset(&mut self) {
        self.last = None;
    }

    pub fn last(&self) -> Option<RawInput> {
        self.last
    }

    /// Feed one report; the first report only sets the baseline.
    pub fn feed(&mut self, now: RawInput, out: &mut Vec<InputEvent>) {
        let Some(prev) = self.last.replace(now) else {
            return;
        };
        for (i, enc) in Encoder::ALL.into_iter().enumerate() {
            // Wrapping 16-bit difference interpreted as signed.
            let mut delta = now.encoder(enc).wrapping_sub(prev.encoder(enc)) as i16 as i32;
            if self.invert[i] {
                delta = -delta;
            }
            if delta != 0 {
                out.push(InputEvent::Turned { encoder: enc, delta });
            }
        }
        for b in Button::ALL {
            match (prev.is_pressed(b), now.is_pressed(b)) {
                (false, true) => out.push(InputEvent::Pressed(b)),
                (true, false) => out.push(InputEvent::Released(b)),
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(main: u16, e2: u16, e1: u16, buttons: u16) -> Vec<u8> {
        let mut v = vec![0x01];
        for x in [main, e2, e1, buttons] {
            v.extend_from_slice(&x.to_le_bytes());
        }
        v
    }

    #[test]
    fn parses_and_ignores_trailing_junk() {
        let mut buf = report(0x1234, 2, 3, 0x01ff);
        let a = RawInput::parse(&buf).unwrap();
        buf.extend_from_slice(&[0xaa; 55]); // the device pads to 64 bytes
        let b = RawInput::parse(&buf).unwrap();
        assert_eq!(a, b);
        assert_eq!(a.main, 0x1234);
        assert_eq!(a.e2, 2);
        assert_eq!(a.e1, 3);
        assert!(RawInput::parse(&buf[..5]).is_err());
    }

    #[test]
    fn encoder_wraps_and_inverts() {
        let mut t = InputTracker::new();
        let mut ev = Vec::new();
        t.feed(
            RawInput::parse(&report(0x0001, 0xfffe, 0, 0xffff)).unwrap(),
            &mut ev,
        );
        assert!(ev.is_empty(), "first report is baseline only");
        t.feed(
            RawInput::parse(&report(0xffff, 0x0001, 0, 0xffff)).unwrap(),
            &mut ev,
        );
        // main: 1 -> 0xffff is raw -2, inverted -> +2 ; e2: 0xfffe -> 1 is +3
        assert_eq!(
            ev,
            vec![
                InputEvent::Turned {
                    encoder: Encoder::Main,
                    delta: 2
                },
                InputEvent::Turned {
                    encoder: Encoder::E2,
                    delta: 3
                },
            ]
        );
    }

    #[test]
    fn buttons_are_active_low() {
        let mut t = InputTracker::new();
        let mut ev = Vec::new();
        t.feed(RawInput::parse(&report(0, 0, 0, 0xffff)).unwrap(), &mut ev);
        t.feed(RawInput::parse(&report(0, 0, 0, !(1 << 2))).unwrap(), &mut ev);
        assert_eq!(ev, vec![InputEvent::Pressed(Button::F3)]);
        ev.clear();
        t.feed(RawInput::parse(&report(0, 0, 0, 0xffff)).unwrap(), &mut ev);
        assert_eq!(ev, vec![InputEvent::Released(Button::F3)]);
    }
}
