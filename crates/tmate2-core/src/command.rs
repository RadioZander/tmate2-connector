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
    /// Bring the control software's window to the front.
    Focus,
    /// Ask for periodic signal-strength reports.
    EnableMeters,
    /// Ask for periodic transmit power / SWR reports.
    EnableTxMeters,
    /// Protocol-specific text passed through unchanged (escape hatch).
    Raw(String),
}

impl RadioState {
    /// Optimistically apply a command we are sending, so the display updates
    /// at once. The radio's own reports overwrite this when they arrive.
    pub fn apply(&mut self, cmd: &Command) {
        match *cmd {
            Command::Vfo { trx, vfo, hz } if vfo < 2 => self.trx_mut(trx).vfo[vfo] = hz,
            Command::Mode { trx, mode } => self.trx_mut(trx).mode = Some(mode),
            Command::Filter { trx, low, high } => self.trx_mut(trx).filter = Some((low, high)),
            Command::Volume { db } => self.volume_db = Some(db),
            Command::Mute(on) => self.mute = on,
            Command::Drive { trx, pct } => self.trx_mut(trx).drive = Some(pct),
            Command::Squelch { trx, enable } => self.trx_mut(trx).sql_enable = enable,
            Command::SquelchLevel { trx, db } => self.trx_mut(trx).sql_level = Some(db),
            Command::AgcGain { trx, value } => self.trx_mut(trx).agc_gain = Some(value),
            Command::Nr { trx, on } => self.trx_mut(trx).nr = on,
            Command::Nb { trx, on } => self.trx_mut(trx).nb = on,
            Command::Anf { trx, on } => self.trx_mut(trx).anf = on,
            Command::Notch { trx, on } => self.trx_mut(trx).notch = on,
            Command::Rit { trx, on } => self.trx_mut(trx).rit = on,
            Command::RitOffset { trx, hz } => self.trx_mut(trx).rit_offset = hz,
            Command::Xit { trx, on } => self.trx_mut(trx).xit = on,
            Command::XitOffset { trx, hz } => self.trx_mut(trx).xit_offset = hz,
            Command::Split { trx, on } => self.trx_mut(trx).split = on,
            Command::Lock { trx, on } => self.trx_mut(trx).lock = on,
            Command::Mox { trx, on } => self.trx_mut(trx).tx = on,
            Command::Tune { trx, on } => self.trx_mut(trx).tune = on,
            Command::AgcMode { trx, ref mode } => self.trx_mut(trx).agc_mode = Some(mode.clone()),
            Command::RxEnable { trx, on } => self.trx_mut(trx).rx_enable = on,
            Command::SubRx { trx, on } => self.trx_mut(trx).sub_rx = on,
            Command::Apf { trx, on } => self.trx_mut(trx).apf = on,
            Command::Binaural { trx, on } => self.trx_mut(trx).binaural = on,
            Command::RxMute { trx, on } => self.trx_mut(trx).rx_mute = on,
            Command::RxVolume { trx, db } => self.trx_mut(trx).rx_volume_db = Some(db),
            Command::Monitor(on) => self.monitor = on,
            Command::MonVolume { db } => self.mon_volume_db = Some(db),
            Command::CwSpeed(wpm) => self.cw_speed = Some(wpm),
            Command::TuneDrive { trx, pct } => self.trx_mut(trx).tune_drive = Some(pct),
            Command::Power(on) => self.power = Some(on),
            Command::Vfo { .. }
            | Command::Focus
            | Command::EnableMeters
            | Command::EnableTxMeters
            | Command::Raw(_) => {}
        }
    }
}
