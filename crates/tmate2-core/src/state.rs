//! Radio state as the radio reports it. The display shows only this: a
//! command we send changes nothing here until the radio reports the result.
//! While a knob turns, the engine works from [`RadioState::working`], this
//! state with the values it has asked for and not yet seen reported.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use crate::command::{Command, Target};
use crate::manifest::{CapKind, Manifest};

/// Operating mode, normalised across backends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Mode {
    Lsb,
    Usb,
    CwL,
    CwU,
    /// CW with the sideband not reported (piHPSDR sends plain "CW" for
    /// both CWL and CWU).
    Cw,
    Am,
    Sam,
    Dsb,
    Fm,
    Nfm,
    /// Wide FM, only when the radio reports it as such ("WFM"). Plain FM
    /// shows without the W: most radios can't say which they mean.
    Wfm,
    DigL,
    DigU,
    Drm,
    Other,
}

impl Mode {
    /// Parse a TCI-style modulation name (case-insensitive). Plain `cw` is
    /// CW with the sideband unknown: piHPSDR reports CWL and CWU both as
    /// `CW`, so the LCD must not guess one. `cwr` (ExpertSDR, kadrad-hub) is
    /// the reverse sideband, CWL.
    pub fn from_name(s: &str) -> Mode {
        match s.to_ascii_uppercase().as_str() {
            "LSB" => Mode::Lsb,
            "USB" => Mode::Usb,
            // CW-R is taken as CWL for now. Which sideband "reverse" means
            // depends on the radio, so this belongs in radio-specific settings.
            "CWL" | "CWR" => Mode::CwL,
            "CWU" => Mode::CwU,
            "CW" => Mode::Cw,
            "AM" => Mode::Am,
            "SAM" => Mode::Sam,
            "DSB" => Mode::Dsb,
            // piHPSDR lists its FM mode as "FMN" but reports it as "FM".
            "FM" | "FMN" => Mode::Fm,
            "WFM" => Mode::Wfm,
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
            Mode::Cw => "CW",
            Mode::Am => "AM",
            Mode::Sam => "SAM",
            Mode::Dsb => "DSB",
            Mode::Fm => "FM",
            Mode::Nfm => "NFM",
            Mode::Wfm => "WFM",
            Mode::DigL => "DIGL",
            Mode::DigU => "DIGU",
            Mode::Drm => "DRM",
            Mode::Other => "",
        }
    }

    pub fn is_lower_sideband(self) -> bool {
        matches!(self, Mode::Lsb | Mode::CwL | Mode::DigL)
    }

    /// Whether `self` and `other` can be the same radio mode: equal, one is
    /// CW with an unknown sideband and the other CWL / CWU, or the two names
    /// of communications FM.
    pub fn matches(self, other: Mode) -> bool {
        self == other
            || matches!(
                (self, other),
                (Mode::Cw, Mode::CwL | Mode::CwU)
                    | (Mode::CwL | Mode::CwU, Mode::Cw)
                    | (Mode::Fm, Mode::Nfm)
                    | (Mode::Nfm, Mode::Fm)
            )
    }

    /// The names a server may know this mode by, the likeliest first:
    /// servers differ (`CWU` or `cw`, `FM` or `nfm`).
    fn spellings(self) -> &'static [&'static str] {
        match self {
            Mode::CwU => &["CWU", "CW"],
            Mode::CwL => &["CWL", "CWR", "CW"],
            Mode::Cw => &["CW", "CWU", "CWL"],
            Mode::Fm => &["FM", "NFM", "FMN"],
            Mode::Nfm => &["NFM", "FM", "FMN"],
            Mode::Wfm => &["WFM"],
            _ => &[],
        }
    }
}

/// One transceiver (TCI "trx"): a receiver with VFO A / B and its controls.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Trx {
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
    /// TX monitor.
    pub monitor: bool,
    /// Monitor volume in dB (-60 .. 0).
    pub mon_volume_db: Option<f32>,
    /// CW keyer speed in words per minute.
    pub cw_speed: Option<u32>,
    /// Radio started (TCI start / stop), if reported.
    pub power: Option<bool>,
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
    /// What we have asked the radio for and not yet seen it report, by
    /// target (see [`RadioState::expect`]).
    pub pending: HashMap<Target, Pending>,
    /// When the radio last reported each readable value (see
    /// [`Target::of_report`]), however it came: a change, an echo, a read.
    pub heard: HashMap<Target, Instant>,
    /// Reads sent and not yet answered, with when they went.
    pub reading: HashMap<Target, Instant>,
    /// Commands whose reads this server leaves unanswered: they are not
    /// waited for or polled again on this connection.
    pub unreadable: HashSet<&'static str>,
    /// The last report of each value asked for, as sent (its arguments).
    pub last_report: HashMap<Target, Vec<String>>,
}

/// A value asked for (see [`RadioState::expect`]).
#[derive(Debug, Clone, PartialEq)]
pub struct Pending {
    /// The newest command for this target.
    pub cmd: Command,
    /// Until then the knobs carry on from `cmd`, not from the last report.
    /// After it, a command the radio never answered is asked about once.
    pub until: Instant,
    /// Commands for this target sent and not yet reported back.
    pub unanswered: u32,
    /// `cmd` itself has been sent (the pacer may still hold it back).
    pub sent: bool,
    /// The radio didn't answer in time and was asked for the value.
    pub asked: bool,
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

    /// A VFO frequency reported by the radio.
    pub fn remote_vfo(&mut self, trx: usize, vfo: usize, hz: u64) {
        if vfo < 2 {
            self.trx_mut(trx).vfo[vfo] = hz;
        }
    }

    /// We are sending `cmd`. Until `until` the knobs carry on from it rather
    /// than from the last report, so a fast turn isn't pulled back by the
    /// echoes of its earlier clicks. Nothing shown changes until the radio
    /// reports.
    pub fn expect(&mut self, cmd: Command, until: Instant) {
        let Some(target) = cmd.target() else { return };
        let p = self.pending.entry(target).or_insert_with(|| Pending {
            cmd: cmd.clone(),
            until,
            unanswered: 0,
            sent: false,
            asked: false,
        });
        p.cmd = cmd;
        p.until = until;
        p.sent = false;
        p.asked = false;
    }

    /// `cmd` has gone to the radio.
    pub fn sent(&mut self, cmd: &Command) {
        if let Some(p) = cmd.target().and_then(|t| self.pending.get_mut(&t)) {
            p.unanswered += 1;
            if p.cmd == *cmd {
                p.sent = true;
            }
        }
    }

    /// The radio reported `name:args;`, already applied. The knobs carry on
    /// from the value they asked for until the radio reports that value (it
    /// has caught up), or until the hold runs out: an earlier click's echo
    /// mustn't become what the next click counts from (G0KAD, IC-7100
    /// behind kadrad-hub). What is shown is the report either way.
    ///
    /// The hold also ends when the radio has answered everything sent and
    /// says the same as last time: it is staying where it is (at the end of
    /// its range, as piHPSDR's volume does at -40 dB), so the knob mustn't
    /// count on past it. Late echoes of a turning knob differ each time.
    pub fn reported(&mut self, name: &str, args: &[String]) {
        let hit: Vec<Target> = self
            .pending
            .keys()
            .filter(|t| t.reported_by(name, args))
            .copied()
            .collect();
        for t in hit {
            let shows = self.shows(&self.pending[&t].cmd);
            let again = self.last_report.insert(t, args.to_vec()).as_deref() == Some(args);
            let p = self.pending.get_mut(&t).expect("listed above");
            p.unanswered = p.unanswered.saturating_sub(1);
            let staying = again && p.sent && p.unanswered == 0;
            if shows || staying {
                self.pending.remove(&t);
            }
        }
    }

    /// Note when a readable value was reported (see [`RadioState::heard`]).
    pub fn note_heard(&mut self, name: &str, args: &[String], now: Instant) {
        if let Some(t) = Target::of_report(name, args) {
            self.heard.insert(t, now);
            self.reading.remove(&t);
        }
    }

    /// What we have for `t` can be changed from without asking first: the
    /// radio reported it within `fresh`, a knob here is setting it, or it
    /// can't be asked for (not readable, or this server doesn't answer).
    pub fn is_current(&self, t: &Target, now: Instant, fresh: Duration) -> bool {
        !t.readable()
            || self.unreadable.contains(t.name)
            || self.pending.get(t).is_some_and(|p| now < p.until && !p.asked)
            || self
                .heard
                .get(t)
                .is_some_and(|h| now.saturating_duration_since(*h) < fresh)
    }

    /// The read for `t`, unless one is already out, this server doesn't
    /// answer them, or its TCI-X manifest doesn't list the command.
    pub fn read(&mut self, t: Target, now: Instant) -> Option<Command> {
        if !t.readable()
            || !self.can_read(t.name)
            || self.unreadable.contains(t.name)
            || self.reading.contains_key(&t)
        {
            return None;
        }
        self.reading.insert(t, now);
        Some(Command::Query(t))
    }

    /// Reads unanswered for `wait`: this server doesn't answer reads of
    /// that command, so stop asking.
    pub fn expire_reads(&mut self, now: Instant, wait: Duration) {
        let late: Vec<Target> = self
            .reading
            .iter()
            .filter(|(_, at)| now.saturating_duration_since(**at) >= wait)
            .map(|(t, _)| *t)
            .collect();
        for t in late {
            self.reading.remove(&t);
            self.unreadable.insert(t.name);
        }
    }

    /// What the radio reports already is what `cmd` sets.
    fn shows(&self, cmd: &Command) -> bool {
        let mut probe = self.clone();
        probe.apply(cmd);
        probe == *self
    }

    /// This state with the values asked for and not yet reported, for the
    /// knobs and buttons to work from.
    pub fn working(&self, now: Instant) -> RadioState {
        let mut w = self.clone();
        for p in self.pending.values().filter(|p| now < p.until && !p.asked) {
            w.apply(&p.cmd);
        }
        w
    }

    /// Take back from a [`RadioState::working`] copy what the engine may
    /// change for real: the record of what has been asked for and read, and
    /// the user's return after an idle close. The values it worked from stay
    /// behind; this state keeps what the radio reported.
    pub fn adopt(&mut self, work: RadioState) {
        self.pending = work.pending;
        self.reading = work.reading;
        self.idle_closed = work.idle_closed;
    }

    /// Follow up what the radio left unanswered by its time: ask once for
    /// the value (the reads returned here are to be sent), and if that goes
    /// unanswered for `wait` too, forget the value rather than show a
    /// guess. Values the radio answered, or that never went, just expire.
    pub fn overdue(&mut self, now: Instant, wait: Duration) -> Vec<Command> {
        let late: Vec<Target> = self
            .pending
            .iter()
            .filter(|(_, p)| now >= p.until)
            .map(|(t, _)| *t)
            .collect();
        let mut reads = Vec::new();
        for t in late {
            let p = self.pending.get_mut(&t).expect("listed above");
            if !p.asked && p.sent && p.unanswered > 0 {
                if let Some(t) = p.cmd.query().and(Some(t)) {
                    p.asked = true;
                    p.until = now + wait;
                    reads.extend(self.read(t, now));
                    continue;
                }
            }
            let p = self.pending.remove(&t).expect("listed above");
            if p.asked {
                self.forget(&p.cmd);
            }
        }
        reads
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

    /// The server's list has `mode`, under any of its names (plain CW
    /// stands for CWL and CWU). True with no list to ask.
    pub fn offers_mode(&self, mode: Mode) -> bool {
        (self.mode_names.is_empty() && self.modes.is_empty())
            || self.modes.iter().any(|r| r.matches(mode))
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

    /// The server can say what this is. Without TCI-X, every readable
    /// command is tried. With TCI-X, only those the manifest lists: a read
    /// of anything else is refused (`error:<command>,unsupported`, TCI-X
    /// B3), every poll, as `tune` on a radio with no tune carrier, or
    /// base `lock` where the radio's lock is `dial_lock`.
    pub fn can_read(&self, command: &str) -> bool {
        self.tcix.as_ref().is_none_or(|m| m.has(command))
    }

    /// TCI-X negotiated and the server takes this extension command. Used to
    /// prefer an extension over its base equivalent.
    pub fn tcix_offers(&self, command: &str) -> bool {
        self.tcix.as_ref().is_some_and(|m| m.writable(command))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_names_from_thetis_and_pihpsdr() {
        assert_eq!(Mode::from_name("CWL"), Mode::CwL);
        assert_eq!(Mode::from_name("cwu"), Mode::CwU);
        assert_eq!(Mode::from_name("CW"), Mode::Cw);
        assert_eq!(Mode::from_name("FMN"), Mode::Fm);
        assert_eq!(Mode::from_name("FM"), Mode::Fm);
        assert_eq!(Mode::from_name("NFM"), Mode::Nfm);
        assert_eq!(Mode::from_name("wfm"), Mode::Wfm);
        assert_eq!(Mode::from_name("cwr"), Mode::CwL);
        assert_eq!(Mode::from_name("cw"), Mode::Cw);
        assert_eq!(Mode::from_name("SPEC"), Mode::Other);
    }

    #[test]
    fn plain_cw_matches_either_sideband() {
        assert!(Mode::Cw.matches(Mode::CwU) && Mode::CwL.matches(Mode::Cw));
        assert!(!Mode::CwL.matches(Mode::CwU));
        assert!(Mode::Usb.matches(Mode::Usb) && !Mode::Cw.matches(Mode::Usb));
    }
}
