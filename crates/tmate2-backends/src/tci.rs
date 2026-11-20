//! TCI (Expert Electronics' Transceiver Control Interface) text protocol:
//! parsing server messages into [`RadioState`] and encoding [`Command`]s.
//!
//! Messages look like `name:arg,arg,...;`. Names are case-insensitive.

use std::time::Instant;

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
pub fn apply(state: &mut RadioState, m: &Msg, now: Instant) -> bool {
    let a: Vec<&str> = m.args.iter().map(String::as_str).collect();
    let n = |i: usize| a.get(i).and_then(|s| s.parse::<i64>().ok());
    let f = |i: usize| a.get(i).and_then(|s| s.parse::<f32>().ok());
    let flag = |i: usize| a.get(i).and_then(|s| b(s));
    let trx = n(0).map(|v| v.max(0) as usize);

    // A knob here is setting this right now: the echo of an earlier click
    // would undo the later ones. The guard ends when the radio reports what
    // we hold (it has caught up), or when its time runs out.
    if state.guarded(&m.name, now) {
        let mut probe = state.clone();
        probe.guards.clear();
        apply(&mut probe, m, now);
        probe.guards.clone_from(&state.guards);
        if probe == *state {
            state.guards.remove(m.name.as_str());
        }
        return true;
    }

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
        "modulations_list" => state.modes = a.iter().map(|s| Mode::from_name(s)).collect(),
        "vfo_limits" => {
            if let (Some(lo), Some(hi)) = (n(0), n(1)) {
                state.vfo_limits = Some((lo.max(0) as u64, hi.max(0) as u64));
            }
        }
        "vfo" => {
            if let (Some(t), Some(ch), Some(hz)) = (trx, n(1), n(2)) {
                state.remote_vfo(t, ch.max(0) as usize, hz.max(0) as u64, now);
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
        Command::AfGain { trx, pct } => format!("af_gain:{trx},{pct};"),
        Command::RfGain { trx, pct } => format!("rf_gain:{trx},{pct};"),
        Command::SquelchKnob { trx, pct } => format!("squelch:{trx},{pct};"),
        Command::FilterPreset { trx, name } => format!("filter:{trx},0,{};", pct_encode(name)),
        Command::DialLock(on) => format!("dial_lock:{};", tf(*on)),
        Command::NotchAt { trx, on, hz } => format!("notch:{trx},{},{hz};", tf(*on)),
        Command::AtuTune { trx } => format!("atu_tune:{trx};"),
        Command::Band { trx, name } => format!("band:{trx},{};", pct_encode(name)),
        Command::VfoSelect { trx, ch } => format!("vfo_select:{trx},{ch};"),
        Command::ControlRequest => "control_request;".to_string(),
        Command::ControlRelease => "control_release;".to_string(),
        Command::EnableMeters => "rx_sensors_enable:true,200;".to_string(),
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

    const THETIS_INIT: &str = include_str!("../tests/fixtures/thetis-init.tci");

    #[test]
    fn thetis_init_burst_fills_state() {
        let mut s = RadioState::default();
        let now = Instant::now();
        for line in THETIS_INIT.lines() {
            for m in split(line) {
                apply(&mut s, &m, now);
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
        let now = Instant::now();
        for line in example.lines().filter(|l| l.trim() != "---") {
            for m in split(line) {
                if n.feed(&m) {
                    if n.done {
                        s.tcix = Some(n.manifest.clone());
                    }
                    continue;
                }
                apply(&mut s, &m, now);
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
            assert!(apply(&mut s, &m, Instant::now()), "{m:?}");
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
        let now = Instant::now();
        for m in split("control:g4abc,1760729000;error:trx,control,g4abc;") {
            apply(&mut s, &m, now);
        }
        assert_eq!(s.control.as_deref(), Some("g4abc"));
        assert!(!s.trx[0].tx, "a refused key-up isn't transmitting");
        assert_eq!((s.alert.as_deref(), s.alert_seq), (Some("ctrl"), 1));
        for m in split("control:;vfo_select:0,1;receive_only:true;error:atu_tune,interlock,amp;") {
            apply(&mut s, &m, now);
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
    fn reports_of_a_knob_in_use_are_ignored() {
        let mut s = RadioState::default();
        let t0 = Instant::now();
        s.trx_mut(0).af_gain = Some(20);
        s.guard("af_gain", t0 + std::time::Duration::from_millis(400));
        // The echo of an earlier click, while the knob is still turning.
        let echo = parse("af_gain:0,17;").unwrap();
        assert!(apply(&mut s, &echo, t0 + std::time::Duration::from_millis(50)));
        assert_eq!(s.trx[0].af_gain, Some(20));
        // Other reports still apply.
        apply(&mut s, &parse("rf_gain:0,90;").unwrap(), t0);
        assert_eq!(s.trx[0].rf_gain, Some(90));
        // The radio catches up with the value we hold: the guard ends, and
        // a later change made at the radio is taken at once.
        let soon = t0 + std::time::Duration::from_millis(100);
        apply(&mut s, &parse("af_gain:0,20;").unwrap(), soon);
        assert!(!s.guarded("af_gain", soon));
        apply(&mut s, &echo, soon);
        assert_eq!(s.trx[0].af_gain, Some(17));
        // With no matching report, the guard runs out by itself.
        s.guard("af_gain", t0 + std::time::Duration::from_millis(400));
        apply(
            &mut s,
            &parse("af_gain:0,5;").unwrap(),
            t0 + std::time::Duration::from_millis(500),
        );
        assert_eq!(s.trx[0].af_gain, Some(5));
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
