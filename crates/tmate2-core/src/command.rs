//! Backend-independent requests to the radio. Each backend translates these
//! into its own protocol (TCI, CAT, ...). Values are absolute, not deltas, so
//! a dropped message can't leave the radio drifting.

use crate::state::{Mode, RadioState};

#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// Set VFO `vfo` (0 = A, 1 = B) of `trx` to `hz`.
    Vfo {
        trx: usize,
        vfo: usize,
        hz: u64,
    },
    Mode {
        trx: usize,
        mode: Mode,
    },
    Filter {
        trx: usize,
        low: i32,
        high: i32,
    },
    /// Master volume in dB (-60 .. 0).
    Volume {
        db: f32,
    },
    Mute(bool),
    Drive {
        trx: usize,
        pct: u8,
    },
    Squelch {
        trx: usize,
        enable: bool,
    },
    SquelchLevel {
        trx: usize,
        db: i32,
    },
    AgcGain {
        trx: usize,
        value: i32,
    },
    Nr {
        trx: usize,
        on: bool,
    },
    Nb {
        trx: usize,
        on: bool,
    },
    Anf {
        trx: usize,
        on: bool,
    },
    Notch {
        trx: usize,
        on: bool,
    },
    Rit {
        trx: usize,
        on: bool,
    },
    RitOffset {
        trx: usize,
        hz: i32,
    },
    Xit {
        trx: usize,
        on: bool,
    },
    XitOffset {
        trx: usize,
        hz: i32,
    },
    Split {
        trx: usize,
        on: bool,
    },
    Lock {
        trx: usize,
        on: bool,
    },
    /// Transmit (MOX).
    Mox {
        trx: usize,
        on: bool,
    },
    Tune {
        trx: usize,
        on: bool,
    },
    AgcMode {
        trx: usize,
        /// off, long, slow, normal, fast or custom.
        mode: String,
    },
    /// Switch a receiver on or off (the first is always on).
    RxEnable {
        trx: usize,
        on: bool,
    },
    /// Switch the second channel (sub-receiver) of a receiver on or off.
    SubRx {
        trx: usize,
        on: bool,
    },
    Apf {
        trx: usize,
        on: bool,
    },
    Binaural {
        trx: usize,
        on: bool,
    },
    /// Mute one receiver.
    RxMute {
        trx: usize,
        on: bool,
    },
    /// One receiver's volume in dB (main channel).
    RxVolume {
        trx: usize,
        db: f32,
    },
    Monitor(bool),
    /// Monitor volume in dB (-60 .. 0).
    MonVolume {
        db: f32,
    },
    /// CW keyer speed in words per minute.
    CwSpeed(u32),
    TuneDrive {
        trx: usize,
        pct: u8,
    },
    /// Start (true) or stop the radio.
    Power(bool),
    /// TCI-X `af_gain`: the radio's own AF gain, 0-100.
    AfGain {
        trx: usize,
        pct: i32,
    },
    /// TCI-X `rf_gain`, 0-100.
    RfGain {
        trx: usize,
        pct: i32,
    },
    /// TCI-X `squelch`: squelch knob position, 0-100.
    SquelchKnob {
        trx: usize,
        pct: i32,
    },
    /// TCI-X `filter`: a named filter preset (receiver channel 0).
    FilterPreset {
        trx: usize,
        name: String,
    },
    /// TCI-X `dial_lock`: the radio's own dial lock.
    DialLock(bool),
    /// TCI-X `notch`: manual notch on / off at an audio frequency.
    NotchAt {
        trx: usize,
        on: bool,
        hz: i32,
    },
    /// TCI-X `atu_tune`: start an antenna tuner cycle. It transmits.
    AtuTune {
        trx: usize,
    },
    /// TCI-X `band`: the radio's band key, using its own band stack.
    Band {
        trx: usize,
        name: String,
    },
    /// TCI-X `vfo_select`: make VFO `ch` (0 = A, 1 = B) the active one.
    VfoSelect {
        trx: usize,
        ch: usize,
    },
    /// TCI-X `preamp`: one of the settings the server lists.
    Preamp {
        trx: usize,
        value: String,
    },
    /// TCI-X `attenuator`: one of the steps the server lists, in dB.
    Attenuator {
        trx: usize,
        value: String,
    },
    /// TCI-X `control_request`: ask for control (granted at once when nobody
    /// has it).
    ControlRequest,
    /// TCI-X `control_release`: give control up.
    ControlRelease,
    /// Ask for periodic signal-strength reports.
    EnableMeters,
    /// Ask for periodic transmit power / SWR reports.
    EnableTxMeters,
    /// Protocol-specific text passed through unchanged (escape hatch).
    Raw(String),
    /// Ask the radio what `target` is now (standard TCI's read form, e.g.
    /// `rx_nr_enable:0;`), when it didn't report the result of a command.
    Query(Target),
}

/// What a command sets: see [`Command::target`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Target {
    pub name: &'static str,
    pub trx: Option<usize>,
    pub vfo: Option<usize>,
}

/// What a value's reports are told apart by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scope {
    /// One for the radio (`volume`).
    Radio,
    /// One per transceiver, the first argument (`rx_nr_enable:0`).
    Trx,
    /// One per transceiver and VFO, the first two (`vfo:0,1`).
    Vfo,
}

/// Plain TCI commands whose read form is the command without its value
/// (`rx_nr_enable:0;`, `volume;`), answered with a report of the value.
/// Not `start` / `stop`, which a read would set off, nor TCI-X's own.
/// Not `rx_filter_band`: piHPSDR answers a read with the mode's preset,
/// not the edges in use, so a filter knob would jump back to the preset.
/// The filter knobs go by the echoes of what they send.
const READABLE: &[(&str, Scope)] = &[
    ("vfo", Scope::Vfo),
    ("modulation", Scope::Trx),
    ("volume", Scope::Radio),
    ("mute", Scope::Radio),
    ("drive", Scope::Trx),
    ("sql_enable", Scope::Trx),
    ("sql_level", Scope::Trx),
    ("agc_gain", Scope::Trx),
    ("agc_mode", Scope::Trx),
    ("rx_nr_enable", Scope::Trx),
    ("rx_nb_enable", Scope::Trx),
    ("rx_anf_enable", Scope::Trx),
    ("rx_nf_enable", Scope::Trx),
    ("rx_apf_enable", Scope::Trx),
    ("rx_bin_enable", Scope::Trx),
    ("rit_enable", Scope::Trx),
    ("rit_offset", Scope::Trx),
    ("xit_enable", Scope::Trx),
    ("xit_offset", Scope::Trx),
    ("split_enable", Scope::Trx),
    ("lock", Scope::Trx),
    ("trx", Scope::Trx),
    ("tune", Scope::Trx),
    ("rx_enable", Scope::Trx),
    ("rx_channel_enable", Scope::Trx),
    ("rx_mute", Scope::Trx),
    ("rx_volume", Scope::Trx),
    ("tune_drive", Scope::Trx),
    ("mon_enable", Scope::Radio),
    ("mon_volume", Scope::Radio),
    ("cw_keyer_speed", Scope::Radio),
];

impl Target {
    /// A target for a readable value (see [`READABLE`]).
    fn readable_as(name: &str, trx: Option<usize>, vfo: Option<usize>) -> Option<Target> {
        let (name, scope) = READABLE.iter().find(|(n, _)| *n == name)?;
        Some(match scope {
            Scope::Radio => Target {
                name,
                trx: None,
                vfo: None,
            },
            Scope::Trx => Target {
                name,
                trx: Some(trx?),
                vfo: None,
            },
            Scope::Vfo => Target {
                name,
                trx: Some(trx?),
                vfo: Some(vfo?),
            },
        })
    }

    /// The readable value a server message `name:args;` reports, if it is
    /// one: what [`RadioState::heard`] keeps the time of.
    pub fn of_report(name: &str, args: &[String]) -> Option<Target> {
        let arg = |i: usize| args.get(i).and_then(|a| a.trim().parse::<usize>().ok());
        Target::readable_as(name, arg(0), arg(1))
    }

    /// `vfo` (0 = A) of `trx`, as a readable value.
    pub fn vfo(trx: usize, vfo: usize) -> Target {
        Target::readable_as("vfo", Some(trx), Some(vfo)).expect("vfo is readable")
    }

    /// A readable value of `trx` by its TCI name (`rx_nr_enable`).
    pub fn of_trx(name: &str, trx: usize) -> Option<Target> {
        Target::readable_as(name, Some(trx), None)
    }

    /// Plain TCI can ask the radio for this (see [`READABLE`]).
    pub fn readable(&self) -> bool {
        Target::readable_as(self.name, self.trx, self.vfo).as_ref() == Some(self)
    }

    /// Whether a server message (`name:args;`) reports this: the same
    /// command, and where we know them the same transceiver (first
    /// argument) and VFO (second).
    pub fn reported_by(&self, name: &str, args: &[String]) -> bool {
        let arg = |i: usize| args.get(i).and_then(|a| a.trim().parse::<usize>().ok());
        self.name == name
            && self.trx.is_none_or(|t| arg(0) == Some(t))
            && self.vfo.is_none_or(|v| arg(1) == Some(v))
    }
}

impl Command {
    /// The TCI command name this is sent as, for checking against the
    /// server's manifest. `None` for raw text, which is sent as written.
    pub fn wire_name(&self) -> Option<&'static str> {
        Some(match self {
            Command::Vfo { .. } => "vfo",
            Command::Mode { .. } => "modulation",
            Command::Filter { .. } => "rx_filter_band",
            Command::Volume { .. } => "volume",
            Command::Mute(_) => "mute",
            Command::Drive { .. } => "drive",
            Command::Squelch { .. } => "sql_enable",
            Command::SquelchLevel { .. } => "sql_level",
            Command::AgcGain { .. } => "agc_gain",
            Command::Nr { .. } => "rx_nr_enable",
            Command::Nb { .. } => "rx_nb_enable",
            Command::Anf { .. } => "rx_anf_enable",
            Command::Notch { .. } => "rx_nf_enable",
            Command::Rit { .. } => "rit_enable",
            Command::RitOffset { .. } => "rit_offset",
            Command::Xit { .. } => "xit_enable",
            Command::XitOffset { .. } => "xit_offset",
            Command::Split { .. } => "split_enable",
            Command::Lock { .. } => "lock",
            Command::Mox { .. } => "trx",
            Command::Tune { .. } => "tune",
            Command::AgcMode { .. } => "agc_mode",
            Command::RxEnable { .. } => "rx_enable",
            Command::SubRx { .. } => "rx_channel_enable",
            Command::Apf { .. } => "rx_apf_enable",
            Command::Binaural { .. } => "rx_bin_enable",
            Command::RxMute { .. } => "rx_mute",
            Command::RxVolume { .. } => "rx_volume",
            Command::Monitor(_) => "mon_enable",
            Command::MonVolume { .. } => "mon_volume",
            Command::CwSpeed(_) => "cw_keyer_speed",
            Command::TuneDrive { .. } => "tune_drive",
            Command::Power(true) => "start",
            Command::Power(false) => "stop",
            Command::AfGain { .. } => "af_gain",
            Command::RfGain { .. } => "rf_gain",
            Command::SquelchKnob { .. } => "squelch",
            Command::FilterPreset { .. } => "filter",
            Command::DialLock(_) => "dial_lock",
            Command::NotchAt { .. } => "notch",
            Command::AtuTune { .. } => "atu_tune",
            Command::Band { .. } => "band",
            Command::VfoSelect { .. } => "vfo_select",
            Command::Preamp { .. } => "preamp",
            Command::Attenuator { .. } => "attenuator",
            Command::ControlRequest => "control_request",
            Command::ControlRelease => "control_release",
            Command::EnableMeters => "rx_sensors_enable",
            Command::EnableTxMeters => "tx_sensors_enable",
            Command::Raw(_) | Command::Query(_) => return None,
        })
    }

    /// The transceiver this command is for; `None` for the radio-wide ones
    /// (master volume, monitor, CW speed, power, ...). On the wire it is the
    /// first argument.
    pub fn trx(&self) -> Option<usize> {
        use Command as C;
        match self {
            C::Vfo { trx, .. }
            | C::Mode { trx, .. }
            | C::Filter { trx, .. }
            | C::Drive { trx, .. }
            | C::Squelch { trx, .. }
            | C::SquelchLevel { trx, .. }
            | C::AgcGain { trx, .. }
            | C::Nr { trx, .. }
            | C::Nb { trx, .. }
            | C::Anf { trx, .. }
            | C::Notch { trx, .. }
            | C::Rit { trx, .. }
            | C::RitOffset { trx, .. }
            | C::Xit { trx, .. }
            | C::XitOffset { trx, .. }
            | C::Split { trx, .. }
            | C::Lock { trx, .. }
            | C::Mox { trx, .. }
            | C::Tune { trx, .. }
            | C::AgcMode { trx, .. }
            | C::RxEnable { trx, .. }
            | C::SubRx { trx, .. }
            | C::Apf { trx, .. }
            | C::Binaural { trx, .. }
            | C::RxMute { trx, .. }
            | C::RxVolume { trx, .. }
            | C::TuneDrive { trx, .. }
            | C::AfGain { trx, .. }
            | C::RfGain { trx, .. }
            | C::SquelchKnob { trx, .. }
            | C::FilterPreset { trx, .. }
            | C::NotchAt { trx, .. }
            | C::AtuTune { trx }
            | C::Band { trx, .. }
            | C::VfoSelect { trx, .. }
            | C::Preamp { trx, .. }
            | C::Attenuator { trx, .. } => Some(*trx),
            C::Volume { .. }
            | C::Mute(_)
            | C::Monitor(_)
            | C::MonVolume { .. }
            | C::CwSpeed(_)
            | C::Power(_)
            | C::DialLock(_)
            | C::ControlRequest
            | C::ControlRelease
            | C::EnableMeters
            | C::EnableTxMeters
            | C::Raw(_)
            | C::Query(_) => None,
        }
    }

    /// What a newer value replaces, and what the pacer and the record of
    /// values asked for go by: the wire name, the transceiver, and for the
    /// dial the VFO. `None` for raw text and reads.
    pub fn target(&self) -> Option<Target> {
        let vfo = match self {
            Command::Vfo { vfo, .. } => Some(*vfo),
            _ => None,
        };
        self.wire_name().map(|name| Target {
            name,
            trx: self.trx(),
            vfo,
        })
    }

    /// A value set by turning a knob, one click at a time. The knob carries
    /// on from the value it asked for until the radio has answered (so the
    /// echo of an earlier click can't pull it back), and of several waiting
    /// to be sent only the newest goes.
    pub fn knob_value(&self) -> bool {
        matches!(
            self,
            Command::Filter { .. }
                | Command::Volume { .. }
                | Command::Drive { .. }
                | Command::SquelchLevel { .. }
                | Command::AgcGain { .. }
                | Command::RitOffset { .. }
                | Command::XitOffset { .. }
                | Command::RxVolume { .. }
                | Command::MonVolume { .. }
                | Command::CwSpeed(_)
                | Command::TuneDrive { .. }
                | Command::AfGain { .. }
                | Command::RfGain { .. }
                | Command::SquelchKnob { .. }
                | Command::FilterPreset { .. }
        )
    }

    /// A command that sets something the radio reports back, so we wait for
    /// that report rather than assume the command worked (see
    /// [`RadioState::expect`]). Not starting or stopping the radio, and not
    /// those that only lead to other reports (a band key, a tuner cycle).
    pub fn awaits_report(&self) -> bool {
        !matches!(
            self,
            Command::Vfo { vfo: 2.., .. }
                | Command::Power(_)
                | Command::AtuTune { .. }
                | Command::Band { .. }
                | Command::ControlRequest
                | Command::ControlRelease
                | Command::EnableMeters
                | Command::EnableTxMeters
                | Command::Raw(_)
                | Command::Query(_)
        )
    }

    /// The read that asks the radio what this command set, for the plain TCI
    /// commands that have one (see [`READABLE`]).
    pub fn query(&self) -> Option<Command> {
        self.target().filter(Target::readable).map(Command::Query)
    }

    /// Unkeying is never refused (TCI-X §4.4), so it is always sent.
    pub fn is_unkey(&self) -> bool {
        matches!(
            self,
            Command::Mox { on: false, .. } | Command::Tune { on: false, .. }
        )
    }
}

impl RadioState {
    /// Whether the server takes this command (see [`RadioState::can_send`]).
    /// Raw text and unkeying are always allowed.
    pub fn allows(&self, cmd: &Command) -> bool {
        cmd.is_unkey() || cmd.wire_name().is_none_or(|n| self.can_send(n))
    }

    /// A knob value (see [`Command::knob_value`]) that is already what the
    /// state holds, as at the end of a knob's travel: nothing to send.
    pub fn holds(&self, cmd: &Command) -> bool {
        let t = |trx: &usize| self.trx(*trx);
        match cmd {
            Command::Filter { trx, low, high } => t(trx).is_some_and(|x| x.filter == Some((*low, *high))),
            Command::Volume { db } => self.volume_db == Some(*db),
            Command::Drive { trx, pct } => t(trx).is_some_and(|x| x.drive == Some(*pct)),
            Command::SquelchLevel { trx, db } => t(trx).is_some_and(|x| x.sql_level == Some(*db)),
            Command::AgcGain { trx, value } => t(trx).is_some_and(|x| x.agc_gain == Some(*value)),
            Command::RitOffset { trx, hz } => t(trx).is_some_and(|x| x.rit_offset == *hz),
            Command::XitOffset { trx, hz } => t(trx).is_some_and(|x| x.xit_offset == *hz),
            Command::RxVolume { trx, db } => t(trx).is_some_and(|x| x.rx_volume_db == Some(*db)),
            Command::MonVolume { db } => self.mon_volume_db == Some(*db),
            Command::CwSpeed(wpm) => self.cw_speed == Some(*wpm),
            Command::TuneDrive { trx, pct } => t(trx).is_some_and(|x| x.tune_drive == Some(*pct)),
            Command::AfGain { trx, pct } => t(trx).is_some_and(|x| x.af_gain == Some(*pct)),
            Command::RfGain { trx, pct } => t(trx).is_some_and(|x| x.rf_gain == Some(*pct)),
            Command::SquelchKnob { trx, pct } => t(trx).is_some_and(|x| x.squelch == Some(*pct)),
            Command::FilterPreset { trx, name } => {
                t(trx).is_some_and(|x| x.filter_preset.as_deref() == Some(name.as_str()))
            }
            _ => false,
        }
    }

    /// Set what `cmd` sets, as if the radio had reported it. Used only on a
    /// [`RadioState::working`] copy, where the knobs carry on from the
    /// values they have asked for: the state shown is what the radio reports.
    pub fn apply(&mut self, cmd: &Command) {
        match cmd {
            &Command::Vfo { trx, vfo, hz } if vfo < 2 => self.trx_mut(trx).vfo[vfo] = hz,
            &Command::Mode { trx, mode } => self.trx_mut(trx).mode = Some(mode),
            &Command::Filter { trx, low, high } => self.trx_mut(trx).filter = Some((low, high)),
            &Command::Volume { db } => self.volume_db = Some(db),
            &Command::Mute(on) => self.mute = on,
            &Command::Drive { trx, pct } => self.trx_mut(trx).drive = Some(pct),
            &Command::Squelch { trx, enable } => self.trx_mut(trx).sql_enable = enable,
            &Command::SquelchLevel { trx, db } => self.trx_mut(trx).sql_level = Some(db),
            &Command::AgcGain { trx, value } => self.trx_mut(trx).agc_gain = Some(value),
            &Command::Nr { trx, on } => self.trx_mut(trx).nr = on,
            &Command::Nb { trx, on } => self.trx_mut(trx).nb = on,
            &Command::Anf { trx, on } => self.trx_mut(trx).anf = on,
            &Command::Notch { trx, on } => self.trx_mut(trx).notch = on,
            &Command::Rit { trx, on } => self.trx_mut(trx).rit = on,
            &Command::RitOffset { trx, hz } => self.trx_mut(trx).rit_offset = hz,
            &Command::Xit { trx, on } => self.trx_mut(trx).xit = on,
            &Command::XitOffset { trx, hz } => self.trx_mut(trx).xit_offset = hz,
            &Command::Split { trx, on } => self.trx_mut(trx).split = on,
            &Command::Lock { trx, on } => self.trx_mut(trx).lock = on,
            &Command::Mox { trx, on } => self.trx_mut(trx).tx = on,
            &Command::Tune { trx, on } => self.trx_mut(trx).tune = on,
            &Command::AgcMode { trx, ref mode } => self.trx_mut(trx).agc_mode = Some(mode.clone()),
            &Command::RxEnable { trx, on } => self.trx_mut(trx).rx_enable = on,
            &Command::SubRx { trx, on } => self.trx_mut(trx).sub_rx = on,
            &Command::Apf { trx, on } => self.trx_mut(trx).apf = on,
            &Command::Binaural { trx, on } => self.trx_mut(trx).binaural = on,
            &Command::RxMute { trx, on } => self.trx_mut(trx).rx_mute = on,
            &Command::RxVolume { trx, db } => self.trx_mut(trx).rx_volume_db = Some(db),
            &Command::Monitor(on) => self.monitor = on,
            &Command::MonVolume { db } => self.mon_volume_db = Some(db),
            &Command::CwSpeed(wpm) => self.cw_speed = Some(wpm),
            &Command::TuneDrive { trx, pct } => self.trx_mut(trx).tune_drive = Some(pct),
            &Command::Power(on) => self.power = Some(on),
            &Command::AfGain { trx, pct } => self.trx_mut(trx).af_gain = Some(pct),
            &Command::RfGain { trx, pct } => self.trx_mut(trx).rf_gain = Some(pct),
            &Command::SquelchKnob { trx, pct } => self.trx_mut(trx).squelch = Some(pct),
            Command::FilterPreset { trx, name } => self.trx_mut(*trx).filter_preset = Some(name.clone()),
            &Command::DialLock(on) => self.dial_lock = on,
            &Command::VfoSelect { trx, ch } => self.trx_mut(trx).vfo_selected = Some(ch),
            Command::Preamp { trx, value } => self.trx_mut(*trx).preamp = Some(value.clone()),
            Command::Attenuator { trx, value } => self.trx_mut(*trx).attenuator = Some(value.clone()),
            &Command::NotchAt { trx, on, hz } => {
                let t = self.trx_mut(trx);
                t.notch = on;
                t.notch_hz = Some(hz);
            }
            // The radio reports what these lead to (keying, a new frequency).
            Command::Vfo { .. }
            | Command::AtuTune { .. }
            | Command::Band { .. }
            | Command::ControlRequest
            | Command::ControlRelease
            | Command::EnableMeters
            | Command::EnableTxMeters
            | Command::Raw(_)
            | Command::Query(_) => {}
        }
    }

    /// The radio answered neither `cmd` nor a read of it: what we last heard
    /// may be out of date, so show less (a lamp off, a value unknown) rather
    /// than guess. The frequency, offsets and keying are left as last
    /// reported.
    pub fn forget(&mut self, cmd: &Command) {
        match *cmd {
            Command::Mode { trx, .. } => self.trx_mut(trx).mode = None,
            Command::Filter { trx, .. } => self.trx_mut(trx).filter = None,
            Command::Volume { .. } => self.volume_db = None,
            Command::Mute(_) => self.mute = false,
            Command::Drive { trx, .. } => self.trx_mut(trx).drive = None,
            Command::Squelch { trx, .. } => self.trx_mut(trx).sql_enable = false,
            Command::SquelchLevel { trx, .. } => self.trx_mut(trx).sql_level = None,
            Command::AgcGain { trx, .. } => self.trx_mut(trx).agc_gain = None,
            Command::Nr { trx, .. } => self.trx_mut(trx).nr = false,
            Command::Nb { trx, .. } => self.trx_mut(trx).nb = false,
            Command::Anf { trx, .. } => self.trx_mut(trx).anf = false,
            Command::Notch { trx, .. } => self.trx_mut(trx).notch = false,
            Command::Rit { trx, .. } => self.trx_mut(trx).rit = false,
            Command::Xit { trx, .. } => self.trx_mut(trx).xit = false,
            Command::Split { trx, .. } => self.trx_mut(trx).split = false,
            Command::Lock { trx, .. } => self.trx_mut(trx).lock = false,
            Command::AgcMode { trx, .. } => self.trx_mut(trx).agc_mode = None,
            Command::SubRx { trx, .. } => self.trx_mut(trx).sub_rx = false,
            Command::Apf { trx, .. } => self.trx_mut(trx).apf = false,
            Command::Binaural { trx, .. } => self.trx_mut(trx).binaural = false,
            Command::RxMute { trx, .. } => self.trx_mut(trx).rx_mute = false,
            Command::RxVolume { trx, .. } => self.trx_mut(trx).rx_volume_db = None,
            Command::Monitor(_) => self.monitor = false,
            Command::MonVolume { .. } => self.mon_volume_db = None,
            Command::CwSpeed(_) => self.cw_speed = None,
            Command::TuneDrive { trx, .. } => self.trx_mut(trx).tune_drive = None,
            _ => {}
        }
    }
}
