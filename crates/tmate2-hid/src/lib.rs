//! Driver for the ELAD / Woodbox TMATE-2 USB controller.
//!
//! The TMATE-2 is a plain USB HID device (no vendor driver). It sends a short
//! input report holding three absolute encoder counters and a button bitmap,
//! and accepts an output report holding the LCD segment bitmap, RGB backlight,
//! contrast, two LEDs, a "click" bit and on-device encoder acceleration.
//!
//! Byte positions were confirmed on hardware (HW1.1 / FW1.5) with
//! `tmate2-probe`. They are kept in one place ([`input::LAYOUT`], the
//! [`output`] constants, and the segment map data file) so other hardware or
//! firmware versions can be accommodated without touching logic.

pub mod device;
pub mod font;
pub mod input;
pub mod output;
pub mod segmap;

pub use device::{list_devices, DeviceSummary, Tmate2};
pub use hidapi;
pub use input::{Button, Encoder, InputEvent, InputTracker, RawInput};
pub use output::OutputReport;
pub use segmap::{SegAddr, SegmentMap};

/// USB vendor ID of the TMATE-2.
pub const VENDOR_ID: u16 = 0x1721;
/// USB product ID of the TMATE-2.
pub const PRODUCT_ID: u16 = 0x0614;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("HID error: {0}")]
    Hid(#[from] hidapi::HidError),
    #[error("TMATE-2 not found (VID {VENDOR_ID:04x}, PID {PRODUCT_ID:04x})")]
    NotFound,
    #[error("input report too short: {0} bytes")]
    ShortReport(usize),
    #[error("segment map: {0}")]
    SegMap(String),
}

pub type Result<T> = std::result::Result<T, Error>;
