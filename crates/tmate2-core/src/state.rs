//! Radio state as reported by the backend (and optimistically updated by the
//! engine so the display responds instantly while tuning).

use std::collections::HashMap;
use std::time::Instant;

use crate::manifest::{CapKind, Manifest};

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
    /// Parse a TCI-style modulation name (case-insensitive). A server with
    /// `cw` and `cwr` (ExpertSDR, kadrad-hub) means the usual CW by `cw`, as
    /// Thetis does by `CWU`.
    pub fn from_name(s: &str) -> Mode {
        match s.to_ascii_uppercase().as_str() {
            "LSB" => Mode::Lsb,
            "USB" => Mode::Usb,
            "CWL" | "CWR" => Mode::CwL,
            "CWU" | "CW" => Mode::CwU,
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

    /// The names a server may know this mode by, the likeliest first:
    /// servers differ (`CWU` or `cw`, `FM` or `nfm`).
    fn spellings(self) -> &'static [&'static str] {
        match self {
            Mode::CwU => &["CWU", "CW"],
            Mode::CwL => &["CWL", "CWR"],
            Mode::Fm => &["FM", "NFM"],
            Mode::Nfm => &["NFM", "FM"],
            _ => &[],
        }
    }

    /// The same mode for stepping through a list: the two names of
    /// communications FM count as one.
    pub fn same_as(self, other: Mode) -> bool {
        self == other || matches!((self, other), (Mode::Fm, Mode::Nfm) | (Mode::Nfm, Mode::Fm))
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
    /// TCI-X `preamp`: the radio's own name for the setting (`off`, `p1`…).
    pub preamp: Option<String>,
    /// TCI-X `attenuator`: the step in use, in dB (`0`, `12`…).
    pub attenuator: Option<String>,
    /// TCI-X `antenna`: the antenna in use, by the radio's name for it.
    pub antenna: Option<String>,
    /// TCI-X `rx_antenna`: receiving on the separate receive antenna.
    pub rx_antenna: bool,
    /// TCI-X `pa_enable`: the radio's own power amplifier is in circuit.
    pub pa_enable: Option<bool>,
}

/// What a front-panel lamp with a step number (ATT, PRE) shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lamp {
    Off,
    /// On, on a radio with one step: no number.
    On,
    /// On, at step 1 or 2 (2 for any step above the first).
    Step(u8),
}

/// The lamp for a setting chosen from the server's list, whose first value
/// is "off". With no list, anything but `off` or `0` is on.
fn step_lamp(values: &[String], current: Option<&str>) -> Lamp {
    let Some(cur) = current else { return Lamp::Off };
    match values.iter().position(|v| v.eq_ignore_ascii_case(cur)) {
        Some(0) => Lamp::Off,
        Some(_) if values.len() == 2 => Lamp::On,
        Some(at) => Lamp::Step(at.min(2) as u8),
        None if cur.eq_ignore_ascii_case("off") || cur == "0" => Lamp::Off,
        None => Lamp::On,
    }
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
    /// The same list as the server spells it (`modulations_list`).
    pub mode_names: Vec<String>,
    /// Master volume in dB (TCI: -60 .. 0).
    pub volume_db: Option<f32>,
    pub mute: bool,
    /// Tuning limits in Hz.
    pub vfo_limits: Option<(u64, u64)>,
    /// TCI-X `dial_lock`: the radio's own dial lock (one for the radio).
    pub dial_lock: bool,
    /// TCI-X `receive_only`: this connection may never transmit.
    pub receive_only: bool,
    /// TCI-X `radio_state`: `connected`, or why the server can't reach the
    /// radio (`no_response`, `powered_off`, `port_lost`).
    pub radio_state: Option<String>,
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

    /// The name to send for `mode`: the server's own spelling where its
    /// list has the mode under one of its names, else ours.
    pub fn mode_wire_name(&self, mode: Mode) -> String {
        let listed = |want: &str| self.mode_names.iter().find(|n| n.eq_ignore_ascii_case(want));
        listed(mode.name())
            .or_else(|| mode.spellings().iter().find_map(|s| listed(s)))
            .cloned()
            .unwrap_or_else(|| mode.name().to_string())
    }

    /// The server's list has `mode`, under any of its names. True with no
    /// list to ask.
    pub fn offers_mode(&self, mode: Mode) -> bool {
        self.mode_names.is_empty()
            || self.modes.contains(&mode)
            || mode
                .spellings()
                .iter()
                .any(|s| self.mode_names.iter().any(|n| n.eq_ignore_ascii_case(s)))
    }

    /// What the ATT and PRE lamps show for `trx`, from the server's own
    /// lists (TCI-X `attenuator`, `preamp`). A preamp given as a list of
    /// gains in dB (a FlexRadio: -20 … 40) lights PRE above 0 and ATT below.
    pub fn front_end_lamps(&self, trx: usize) -> (Lamp, Lamp) {
        let Some(x) = self.trx(trx) else {
            return (Lamp::Off, Lamp::Off);
        };
        let list = |cap: &str| {
            self.tcix
                .as_ref()
                .and_then(|m| m.enum_values(cap))
                .unwrap_or_default()
        };
        let mut att = step_lamp(list("attenuator"), x.attenuator.as_deref());
        let gains = list("preamp");
        let in_db = gains.iter().any(|g| g.parse::<f32>().is_ok_and(|g| g < 0.0));
        let pre = if in_db {
            match x.preamp.as_deref().and_then(|g| g.parse::<f32>().ok()) {
                Some(g) if g > 0.0 => Lamp::On,
                Some(g) if g < 0.0 => {
                    if att == Lamp::Off {
                        att = Lamp::On;
                    }
                    Lamp::Off
                }
                _ => Lamp::Off,
            }
        } else {
            step_lamp(gains, x.preamp.as_deref())
        };
        (att, pre)
    }

    /// The ANT lamp: receiving on the separate receive antenna, or on any
    /// antenna but the first in the server's list.
    pub fn other_antenna(&self, trx: usize) -> bool {
        let Some(x) = self.trx(trx) else { return false };
        let first = self
            .tcix
            .as_ref()
            .and_then(|m| m.enum_values("antenna"))
            .and_then(|l| l.first());
        x.rx_antenna
            || matches!((x.antenna.as_deref(), first), (Some(a), Some(f)) if !a.eq_ignore_ascii_case(f))
    }

    /// How far the dial may go, in Hz. With TCI-X, the range the manifest
    /// gives `vfo`: it can change while connected (a FlexRadio's
    /// transverter bands are known only once the server has logged in to
    /// the radio), which the `vfo_limits` of the greeting can't. Without
    /// one, `vfo_limits`; with neither, anywhere.
    pub fn dial_limits(&self) -> (u64, u64) {
        let from_manifest = self.tcix.as_ref().and_then(|m| match m.get("vfo")?.kind {
            CapKind::Range { min, max, .. } if max > min && min >= 0.0 => Some((min as u64, max as u64)),
            _ => None,
        });
        from_manifest.or(self.vfo_limits).unwrap_or((0, 1_000_000_000))
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
