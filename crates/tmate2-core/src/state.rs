//! Radio state as reported by the backend (and optimistically updated by the
//! engine so the display responds instantly while tuning).

use std::time::Instant;

/// Operating mode, normalised across backends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Mode {
    Lsb,
    Usb,
    CwL,
    CwU,
    Am,
    Sam,
    Dsb,
    Fm,
    Nfm,
    DigL,
    DigU,
    Drm,
    Other,
}

impl Mode {
    /// Parse a TCI-style modulation name (case-insensitive).
    pub fn from_name(s: &str) -> Mode {
        match s.to_ascii_uppercase().as_str() {
            "LSB" => Mode::Lsb,
            "USB" => Mode::Usb,
            "CWL" | "CW" => Mode::CwL,
            "CWU" | "CWR" => Mode::CwU,
            "AM" => Mode::Am,
            "SAM" => Mode::Sam,
            "DSB" => Mode::Dsb,
            "FM" | "WFM" => Mode::Fm,
            "NFM" => Mode::Nfm,
            "DIGL" | "DIG-" => Mode::DigL,
            "DIGU" | "DIG+" | "DIG" => Mode::DigU,
            "DRM" => Mode::Drm,
            _ => Mode::Other,
        }
    }

    /// TCI modulation name.
    pub fn name(self) -> &'static str {
        match self {
            Mode::Lsb => "LSB",
            Mode::Usb => "USB",
            Mode::CwL => "CWL",
            Mode::CwU => "CWU",
            Mode::Am => "AM",
            Mode::Sam => "SAM",
            Mode::Dsb => "DSB",
            Mode::Fm => "FM",
            Mode::Nfm => "NFM",
            Mode::DigL => "DIGL",
            Mode::DigU => "DIGU",
            Mode::Drm => "DRM",
            Mode::Other => "",
        }
    }

    pub fn is_lower_sideband(self) -> bool {
        matches!(self, Mode::Lsb | Mode::CwL | Mode::DigL)
    }
}

/// One transceiver (TCI "trx"): a receiver with VFO A / B and its controls.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Trx {
    /// While tuning locally, ignore VFO echoes from the radio until this time,
    /// so stale echoes don't make the display jump back.
    pub vfo_guard_until: Option<Instant>,
    /// VFO frequencies in Hz: [A, B].
    pub vfo: [u64; 2],
    pub mode: Option<Mode>,
    /// Receive filter passband edges in Hz relative to the carrier.
    pub filter: Option<(i32, i32)>,
    /// Latest received signal strength in dBm.
    pub rx_dbm: Option<f32>,
    /// Latest transmit readings (TCI tx_sensors): peak power in W and SWR.
    pub tx_power_w: Option<f32>,
    pub tx_swr: Option<f32>,
    pub nr: bool,
    pub nb: bool,
    pub anf: bool,
    /// Manual notch filter (Thetis MNF).
    pub notch: bool,
    pub rit: bool,
    pub rit_offset: i32,
    pub xit: bool,
    pub xit_offset: i32,
    pub split: bool,
    pub lock: bool,
    pub tx: bool,
    pub tune: bool,
    /// Drive in percent.
    pub drive: Option<u8>,
    pub sql_enable: bool,
    /// Squelch level in dB.
    pub sql_level: Option<i32>,
    /// AGC gain / threshold (Thetis "AGC-T"), shown as RF gain.
    pub agc_gain: Option<i32>,
    /// AGC mode name as TCI sends it: off, long, slow, normal, fast, custom.
    pub agc_mode: Option<String>,
    /// Receiver switched on (TCI rx_enable; the first receiver is always on).
    pub rx_enable: bool,
    /// Second channel (sub-receiver) of this receiver switched on.
    pub sub_rx: bool,
    pub apf: bool,
    pub binaural: bool,
    /// This receiver muted (TCI rx_mute), separate from the master mute.
    pub rx_mute: bool,
    /// This receiver's own volume in dB (main channel).
    pub rx_volume_db: Option<f32>,
    /// Drive used while tuning, in percent.
    pub tune_drive: Option<u8>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct RadioState {
    /// Backend connected and initial state received.
    pub connected: bool,
    /// Server identification, e.g. "ExpertSDR3,2.0".
    pub protocol: Option<String>,
    pub device: Option<String>,
    /// TCI-X negotiated; holds the capability names the server announced.
    pub tcix: Option<Vec<String>>,
    pub trx: Vec<Trx>,
    /// Modes the radio offers, in its own order.
    pub modes: Vec<Mode>,
    /// Master volume in dB (TCI: -60 .. 0).
    pub volume_db: Option<f32>,
    pub mute: bool,
    /// Tuning limits in Hz.
    pub vfo_limits: Option<(u64, u64)>,
    /// TX monitor.
    pub monitor: bool,
    /// Monitor volume in dB (-60 .. 0).
    pub mon_volume_db: Option<f32>,
    /// CW keyer speed in words per minute.
    pub cw_speed: Option<u32>,
    /// Radio started (TCI start / stop), if reported.
    pub power: Option<bool>,
}

impl RadioState {
    /// Mutable access to a transceiver, creating it if needed.
    pub fn trx_mut(&mut self, i: usize) -> &mut Trx {
        if self.trx.len() <= i {
            self.trx.resize_with(i + 1, Trx::default);
        }
        &mut self.trx[i]
    }

    pub fn trx(&self, i: usize) -> Option<&Trx> {
        self.trx.get(i)
    }

    /// Apply a VFO frequency reported by the radio, unless we are tuning
    /// that transceiver locally right now.
    pub fn remote_vfo(&mut self, trx: usize, vfo: usize, hz: u64, now: Instant) {
        let t = self.trx_mut(trx);
        if t.vfo_guard_until.is_some_and(|g| now < g) || vfo > 1 {
            return;
        }
        t.vfo[vfo] = hz;
    }

    pub fn has_tcix(&self, cap: &str) -> bool {
        self.tcix.as_ref().is_some_and(|c| c.iter().any(|x| x == cap))
    }
}
