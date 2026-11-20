//! TCI (Expert Electronics' Transceiver Control Interface) text protocol:
//! parsing server messages into [`RadioState`] and encoding [`Command`]s.
//!
//! Messages look like `name:arg,arg,...;`. Names are case-insensitive.

use std::time::Instant;

use tmate2_core::state::Mode;
use tmate2_core::{Command, RadioState};

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
        Command::Focus => "set_in_focus;".to_string(),
        Command::EnableMeters => "rx_sensors_enable:true,200;".to_string(),
        Command::EnableTxMeters => "tx_sensors_enable:true,200;".to_string(),
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

/// TCI-X negotiation state, fed with every server message.
#[derive(Debug, Default)]
pub struct TcixNegotiation {
    pub replied: bool,
    pub caps: Vec<String>,
    pub done: bool,
}

impl TcixNegotiation {
    /// Returns true if the message belonged to the TCI-X handshake.
    pub fn feed(&mut self, m: &Msg) -> bool {
        match m.name.as_str() {
            "tcix" => self.replied = true,
            "cap" if self.replied => {
                if let Some(c) = m.args.first() {
                    self.caps.push(c.to_ascii_lowercase());
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

    #[test]
    fn tx_sensors_give_peak_power_and_swr() {
        let mut s = RadioState::default();
        let m = parse("tx_sensors:0,-20.5,48.0,95.2,1.3;").unwrap();
        assert!(apply(&mut s, &m, Instant::now()));
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
            (Command::Focus, "set_in_focus;"),
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
        assert_eq!(n.caps, vec!["band", "preamp"]);
        assert!(!n.feed(&parse("vfo:0,0,1;").unwrap()));
    }
}
