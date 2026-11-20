//! Output report: LCD segments, backlight, LEDs, click and encoder acceleration.
//!
//! Byte 0 is the HID report ID (0 = device uses no report IDs). Layout confirmed
//! on HW1.1 / FW1.5: the device accepts the full 45-byte write.

use crate::segmap::SegAddr;

/// Full buffer length including the report-ID byte.
pub const REPORT_LEN: usize = 45;
/// Segment bitmap occupies bytes `SEGMENT_FIRST..SEGMENT_END` (byte 33 holds LEDs).
pub const SEGMENT_FIRST: usize = 1;
pub const SEGMENT_END: usize = 33;

const LEDS: usize = 33;
const LED_USB_BIT: u8 = 0;
const LED_LOCK_BIT: u8 = 1;
const CLICK_BIT: u8 = 2;
const RGB: usize = 34;
const CONTRAST: usize = 37;
const REFRESH: usize = 38;
const ENC_SPEED: usize = 39; // three bytes: speed 1, 2, 3
const ENC_TRANS12: usize = 42;
const ENC_TRANS23: usize = 43;
const ENC_TRANS_TIME: usize = 44;

/// On-device encoder acceleration for the main knob: the device multiplies
/// steps by `speeds[n]` once rotation rate passes the thresholds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncoderAccel {
    pub speeds: [u8; 3],
    pub trans12: u8,
    pub trans23: u8,
    pub trans_time: u8,
}

impl EncoderAccel {
    /// No hardware acceleration: every detent is one count. Acceleration is
    /// done in software instead, where it is configurable per mode.
    pub const FLAT: EncoderAccel = EncoderAccel {
        speeds: [1, 1, 1],
        trans12: 255,
        trans23: 255,
        trans_time: 10,
    };
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputReport {
    pub bytes: [u8; REPORT_LEN],
}

impl Default for OutputReport {
    fn default() -> Self {
        let mut r = OutputReport {
            bytes: [0; REPORT_LEN],
        };
        r.set_rgb(255, 255, 255);
        r.set_contrast(0); // lower = darker; 192+ is invisible (HW1.1/FW1.5)
        r.set_encoder_accel(EncoderAccel::FLAT);
        r
    }
}

impl OutputReport {
    pub fn set_segment(&mut self, a: SegAddr, on: bool) {
        let b = &mut self.bytes[a.byte as usize];
        if on {
            *b |= 1 << a.bit;
        } else {
            *b &= !(1 << a.bit);
        }
    }

    pub fn segment(&self, a: SegAddr) -> bool {
        self.bytes[a.byte as usize] & (1 << a.bit) != 0
    }

    /// Clear every LCD segment (LEDs, backlight and settings are untouched).
    pub fn clear_segments(&mut self) {
        for b in &mut self.bytes[SEGMENT_FIRST..SEGMENT_END] {
            *b = 0;
        }
    }

    /// Light every LCD segment (display test).
    pub fn all_segments(&mut self) {
        for b in &mut self.bytes[SEGMENT_FIRST..SEGMENT_END] {
            *b = 0xff;
        }
    }

    pub fn set_rgb(&mut self, r: u8, g: u8, b: u8) {
        self.bytes[RGB..RGB + 3].copy_from_slice(&[r, g, b]);
    }

    pub fn rgb(&self) -> (u8, u8, u8) {
        (self.bytes[RGB], self.bytes[RGB + 1], self.bytes[RGB + 2])
    }

    pub fn set_contrast(&mut self, c: u8) {
        self.bytes[CONTRAST] = c;
    }

    pub fn set_refresh(&mut self, v: u8) {
        self.bytes[REFRESH] = v;
    }

    fn set_bit(&mut self, byte: usize, bit: u8, on: bool) {
        self.set_segment(
            SegAddr {
                byte: byte as u8,
                bit,
            },
            on,
        );
    }

    pub fn set_usb_led(&mut self, on: bool) {
        self.set_bit(LEDS, LED_USB_BIT, on);
    }

    pub fn set_lock_led(&mut self, on: bool) {
        self.set_bit(LEDS, LED_LOCK_BIT, on);
    }

    /// The device clicks when this bit changes state.
    pub fn toggle_click(&mut self) {
        self.bytes[LEDS] ^= 1 << CLICK_BIT;
    }

    pub fn set_encoder_accel(&mut self, a: EncoderAccel) {
        self.bytes[ENC_SPEED..ENC_SPEED + 3].copy_from_slice(&a.speeds);
        self.bytes[ENC_TRANS12] = a.trans12;
        self.bytes[ENC_TRANS23] = a.trans23;
        self.bytes[ENC_TRANS_TIME] = a.trans_time;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segment_bits_round_trip() {
        let mut r = OutputReport::default();
        let a = SegAddr { byte: 12, bit: 5 };
        r.set_segment(a, true);
        assert!(r.segment(a));
        assert_eq!(r.bytes[12], 1 << 5);
        r.set_segment(a, false);
        assert!(!r.segment(a));
    }

    #[test]
    fn clear_keeps_leds_and_settings() {
        let mut r = OutputReport::default();
        r.set_lock_led(true);
        r.all_segments();
        r.clear_segments();
        assert!(r.bytes[1..33].iter().all(|&b| b == 0));
        assert_eq!(r.bytes[33], 1 << 1);
        assert_eq!(r.rgb(), (255, 255, 255));
    }
}
