//! TCI (Expert Electronics' Transceiver Control Interface) text protocol:
//! parsing server messages into [`RadioState`] and encoding [`Command`]s.
//!
//! Messages look like `name:arg,arg,...;`. Names are case-insensitive.

use tmate2_core::state::Mode;
use tmate2_core::{Command, Manifest, RadioState};

/// One parsed message: lower-case name plus arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Msg {
    pub name: String,
    pub args: Vec<String>,
}

/// Split a WebSocket text frame into messages (a frame may hold several).
pub fn split(frame: &str) -> impl Iterator<Item = Msg> + '_ {
    frame.split(';').filter_map(parse)
}

/// Parse one message (without or with its trailing ';').
pub fn parse(text: &str) -> Option<Msg> {
    let text = text.trim().trim_end_matches(';').trim();
    if text.is_empty() {
        return None;
    }
    let (name, rest) = text.split_once(':').unwrap_or((text, ""));
    let args = if rest.is_empty() {
        vec![]
    } else {
        rest.split(',').map(|a| a.trim().to_string()).collect()
    };
    Some(Msg {
        name: name.trim().to_ascii_lowercase(),
        args,
    })
}

/// Percent-encode a TCI-X string argument (§2): `%`, `:`, `,`, `;` and
/// control characters.
pub fn pct_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if matches!(c, '%' | ':' | ',' | ';') || c.is_control() {
            let mut buf = [0u8; 4];
            for b in c.encode_utf8(&mut buf).bytes() {
                out.push_str(&format!("%{b:02X}"));
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Decode a percent-encoded TCI-X string argument. Malformed escapes are
/// kept as they are.
pub fn pct_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |j: usize| bytes.get(j).and_then(|b| (*b as char).to_digit(16));
        match (bytes[i], hex(i + 1), hex(i + 2)) {
            (b'%', Some(h), Some(l)) => {
                out.push((h * 16 + l) as u8);
                i += 3;
            }
            (b, ..) => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// What to flash on the LCD for a TCI-X `error` code (§5.1). The display
/// has seven segments: no M, so no "timeout".
pub fn error_text(code: &str) -> &'static str {
    match code {
        "busy" => "busy",
        "control" => "ctrl",
        "access" => "denied",
        "range" => "range",
        "unsupported" => "no",
        "radio" => "radio",
        "duplex" => "duplex",
        "tx_active" => "tx on",
        "tx_lock" => "tx lock",
        "tx_audio" => "tx audio",
        "tx_timer" => "tx tot",
        "tx_where" => "tx freq",
        "device" => "device",
        "interlock" => "interlock",
        "timeout" => "no reply",
        "idle" => "idle",
        _ => "error",
    }
}

fn b(s: &str) -> Option<bool> {
    match s.to_ascii_lowercase().as_str() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

/// Apply a server message to the state. Returns true if it was understood.
/// Every report is taken as it comes: the state is what the radio says.
pub fn apply(state: &mut RadioState, m: &Msg) -> bool {
    let known = apply_report(state, m);
    state.reported(&m.name, &m.args);
    known
}

fn apply_report(state: &mut RadioState, m: &Msg) -> bool {
    let a: Vec<&str> = m.args.iter().map(String::as_str).collect();
    let n = |i: usize| a.get(i).and_then(|s| s.parse::<i64>().ok());
    let f = |i: usize| a.get(i).and_then(|s| s.parse::<f32>().ok());
    let flag = |i: usize| a.get(i).and_then(|s| b(s));
    let trx = n(0).map(|v| v.max(0) as usize);

    macro_rules! set_trx_bool {
        ($field:ident) => {
            if let (Some(t), Some(v)) = (trx, flag(1)) {
                state.trx_mut(t).$field = v;
            }
        };
    }

    match m.name.as_str() {
        "protocol" => state.protocol = Some(m.args.join(",")),
        "device" => state.device = a.first().map(|s| s.to_string()),
        "modulations_list" => {
            state.modes = a.iter().map(|s| Mode::from_name(s)).collect();
            state.mode_names = a.iter().map(|s| s.to_string()).collect();
        }
        "vfo_limits" => {
            if let (Some(lo), Some(hi)) = (n(0), n(1)) {
                state.vfo_limits = Some((lo.max(0) as u64, hi.max(0) as u64));
            }
        }
        "vfo" => {
            if let (Some(t), Some(ch), Some(hz)) = (trx, n(1), n(2)) {
                state.remote_vfo(t, ch.max(0) as usize, hz.max(0) as u64);
            }
        }
        "modulation" => {
            if let (Some(t), Some(name)) = (trx, a.get(1)) {
                state.trx_mut(t).mode = Some(Mode::from_name(name));
            }
        }
        "rx_filter_band" => {
            if let (Some(t), Some(lo), Some(hi)) = (trx, n(1), n(2)) {
                state.trx_mut(t).filter = Some((lo as i32, hi as i32));
            }
        }
        // rx_channel_sensors:trx,channel,dBm (channel 0 = main receiver)
        "rx_channel_sensors" => {
            if let (Some(t), Some(0), Some(dbm)) = (trx, n(1), f(2)) {
                state.trx_mut(t).rx_dbm = Some(dbm);
            }
        }
        // Older form: rx_sensors:trx,dBm. Only used if the channel form never arrives.
        "rx_sensors" => {}
        "rx_smeter" => {
            if let (Some(t), Some(0), Some(dbm)) = (trx, n(1), f(2)) {
                state.trx_mut(t).rx_dbm = Some(dbm);
            }
        }
        // tx_sensors:trx,mic_dBm,power_W,peak_W,swr
        "tx_sensors" => {
            if let (Some(t), Some(peak), Some(swr)) = (trx, f(3), f(4)) {
                let x = state.trx_mut(t);
                x.tx_power_w = Some(peak.max(0.0));
                x.tx_swr = Some(swr.max(1.0));
            }
        }
        "rx_nr_enable" => set_trx_bool!(nr),
        "rx_nb_enable" => set_trx_bool!(nb),
        "rx_anf_enable" => set_trx_bool!(anf),
        "rx_nf_enable" => set_trx_bool!(notch),
        "rit_enable" => set_trx_bool!(rit),
        "xit_enable" => set_trx_bool!(xit),
        "split_enable" => set_trx_bool!(split),
        "lock" => set_trx_bool!(lock),
        "trx" => set_trx_bool!(tx),
        "tune" => set_trx_bool!(tune),
        "sql_enable" => set_trx_bool!(sql_enable),
        "rit_offset" => {
            if let (Some(t), Some(v)) = (trx, n(1)) {
                state.trx_mut(t).rit_offset = v as i32;
            }
        }
        "xit_offset" => {
            if let (Some(t), Some(v)) = (trx, n(1)) {
                state.trx_mut(t).xit_offset = v as i32;
            }
        }
        "sql_level" => {
            if let (Some(t), Some(v)) = (trx, f(1)) {
                state.trx_mut(t).sql_level = Some(v.round() as i32);
            }
        }
        "agc_gain" => {
            if let (Some(t), Some(v)) = (trx, f(1)) {
                state.trx_mut(t).agc_gain = Some(v.round() as i32);
            }
        }
        "drive" => match (n(0), f(1)) {
            (Some(t), Some(v)) => state.trx_mut(t.max(0) as usize).drive = Some(v.clamp(0.0, 100.0) as u8),
            // TCI 1.x form: drive:value
            (Some(v), None) => state.trx_mut(0).drive = Some(v.clamp(0, 100) as u8),
            _ => {}
        },
        "rx_enable" => set_trx_bool!(rx_enable),
        "rx_apf_enable" => set_trx_bool!(apf),
        "rx_bin_enable" => set_trx_bool!(binaural),
        "rx_mute" => set_trx_bool!(rx_mute),
        // rx_channel_enable:trx,channel,bool (channel 1 = sub-receiver)
        "rx_channel_enable" => {
            if let (Some(t), Some(1), Some(v)) = (trx, n(1), flag(2)) {
                state.trx_mut(t).sub_rx = v;
            }
        }
        // rx_volume:trx,channel,dB
        "rx_volume" => {
            if let (Some(t), Some(0), Some(v)) = (trx, n(1), f(2)) {
                state.trx_mut(t).rx_volume_db = Some(v);
            }
        }
        "agc_mode" => {
            if let (Some(t), Some(mode)) = (trx, a.get(1)) {
                state.trx_mut(t).agc_mode = Some(mode.to_ascii_lowercase());
            }
        }
        "tune_drive" => {
            if let (Some(t), Some(v)) = (trx, f(1)) {
                state.trx_mut(t).tune_drive = Some(v.clamp(0.0, 100.0) as u8);
            }
        }
        "mon_enable" => {
            if let Some(v) = flag(0) {
                state.monitor = v;
            }
        }
        "mon_volume" => state.mon_volume_db = f(0),
        "cw_keyer_speed" => {
            if let Some(v) = n(0) {
                state.cw_speed = Some(v.max(1) as u32);
            }
        }
        "start" => state.power = Some(true),
        "stop" => state.power = Some(false),
        "volume" => state.volume_db = f(0),
        "mute" => {
            if let Some(v) = flag(0) {
                state.mute = v;
            }
        }
        // TCI-X extensions (pushed after cap_end, then on change).
        "af_gain" => {
            if let (Some(t), Some(v)) = (trx, f(1)) {
                state.trx_mut(t).af_gain = Some(v.round() as i32);
            }
        }
        "rf_gain" => {
            if let (Some(t), Some(v)) = (trx, f(1)) {
                state.trx_mut(t).rf_gain = Some(v.round() as i32);
            }
        }
        "squelch" => {
            if let (Some(t), Some(v)) = (trx, f(1)) {
                state.trx_mut(t).squelch = Some(v.round() as i32);
            }
        }
        // filter:trx,channel,name (channel 0 = main receiver)
        "filter" => {
            if let (Some(t), Some(0), Some(name)) = (trx, n(1), a.get(2)) {
                state.trx_mut(t).filter_preset = Some(name.to_string());
            }
        }
        "dial_lock" => {
            if let Some(v) = flag(0) {
                state.dial_lock = v;
            }
        }
        // notch:trx,on,hz
        "notch" => {
            if let (Some(t), Some(on)) = (trx, flag(1)) {
                let x = state.trx_mut(t);
                x.notch = on;
                if let Some(hz) = n(2) {
                    x.notch_hz = Some(hz as i32);
                }
            }
        }
        "receive_only" => state.receive_only = flag(0).unwrap_or(false),
        "radio_state" => state.radio_state = a.first().map(|s| s.to_ascii_lowercase()),
        "preamp" => {
            if let (Some(t), Some(v)) = (trx, a.get(1)) {
                state.trx_mut(t).preamp = Some(pct_decode(v));
            }
        }
        "attenuator" => {
            if let (Some(t), Some(v)) = (trx, a.get(1)) {
                state.trx_mut(t).attenuator = Some(v.to_string());
            }
        }
        "antenna" => {
            if let (Some(t), Some(v)) = (trx, a.get(1)) {
                state.trx_mut(t).antenna = Some(pct_decode(v));
            }
        }
        "rx_antenna" => set_trx_bool!(rx_antenna),
        "pa_enable" => {
            if let (Some(t), Some(v)) = (trx, flag(1)) {
                state.trx_mut(t).pa_enable = Some(v);
            }
        }
        "vfo_select" => {
            if let (Some(t), Some(ch)) = (trx, n(1)) {
                state.trx_mut(t).vfo_selected = Some(ch.clamp(0, 1) as usize);
            }
        }
        // control:<user>,<since>; or control:; when nobody has it (§4.4).
        "control" => {
            state.control = a.first().map(|u| pct_decode(u)).filter(|u| !u.is_empty());
        }
        "control_asked" => state.push_alert("ctrl ask"),
        "control_refused" => state.push_alert("refused"),
        // error:<command>,<code>[,<text>] (§5.1).
        "error" => {
            let (cmd, code) = (a.first().copied().unwrap_or(""), a.get(1).copied().unwrap_or(""));
            tracing::warn!(
                "server refused {cmd}: {code} {}",
                pct_decode(&m.args.get(2..).unwrap_or_default().join(","))
            );
            // We showed transmit at once when keying: the radio isn't.
            if matches!(cmd, "trx" | "tune" | "atu_tune") {
                for t in &mut state.trx {
                    t.tx = false;
                    t.tune = false;
                }
            }
            // A watcher's change names the operator.
            if code == "control" {
                if let Some(op) = a.get(2).map(|u| pct_decode(u)).filter(|u| !u.is_empty()) {
                    state.control = Some(op);
                }
            }
            state.push_alert(error_text(code));
        }
        "ready" => state.connected = true,
        _ => return false,
    }
    true
}

/// Encode a command as TCI text, or `None` if TCI has no equivalent.
pub fn encode(cmd: &Command) -> Option<String> {
    let tf = |v: bool| if v { "true" } else { "false" };
    Some(match cmd {
        Command::Vfo { trx, vfo, hz } => format!("vfo:{trx},{vfo},{hz};"),
        Command::Mode {
            mode: Mode::Other, ..
        } => return None,
        // Plain CW is sent as CWU, which every server we know accepts
        // (piHPSDR takes "CW" as CWU anyway).
        Command::Mode { trx, mode: Mode::Cw } => format!("modulation:{trx},CWU;"),
        Command::Mode { trx, mode } => format!("modulation:{trx},{};", mode.name()),
        Command::Filter { trx, low, high } => format!("rx_filter_band:{trx},{low},{high};"),
        Command::Volume { db } => format!("volume:{db:.2};"),
        Command::Mute(on) => format!("mute:{};", tf(*on)),
        Command::Drive { trx, pct } => format!("drive:{trx},{pct};"),
        Command::Squelch { trx, enable } => format!("sql_enable:{trx},{};", tf(*enable)),
        Command::SquelchLevel { trx, db } => format!("sql_level:{trx},{db};"),
        Command::AgcGain { trx, value } => format!("agc_gain:{trx},{value};"),
        Command::Nr { trx, on } => format!("rx_nr_enable:{trx},{};", tf(*on)),
        Command::Nb { trx, on } => format!("rx_nb_enable:{trx},{};", tf(*on)),
        Command::Anf { trx, on } => format!("rx_anf_enable:{trx},{};", tf(*on)),
        Command::Notch { trx, on } => format!("rx_nf_enable:{trx},{};", tf(*on)),
        Command::Rit { trx, on } => format!("rit_enable:{trx},{};", tf(*on)),
        Command::RitOffset { trx, hz } => format!("rit_offset:{trx},{hz};"),
        Command::Xit { trx, on } => format!("xit_enable:{trx},{};", tf(*on)),
        Command::XitOffset { trx, hz } => format!("xit_offset:{trx},{hz};"),
        Command::Split { trx, on } => format!("split_enable:{trx},{};", tf(*on)),
        Command::Lock { trx, on } => format!("lock:{trx},{};", tf(*on)),
        Command::Mox { trx, on } => format!("trx:{trx},{};", tf(*on)),
        Command::Tune { trx, on } => format!("tune:{trx},{};", tf(*on)),
        Command::AgcMode { trx, mode } => format!("agc_mode:{trx},{mode};"),
        Command::RxEnable { trx, on } => format!("rx_enable:{trx},{};", tf(*on)),
        Command::SubRx { trx, on } => format!("rx_channel_enable:{trx},1,{};", tf(*on)),
        Command::Apf { trx, on } => format!("rx_apf_enable:{trx},{};", tf(*on)),
        Command::Binaural { trx, on } => format!("rx_bin_enable:{trx},{};", tf(*on)),
        Command::RxMute { trx, on } => format!("rx_mute:{trx},{};", tf(*on)),
        Command::RxVolume { trx, db } => format!("rx_volume:{trx},0,{db:.2};"),
        Command::Monitor(on) => format!("mon_enable:{};", tf(*on)),
        Command::MonVolume { db } => format!("mon_volume:{db:.2};"),
        Command::CwSpeed(wpm) => format!("cw_keyer_speed:{wpm};"),
        Command::TuneDrive { trx, pct } => format!("tune_drive:{trx},{pct};"),
        Command::Power(on) => if *on { "start;" } else { "stop;" }.to_string(),
        Command::AfGain { trx, pct } => format!("af_gain:{trx},{pct};"),
        Command::RfGain { trx, pct } => format!("rf_gain:{trx},{pct};"),
        Command::SquelchKnob { trx, pct } => format!("squelch:{trx},{pct};"),
        Command::FilterPreset { trx, name } => format!("filter:{trx},0,{};", pct_encode(name)),
        Command::DialLock(on) => format!("dial_lock:{};", tf(*on)),
        Command::NotchAt { trx, on, hz } => format!("notch:{trx},{},{hz};", tf(*on)),
        Command::AtuTune { trx } => format!("atu_tune:{trx};"),
        Command::Band { trx, name } => format!("band:{trx},{};", pct_encode(name)),
        Command::VfoSelect { trx, ch } => format!("vfo_select:{trx},{ch};"),
        Command::Preamp { trx, value } => format!("preamp:{trx},{};", pct_encode(value)),
        Command::Attenuator { trx, value } => format!("attenuator:{trx},{};", pct_encode(value)),
        Command::ControlRequest => "control_request;".to_string(),
        Command::ControlRelease => "control_release;".to_string(),
        Command::EnableMeters => "rx_sensors_enable:true,200;".to_string(),
        Command::EnableTxMeters => "tx_sensors_enable:true,200;".to_string(),
        // Standard TCI's read form: the command with only its receiver (and
        // VFO, or channel) and no value.
        Command::Query(t) | Command::Poll(t) => match (t.name, t.trx, t.vfo) {
            ("rx_volume", Some(trx), _) => format!("rx_volume:{trx},0;"),
            ("rx_channel_enable", Some(trx), _) => format!("rx_channel_enable:{trx},1;"),
            (name, Some(trx), Some(vfo)) => format!("{name}:{trx},{vfo};"),
            (name, Some(trx), None) => format!("{name}:{trx};"),
            (name, None, _) => format!("{name};"),
        },
        Command::Raw(text) => {
            let t = text.trim();
            if t.ends_with(';') {
                t.to_string()
            } else {
                format!("{t};")
            }
        }
    })
}

/// Encode a command for this server: as [`encode`], but a mode goes by the
/// name the server's own list gives it (`cw`, not `CWU`).
pub fn encode_for(cmd: &Command, state: &RadioState) -> Option<String> {
    match cmd {
        Command::Mode { trx, mode } => Some(format!("modulation:{trx},{};", state.mode_wire_name(*mode))),
        _ => encode(cmd),
    }
}

/// TCI-X negotiation state, fed with every server message. `cap` lines
/// that arrive after `cap_end` keep updating the manifest (TCI-X §4.2).
#[derive(Debug, Default)]
pub struct TcixNegotiation {
    pub replied: bool,
    pub manifest: Manifest,
    pub done: bool,
}

impl TcixNegotiation {
    /// Returns true if the message belonged to the TCI-X handshake or
    /// changed the manifest.
    pub fn feed(&mut self, m: &Msg) -> bool {
        match m.name.as_str() {
            "tcix" => self.replied = true,
            "cap" if self.replied => {
                if !self.manifest.apply(&m.args) {
                    tracing::debug!("unreadable TCI-X cap line: {:?}", m.args);
                }
            }
            "cap_end" if self.replied => self.done = true,
            _ => return false,
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::time::Instant;

    const THETIS_INIT: &str = include_str!("../tests/fixtures/thetis-init.tci");

    #[test]
    fn thetis_init_burst_fills_state() {
        let mut s = RadioState::default();
        for line in THETIS_INIT.lines() {
            for m in split(line) {
                apply(&mut s, &m);
            }
        }
        assert!(s.connected, "ready; seen");
        assert_eq!(s.protocol.as_deref(), Some("ExpertSDR3,2.0"));
        assert_eq!(s.vfo_limits, Some((0, 61_440_000)));
        assert!(s.modes.contains(&Mode::DigU) && s.modes.contains(&Mode::CwL));
        let t = &s.trx[0];
        assert_eq!(t.vfo, [14_074_000, 7_074_000]);
        assert_eq!(t.mode, Some(Mode::DigU));
        assert_eq!(t.filter, Some((100, 3500)));
        assert_eq!(t.drive, Some(10));
        assert_eq!(s.volume_db, Some(-42.0));
        assert!(t.rx_dbm.is_some_and(|d| (-70.0..-60.0).contains(&d)));
        assert!(!t.tx && !t.lock);
        // Controls added for the extra button actions and knob functions.
        assert_eq!(t.agc_mode.as_deref(), Some("off"));
        assert_eq!(s.trx[1].agc_mode.as_deref(), Some("normal"));
        assert!(t.rx_enable && !s.trx[1].rx_enable);
        assert!(!t.sub_rx && !t.apf && !t.binaural && !t.rx_mute);
        assert_eq!(t.rx_volume_db, Some(-1.94));
        assert_eq!(t.tune_drive, Some(28));
        assert_eq!(s.mon_volume_db, Some(-30.0));
        assert_eq!(s.cw_speed, Some(22));
        assert!(!s.monitor);
        assert_eq!(s.power, Some(true));
    }

    /// Init burst from piHPSDR (DL1YCF, 2026-11), one WebSocket frame per
    /// line, kept exactly as sent (two lines have no ';').
    const PIHPSDR_INIT: &str = include_str!("../tests/fixtures/pihpsdr-init.tci");

    #[test]
    fn pihpsdr_init_burst_fills_state() {
        let mut s = RadioState::default();
        for line in PIHPSDR_INIT.lines() {
            for m in split(line) {
                apply(&mut s, &m);
            }
        }
        assert!(s.connected, "ready; seen");
        assert_eq!(s.protocol.as_deref(), Some("ExpertSDR3,2.0"));
        assert_eq!(s.device.as_deref(), Some("SunSDR2QRP"));
        assert_eq!(s.vfo_limits, Some((0, 61_440_000)));
        // "CW" and "FMN" in modulations_list.
        assert!(s.modes.contains(&Mode::Cw) && s.modes.contains(&Mode::Fm));
        let t = &s.trx[0];
        assert_eq!(t.vfo, [14_009_925, 14_010_000]);
        assert_eq!(t.mode, Some(Mode::Usb));
        assert_eq!(t.filter, Some((150, 2850)));
        // Whole dB, -40..0.
        assert_eq!(s.volume_db, Some(-20.0));
        assert_eq!(t.rx_volume_db, Some(-20.0));
        assert_eq!(t.tune_drive, Some(10));
        assert_eq!(t.agc_mode.as_deref(), Some("normal"));
        assert!(t.notch && !t.nr && !t.nb && !t.anf && !t.apf && !t.binaural);
        assert!(t.rx_enable && !t.tx && !t.lock && !t.split && !s.mute);
        assert_eq!(s.cw_speed, Some(16));
        assert_eq!(s.power, Some(true));
        // One receiver only: nothing for a second trx.
        assert_eq!(s.trx.len(), 1);
    }

    #[test]
    fn plain_cw_is_sent_as_cwu() {
        let m = |mode| encode(&Command::Mode { trx: 0, mode }).unwrap();
        assert_eq!(m(Mode::Cw), "modulation:0,CWU;");
        assert_eq!(m(Mode::CwL), "modulation:0,CWL;");
        assert_eq!(m(Mode::Fm), "modulation:0,FM;");
    }

    #[test]
    fn tx_sensors_give_peak_power_and_swr() {
        let mut s = RadioState::default();
        let m = parse("tx_sensors:0,-20.5,48.0,95.2,1.3;").unwrap();
        assert!(apply(&mut s, &m));
        assert_eq!(s.trx[0].tx_power_w, Some(95.2));
        assert_eq!(s.trx[0].tx_swr, Some(1.3));
    }

    #[test]
    fn parse_handles_case_and_spacing() {
        assert_eq!(
            parse("VFO:0, 1 ,7074000;"),
            Some(Msg {
                name: "vfo".into(),
                args: vec!["0".into(), "1".into(), "7074000".into()]
            })
        );
        assert_eq!(
            parse("ready;"),
            Some(Msg {
                name: "ready".into(),
                args: vec![]
            })
        );
        assert_eq!(parse("  ;"), None);
    }

    #[test]
    fn encodes_commands() {
        assert_eq!(
            encode(&Command::Vfo {
                trx: 0,
                vfo: 1,
                hz: 7_074_000
            })
            .unwrap(),
            "vfo:0,1,7074000;"
        );
        assert_eq!(
            encode(&Command::Mode {
                trx: 0,
                mode: Mode::Usb
            })
            .unwrap(),
            "modulation:0,USB;"
        );
        assert_eq!(encode(&Command::Volume { db: -58.17 }).unwrap(), "volume:-58.17;");
        assert_eq!(encode(&Command::Raw("trx:0,true".into())).unwrap(), "trx:0,true;");
        let cases = [
            (
                Command::AgcMode {
                    trx: 0,
                    mode: "fast".into(),
                },
                "agc_mode:0,fast;",
            ),
            (Command::RxEnable { trx: 1, on: true }, "rx_enable:1,true;"),
            (Command::SubRx { trx: 0, on: true }, "rx_channel_enable:0,1,true;"),
            (Command::Apf { trx: 0, on: true }, "rx_apf_enable:0,true;"),
            (Command::Binaural { trx: 0, on: false }, "rx_bin_enable:0,false;"),
            (Command::RxMute { trx: 1, on: true }, "rx_mute:1,true;"),
            (Command::RxVolume { trx: 0, db: -6.0 }, "rx_volume:0,0,-6.00;"),
            (Command::Monitor(true), "mon_enable:true;"),
            (Command::MonVolume { db: -30.0 }, "mon_volume:-30.00;"),
            (Command::CwSpeed(25), "cw_keyer_speed:25;"),
            (Command::TuneDrive { trx: 0, pct: 30 }, "tune_drive:0,30;"),
            (Command::Power(true), "start;"),
            (Command::Power(false), "stop;"),
            (Command::EnableTxMeters, "tx_sensors_enable:true,200;"),
        ];
        for (cmd, text) in cases {
            assert_eq!(encode(&cmd).as_deref(), Some(text), "{cmd:?}");
        }
    }

    #[test]
    fn tcix_handshake() {
        let mut n = TcixNegotiation::default();
        for m in split("tcix:1.0;cap:band,rw,enum,160m,80m;cap:preamp,rw,range,0,2,1,step;cap_end;") {
            assert!(n.feed(&m));
        }
        assert!(n.done);
        assert_eq!(n.manifest.names().collect::<Vec<_>>(), vec!["band", "preamp"]);
        assert!(!n.feed(&parse("vfo:0,0,1;").unwrap()));
        // A later cap line changes the manifest.
        assert!(n.feed(&parse("cap:preamp,none;").unwrap()));
        assert!(!n.manifest.has("preamp"));
    }

    /// Feed a TCI-X example (tci-x/examples) through the handshake and the
    /// state parser, as the client does.
    fn hub(example: &str) -> RadioState {
        let mut s = RadioState::default();
        let mut n = TcixNegotiation::default();
        for line in example.lines().filter(|l| l.trim() != "---") {
            for m in split(line) {
                if n.feed(&m) {
                    if n.done {
                        s.tcix = Some(n.manifest.clone());
                    }
                    continue;
                }
                apply(&mut s, &m);
            }
        }
        s
    }

    #[test]
    fn ic7100_manifest_and_extension_state() {
        let s = hub(include_str!("../tests/fixtures/tcix/ic7100_tcix.txt"));
        let m = s.tcix.as_ref().expect("negotiated");
        assert_eq!(m.enum_values("filter").unwrap(), ["fil1", "fil2", "fil3"]);
        assert!(m.writable("af_gain") && m.writable("squelch") && m.writable("band"));
        // The IC-7100 has presets, not a continuous passband, and no RF gain.
        assert!(!s.can_send("rx_filter_band") && !s.can_send("rf_gain"));
        assert!(!s.can_send("volume") && !s.can_send("lock"));
        assert_eq!(s.trx[0].filter_preset.as_deref(), Some("fil1"));
        // Not in the example's push (the hub hadn't read the radio): feed them.
        let mut s = s;
        for m in split("af_gain:0,40;squelch:0,12;dial_lock:true;notch:0,true,1200;") {
            assert!(apply(&mut s, &m), "{m:?}");
        }
        let t = &s.trx[0];
        assert_eq!(
            (t.af_gain, t.squelch, t.notch_hz),
            (Some(40), Some(12), Some(1200))
        );
        assert!(t.notch && s.dial_lock);
    }

    #[test]
    fn percent_encoding_round_trips() {
        assert_eq!(pct_encode("a,b:c;d%e"), "a%2Cb%3Ac%3Bd%25e");
        assert_eq!(pct_decode("a%2Cb%3Ac%3Bd%25e"), "a,b:c;d%e");
        assert_eq!(pct_decode("100%"), "100%");
        assert_eq!(pct_decode(&pct_encode("G4ÄBC")), "G4ÄBC");
    }

    #[test]
    fn control_and_refusals() {
        let mut s = RadioState::default();
        s.trx_mut(0).tx = true; // shown at once on key-up
        for m in split("control:g4abc,1760729000;error:trx,control,g4abc;") {
            apply(&mut s, &m);
        }
        assert_eq!(s.control.as_deref(), Some("g4abc"));
        assert!(!s.trx[0].tx, "a refused key-up isn't transmitting");
        assert_eq!((s.alert.as_deref(), s.alert_seq), (Some("ctrl"), 1));
        for m in split("control:;vfo_select:0,1;receive_only:true;error:atu_tune,interlock,amp;") {
            apply(&mut s, &m);
        }
        assert_eq!(s.control, None);
        assert_eq!(s.trx[0].vfo_selected, Some(1));
        assert!(s.receive_only);
        assert_eq!((s.alert.as_deref(), s.alert_seq), (Some("interlock"), 2));
        // Every flashed text can be drawn on the 7-segment display.
        for code in [
            "busy",
            "control",
            "access",
            "range",
            "unsupported",
            "radio",
            "duplex",
            "tx_active",
            "tx_lock",
            "tx_audio",
            "tx_timer",
            "tx_where",
            "device",
            "interlock",
            "timeout",
            "idle",
            "other",
        ] {
            let t = error_text(code);
            assert!(
                t.len() <= 9 && t.chars().all(|c| tmate2_hid::font::glyph(c).is_some()),
                "{code}: {t}"
            );
        }
    }

    #[test]
    fn every_report_is_taken() {
        let mut s = RadioState::default();
        let t0 = Instant::now();
        let ms = |n: u64| t0 + std::time::Duration::from_millis(n);
        s.trx_mut(0).af_gain = Some(20);
        // Two clicks of a knob go out.
        for pct in [21, 22] {
            let c = Command::AfGain { trx: 0, pct };
            s.expect(c.clone(), ms(1500));
            s.sent(&c);
        }
        // The echo of the first is shown; the knob goes on from the second.
        assert!(apply(&mut s, &parse("af_gain:0,21;").unwrap()));
        assert_eq!(s.trx[0].af_gain, Some(21));
        assert_eq!(s.working(ms(10)).trx[0].af_gain, Some(22));
        // Other reports apply as usual.
        apply(&mut s, &parse("rf_gain:0,90;").unwrap());
        assert_eq!(s.trx[0].rf_gain, Some(90));
        // A value of the radio's own is shown at once; the knob goes on from
        // the second click until the hold runs out, then from the radio's.
        apply(&mut s, &parse("af_gain:0,17;").unwrap());
        assert_eq!(s.trx[0].af_gain, Some(17));
        assert_eq!(s.working(ms(10)).trx[0].af_gain, Some(22));
        assert_eq!(s.working(ms(1500)).trx[0].af_gain, Some(17));
        // The radio catching up with the second ends the hold.
        apply(&mut s, &parse("af_gain:0,22;").unwrap());
        assert!(s.pending.is_empty());
    }

    #[test]
    fn a_value_held_is_only_its_own_receivers() {
        let mut s = RadioState::default();
        let t0 = Instant::now();
        s.trx_mut(0).rx_volume_db = Some(-10.0);
        let c = Command::RxVolume { trx: 0, db: -12.0 };
        s.expect(c.clone(), t0 + std::time::Duration::from_millis(400));
        s.sent(&c);
        apply(&mut s, &parse("rx_volume:1,0,-20;").unwrap());
        assert_eq!(s.pending.len(), 1, "RX2's report leaves RX1's alone");
        assert_eq!(s.trx[1].rx_volume_db, Some(-20.0));
        apply(&mut s, &parse("rx_volume:0,0,-12;").unwrap());
        assert!(s.pending.is_empty());
        // Radio-wide values have no receiver to tell apart.
        let vol = Command::Volume { db: -6.0 }.target().unwrap();
        assert!(vol.reported_by("volume", &["-12".into()]));
    }

    #[test]
    fn reads_ask_without_a_value() {
        let q = |c: Command| encode(&c.query().unwrap()).unwrap();
        assert_eq!(q(Command::Nr { trx: 0, on: true }), "rx_nr_enable:0;");
        assert_eq!(
            q(Command::Vfo {
                trx: 1,
                vfo: 1,
                hz: 7_000_000
            }),
            "vfo:1,1;"
        );
        assert_eq!(q(Command::Volume { db: -20.0 }), "volume;");
        assert_eq!(q(Command::RxVolume { trx: 0, db: -20.0 }), "rx_volume:0,0;");
        assert_eq!(q(Command::SubRx { trx: 0, on: true }), "rx_channel_enable:0,1;");
        // Never a read that could start the radio, nor TCI-X's own commands.
        assert!(Command::Power(true).query().is_none());
        assert!(Command::AfGain { trx: 0, pct: 1 }.query().is_none());
        assert_eq!(
            q(Command::Filter {
                trx: 0,
                low: 100,
                high: 2800
            }),
            "rx_filter_band:0;"
        );
    }

    #[test]
    fn front_end_state_and_commands() {
        let mut s = RadioState::default();
        for m in split("preamp:0,p2;attenuator:0,12;antenna:0,ant2;rx_antenna:0,true;pa_enable:0,false;radio_state:no_response;") {
            assert!(apply(&mut s, &m), "{m:?}");
        }
        let t = &s.trx[0];
        assert_eq!(
            (t.preamp.as_deref(), t.attenuator.as_deref()),
            (Some("p2"), Some("12"))
        );
        assert_eq!(t.antenna.as_deref(), Some("ant2"));
        assert!(t.rx_antenna && t.pa_enable == Some(false));
        assert_eq!(s.radio_state.as_deref(), Some("no_response"));
        let e = |c: Command| encode(&c).unwrap();
        assert_eq!(
            e(Command::Preamp {
                trx: 0,
                value: "p1".into()
            }),
            "preamp:0,p1;"
        );
        assert_eq!(
            e(Command::Attenuator {
                trx: 0,
                value: "12".into()
            }),
            "attenuator:0,12;"
        );
    }

    #[test]
    fn encodes_tcix_commands() {
        let e = |c: Command| encode(&c).unwrap();
        assert_eq!(e(Command::AfGain { trx: 0, pct: 40 }), "af_gain:0,40;");
        assert_eq!(e(Command::SquelchKnob { trx: 0, pct: 5 }), "squelch:0,5;");
        assert_eq!(e(Command::RfGain { trx: 0, pct: 90 }), "rf_gain:0,90;");
        assert_eq!(
            e(Command::FilterPreset {
                trx: 0,
                name: "fil2".into()
            }),
            "filter:0,0,fil2;"
        );
        assert_eq!(e(Command::DialLock(true)), "dial_lock:true;");
        assert_eq!(
            e(Command::NotchAt {
                trx: 0,
                on: true,
                hz: 1200
            }),
            "notch:0,true,1200;"
        );
        assert_eq!(e(Command::AtuTune { trx: 0 }), "atu_tune:0;");
        assert_eq!(
            e(Command::Band {
                trx: 0,
                name: "20m".into()
            }),
            "band:0,20m;"
        );
    }
}
