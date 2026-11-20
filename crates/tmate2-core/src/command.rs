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
    /// TCI-X `control_request`: ask for control (granted at once when nobody
    /// has it).
    ControlRequest,
    /// TCI-X `control_release`: give control up.
    ControlRelease,
    /// Ask for periodic signal-strength reports.
    EnableMeters,
    /// Protocol-specific text passed through unchanged (escape hatch).
    Raw(String),
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
            Command::AfGain { .. } => "af_gain",
            Command::RfGain { .. } => "rf_gain",
            Command::SquelchKnob { .. } => "squelch",
            Command::FilterPreset { .. } => "filter",
            Command::DialLock(_) => "dial_lock",
            Command::NotchAt { .. } => "notch",
            Command::AtuTune { .. } => "atu_tune",
            Command::Band { .. } => "band",
            Command::VfoSelect { .. } => "vfo_select",
            Command::ControlRequest => "control_request",
            Command::ControlRelease => "control_release",
            Command::EnableMeters => "rx_sensors_enable",
            Command::Raw(_) => return None,
        })
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

    /// Optimistically apply a command we are sending, so the display updates
    /// at once. The radio's own reports overwrite this when they arrive.
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
            &Command::AfGain { trx, pct } => self.trx_mut(trx).af_gain = Some(pct),
            &Command::RfGain { trx, pct } => self.trx_mut(trx).rf_gain = Some(pct),
            &Command::SquelchKnob { trx, pct } => self.trx_mut(trx).squelch = Some(pct),
            Command::FilterPreset { trx, name } => self.trx_mut(*trx).filter_preset = Some(name.clone()),
            &Command::DialLock(on) => self.dial_lock = on,
            &Command::VfoSelect { trx, ch } => self.trx_mut(trx).vfo_selected = Some(ch),
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
            | Command::Raw(_) => {}
        }
    }
}
