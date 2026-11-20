//! Opening and talking to the physical device through hidapi.

use hidapi::{HidApi, HidDevice};

use crate::input::RawInput;
use crate::output::{OutputReport, REPORT_LEN};
use crate::{Error, Result, PRODUCT_ID, VENDOR_ID};

/// What `list_devices` reports about each matching HID interface.
#[derive(Debug, Clone)]
pub struct DeviceSummary {
    pub path: String,
    pub vendor_id: u16,
    pub product_id: u16,
    pub manufacturer: Option<String>,
    pub product: Option<String>,
    pub serial: Option<String>,
    pub interface: i32,
    pub usage_page: u16,
    pub usage: u16,
}

/// List HID interfaces. With `all = false` only TMATE-2 interfaces are returned.
pub fn list_devices(api: &HidApi, all: bool) -> Vec<DeviceSummary> {
    api.device_list()
        .filter(|d| all || (d.vendor_id() == VENDOR_ID && d.product_id() == PRODUCT_ID))
        .map(|d| DeviceSummary {
            path: d.path().to_string_lossy().into_owned(),
            vendor_id: d.vendor_id(),
            product_id: d.product_id(),
            manufacturer: d.manufacturer_string().map(str::to_owned),
            product: d.product_string().map(str::to_owned),
            serial: d.serial_number().map(str::to_owned),
            interface: d.interface_number(),
            usage_page: d.usage_page(),
            usage: d.usage(),
        })
        .collect()
}

pub struct Tmate2 {
    dev: HidDevice,
    out: OutputReport,
    /// Write length that the device accepted, found on first flush.
    write_len: Option<usize>,
}

impl Tmate2 {
    /// Open the first TMATE-2 found.
    pub fn open(api: &HidApi) -> Result<Self> {
        let dev = api
            .open(VENDOR_ID, PRODUCT_ID)
            .map_err(|e| match list_devices(api, false).is_empty() {
                true => Error::NotFound,
                false => Error::Hid(e),
            })?;
        Ok(Tmate2 {
            dev,
            out: OutputReport::default(),
            write_len: None,
        })
    }

    pub fn output(&self) -> &OutputReport {
        &self.out
    }

    pub fn output_mut(&mut self) -> &mut OutputReport {
        &mut self.out
    }

    /// Read one input report. Returns `Ok(None)` on timeout.
    /// `timeout_ms` of -1 blocks.
    pub fn read(&self, timeout_ms: i32) -> Result<Option<RawInput>> {
        let (n, buf) = self.read_raw(timeout_ms)?;
        if n == 0 {
            return Ok(None);
        }
        RawInput::parse(&buf[..n]).map(Some)
    }

    /// Read one report as raw bytes (for probing). Returns (len, buffer).
    pub fn read_raw(&self, timeout_ms: i32) -> Result<(usize, [u8; 64])> {
        let mut buf = [0u8; 64];
        let n = self.dev.read_timeout(&mut buf, timeout_ms)?;
        Ok((n, buf))
    }

    /// Send the current output report. The exact output length isn't known
    /// for certain, so the full report is tried first, then one byte shorter;
    /// the length that works is remembered.
    pub fn flush(&mut self) -> Result<()> {
        if let Some(len) = self.write_len {
            self.dev.write(&self.out.bytes[..len])?;
            return Ok(());
        }
        let mut last_err = None;
        for len in [REPORT_LEN, REPORT_LEN - 1] {
            match self.dev.write(&self.out.bytes[..len]) {
                Ok(_) => {
                    self.write_len = Some(len);
                    return Ok(());
                }
                Err(e) => last_err = Some(e),
            }
        }
        Err(last_err.expect("at least one attempt").into())
    }

    /// Which write length the device accepted (after the first flush).
    pub fn write_len(&self) -> Option<usize> {
        self.write_len
    }

    /// Make the device click once.
    pub fn click(&mut self) -> Result<()> {
        self.out.toggle_click();
        self.flush()
    }
}
