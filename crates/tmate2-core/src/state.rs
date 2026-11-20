//! Radio state as reported by the backend (and optimistically updated by the
//! engine so the display responds instantly while tuning).

use std::collections::HashMap;
use std::time::Instant;

use crate::manifest::Manifest;

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
    /// TCI-X `af_gain`: the radio's own AF gain, 0-100.
    pub af_gain: Option<i32>,
    /// TCI-X `rf_gain`, 0-100.
    pub rf_gain: Option<i32>,
    /// TCI-X `squelch`: squelch knob position, 0-100.
    pub squelch: Option<i32>,
    /// TCI-X `filter`: the named filter preset in use (receiver channel 0).
    pub filter_preset: Option<String>,
    /// TCI-X `notch`: the manual notch's audio frequency in Hz.
    pub notch_hz: Option<i32>,
    /// TCI-X `vfo_select`: the VFO the radio has active (0 = A, 1 = B).
    pub vfo_selected: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct RadioState {
    /// Backend connected and initial state received.
    pub connected: bool,
    /// Server identification, e.g. "ExpertSDR3,2.0".
    pub protocol: Option<String>,
    pub device: Option<String>,
    /// TCI-X negotiated; holds the server's capability manifest.
    pub tcix: Option<Manifest>,
    pub trx: Vec<Trx>,
    /// Modes the radio offers, in its own order.
    pub modes: Vec<Mode>,
    /// Master volume in dB (TCI: -60 .. 0).
    pub volume_db: Option<f32>,
    pub mute: bool,
    /// Tuning limits in Hz.
    pub vfo_limits: Option<(u64, u64)>,
    /// TCI-X `dial_lock`: the radio's own dial lock (one for the radio).
    pub dial_lock: bool,
    /// TCI-X `receive_only`: this connection may never transmit.
    pub receive_only: bool,
    /// The account we logged in as and the role the server gave it
    /// (TCI-X §4.3), e.g. `operate`.
    pub login_user: Option<String>,
    pub role: Option<String>,
    /// Who holds control of the radio (TCI-X §4.4); `None` when nobody
    /// does, or the server doesn't apply control.
    pub control: Option<String>,
    /// Shown instead of "no radio" while disconnected, when we know why
    /// (e.g. "login", "idle").
    pub notice: Option<String>,
    /// The server closed us for being idle (TCI-X §4.4): don't reconnect
    /// until the user touches the controller.
    pub idle_closed: bool,
    /// A short message to flash on the display (a refusal, a control
    /// request). `alert_seq` counts them, so each is shown once.
    pub alert: Option<String>,
    pub alert_seq: u64,
    /// Commands (by wire name) whose reports from the radio are ignored
    /// until the time given, because a knob here is setting them right now.
    pub guards: HashMap<&'static str, Instant>,
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

    /// Ignore the radio's reports of `command` until `until`: a knob here is
    /// setting it, and the echo of an earlier click would undo later ones.
    pub fn guard(&mut self, command: &'static str, until: Instant) {
        self.guards.insert(command, until);
    }

    /// Reports of `command` are being ignored (see [`RadioState::guard`]).
    pub fn guarded(&self, command: &str, now: Instant) -> bool {
        self.guards.get(command).is_some_and(|until| now < *until)
    }

    /// Flash a short message on the display.
    pub fn push_alert(&mut self, text: impl Into<String>) {
        self.alert = Some(text.into());
        self.alert_seq = self.alert_seq.wrapping_add(1);
    }

    /// We hold control of the radio (TCI-X §4.4).
    pub fn in_control(&self) -> bool {
        self.control.is_some() && self.control == self.login_user
    }

    /// The server listed `cap` in its TCI-X manifest.
    pub fn has_tcix(&self, cap: &str) -> bool {
        self.tcix.as_ref().is_some_and(|m| m.has(cap))
    }

    /// The server takes this command from us. Without TCI-X there is no
    /// manifest to ask, so every command is tried, as plain TCI clients do.
    /// With TCI-X, only commands the manifest lists as writable.
    pub fn can_send(&self, command: &str) -> bool {
        self.tcix.as_ref().is_none_or(|m| m.writable(command))
    }

    /// TCI-X negotiated and the server takes this extension command. Used to
    /// prefer an extension over its base equivalent.
    pub fn tcix_offers(&self, command: &str) -> bool {
        self.tcix.as_ref().is_some_and(|m| m.writable(command))
    }
}
