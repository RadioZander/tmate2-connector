//! Draws the radio state on the TMATE-2 LCD, the way a radio would show it.

use tmate2_hid::{OutputReport, SegmentMap};

use crate::config::{DisplayConfig, KnobFunction, Readout, TxMeter, TxReadout};
use crate::engine::View;
use crate::state::{Lamp, Mode, RadioState};

/// Scale labels that are always shown (scale.10 is deliberately not used).
const SCALE: [&str; 12] = [
    "scale.s",
    "scale.1",
    "scale.3",
    "scale.5",
    "scale.7",
    "scale.9",
    "scale.20.plus",
    "scale.20",
    "scale.40.plus",
    "scale.40",
    "scale.60.plus",
    "scale.60",
];

/// Device contrast value for a percentage (100 = darkest). The LCD gets
/// darker as the raw value falls; 0 is darkest, segments fade towards 128 and
/// vanish by 192 (HW1.1 / FW1.5). 0 % maps to a faint but readable 80.
pub fn contrast_raw(pct: u8) -> u8 {
    const FAINTEST: u32 = 80;
    ((100 - pct.min(100) as u32) * FAINTEST / 100) as u8
}

/// Number of S-meter bars (of 15) for a level in dBm: bars 1-9 are S1-S9
/// (6 dB each, S9 = -73 dBm), bars 10-15 are +10 .. +60 dB over S9.
pub fn bars_for_dbm(dbm: f32) -> usize {
    if dbm < -73.0 {
        (((dbm + 127.0) / 6.0).floor().max(0.0) as usize).min(9)
    } else {
        (9 + ((dbm + 73.0) / 10.0).floor() as usize).min(15)
    }
}

/// Small-readout text for a level in dBm: the S unit ("7", "9"), or the dB
/// over S9 ("+20"). No "S" prefix: on seven segments it reads as a 5.
pub fn s_units_text(dbm: f32) -> String {
    match bars_for_dbm(dbm) {
        n @ 0..=9 => n.to_string(),
        n => format!("+{}", (n - 9) * 10),
    }
}

/// Bars (of 15) for transmit power, 0 W to `max_w`.
pub fn bars_for_power(w: f32, max_w: u32) -> usize {
    ((w / max_w.max(1) as f32) * 15.0).round().clamp(0.0, 15.0) as usize
}

/// Bars (of 15) for SWR, laid out like the S scale: 1:1 is none, 1.8:1 (the
/// usual safe limit) is the S9 bar, and the six bars above S9 run to 3:1.
pub fn bars_for_swr(swr: f32) -> usize {
    let swr = swr.max(1.0);
    let bars = if swr <= 1.8 {
        (swr - 1.0) / 0.8 * 9.0
    } else {
        9.0 + (swr - 1.8) / 1.2 * 6.0
    };
    bars.round().clamp(0.0, 15.0) as usize
}

/// SWR on the 3-digit readout, which has no decimal point: the middle digit's
/// bottom bar stands in for it, so 1.2:1 is "1_2". 10:1 and worse is "Hi".
pub fn swr_text(swr: f32) -> String {
    let tenths = (swr.max(1.0) * 10.0).round() as u32;
    if tenths >= 100 {
        "Hi".to_string()
    } else {
        format!("{}_{}", tenths / 10, tenths % 10)
    }
}

fn mode_icons(m: Mode) -> &'static [&'static str] {
    match m {
        Mode::Lsb => &["mode.lsb"],
        Mode::Usb => &["mode.usb"],
        Mode::CwL => &["mode.cw", "mode.cw.minus"],
        Mode::CwU => &["mode.cw", "mode.cw.plus"],
        Mode::Cw => &["mode.cw"],
        Mode::Am => &["mode.am"],
        Mode::Sam => &["mode.sam"],
        Mode::Dsb => &["mode.dsb"],
        // W only when the radio reports wide FM: Thetis lists NFM and FM but
        // they are one mode, and piHPSDR has no wide FM.
        Mode::Fm | Mode::Nfm => &["mode.fm"],
        Mode::Wfm => &["mode.fm", "mode.fm.w"],
        Mode::DigL => &["mode.dig", "mode.dig.minus"],
        Mode::DigU => &["mode.dig", "mode.dig.plus"],
        Mode::Drm => &["mode.drm"],
        Mode::Other => &[],
    }
}

pub fn render(
    state: &RadioState,
    view: &View,
    cfg: &DisplayConfig,
    map: &SegmentMap,
    out: &mut OutputReport,
) {
    out.clear_segments();
    out.set_contrast(contrast_raw(cfg.contrast));

    if !state.connected {
        let [r, g, b] = cfg.backlight_disconnected;
        out.set_rgb(r, g, b);
        out.set_usb_led(false);
        out.set_lock_led(false);
        map.draw_text(out, "main", state.notice.as_deref().unwrap_or("no radio"));
        return;
    }

    let trx = state.trx(view.trx).cloned().unwrap_or_default();
    let transmitting = trx.tx || trx.tune;
    let [r, g, b] = if transmitting {
        cfg.backlight_tx
    } else {
        cfg.backlight
    };
    out.set_rgb(r, g, b);
    out.set_usb_led(true);
    out.set_lock_led(trx.lock || state.dial_lock);

    // Main display: a flashed value, or the active VFO frequency.
    match &view.flash {
        Some(text) => map.draw_text(out, "main", text),
        None => {
            map.draw_frequency(out, trx.vfo[view.active_vfo.min(1)]);
            map.set(out, "unit.hz", true);
            // Underline the digit the step changes (only for 1, 10, 100, ... Hz).
            let step = view.step_hz;
            if step > 0 && 10u64.pow(step.ilog10()) == step {
                map.set(out, &format!("underline.{}", step.ilog10() + 1), true);
            }
        }
    }

    let mut on = |name: &str| {
        map.set(out, name, true);
    };
    on("vfo");
    on(if view.active_vfo == 0 { "vfo.a" } else { "vfo.b" });
    if let Some(m) = trx.mode {
        mode_icons(m).iter().for_each(|n| on(n));
    }
    on(if transmitting { "tx" } else { "rx" });
    if trx.nr {
        on("left.nr");
    }
    if trx.nb {
        on("left.nb");
    }
    // The radio's front end, where the server reports it (TCI-X).
    let (att, pre) = state.front_end_lamps(view.trx);
    for (lamp, name) in [(att, "att"), (pre, "pre")] {
        match lamp {
            Lamp::Off => {}
            Lamp::On => on(name),
            Lamp::Step(n) => {
                on(name);
                on(&format!("{name}.{n}"));
            }
        }
    }
    if state.other_antenna(view.trx) {
        on("left.ant");
    }
    if trx.pa_enable == Some(false) {
        on("lp");
    }
    if state.radio_state.as_deref().is_some_and(|r| r != "connected") {
        on("err");
    }
    // Knob labels. Functions without a label of their own light only E1 / E2,
    // and on receive the small readout holds the name of one of them. If both
    // knobs are on such functions, only the E marker of the knob named there
    // stays lit, so the name can't be taken for the other knob's.
    let named = if transmitting { None } else { view.readout_knob() };
    let marker = |k: usize| view.knob(k).has_lcd_label() || named.is_none_or(|n| n == k);
    if marker(0) {
        on("e1");
    }
    if view.e1.has_lcd_label() {
        on(&format!("e1.{}", view.e1.name()));
    }
    if marker(1) {
        on("e2");
    }
    match view.e2 {
        KnobFunction::Width => {
            on("e2.high");
            on("e2.low");
        }
        f if f.has_lcd_label() => on(&format!("e2.{}", f.name())),
        _ => {}
    }

    on("bar.line");
    if transmitting {
        // Transmit meter: no S scale, the bargraph shows power or SWR.
        let bars = match cfg.tx_meter {
            TxMeter::Power => trx.tx_power_w.map(|w| bars_for_power(w, cfg.tx_power_max_w)),
            TxMeter::Swr => trx.tx_swr.map(bars_for_swr),
            TxMeter::Off => None,
        };
        for i in 1..=bars.unwrap_or(0) {
            on(&format!("bar.{i}"));
        }
        match (cfg.tx_readout, trx.tx_power_w, trx.tx_swr) {
            (TxReadout::Watts, Some(w), _) => {
                on("unit.w");
                map.draw_text(out, "smeter", &(w.round() as u32).min(999).to_string());
            }
            (TxReadout::Swr, _, Some(swr)) => map.draw_text(out, "smeter", &swr_text(swr)),
            _ => {}
        }
        return;
    }

    // S-meter: bar graph, fixed scale, small readout.
    SCALE.iter().for_each(|n| on(n));
    if let Some(dbm) = trx.rx_dbm {
        for i in 1..=bars_for_dbm(dbm) {
            on(&format!("bar.{i}"));
        }
    }
    // A knob resting on a function without an LCD label: the readout holds
    // its name instead, so it's clear which one the knob is on.
    if let Some((name, minus)) = named.and_then(|k| view.knob(k).readout_name()) {
        if minus {
            on("smeter.minus");
        }
        map.draw_text(out, "smeter", name);
    } else if let Some(dbm) = trx.rx_dbm {
        match cfg.readout {
            Readout::Dbm => {
                let v = dbm.round() as i32;
                if v < 0 {
                    on("smeter.minus");
                }
                on("unit.dbm");
                map.draw_text(out, "smeter", &v.unsigned_abs().min(999).to_string());
            }
            Readout::SUnits => map.draw_text(out, "smeter", &s_units_text(dbm)),
            Readout::Off => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, KnobFunction};
    use crate::engine::tests::input;

    fn view() -> View {
        View {
            trx: 0,
            active_vfo: 0,
            step_hz: 100,
            e1: KnobFunction::Vol,
            e2: KnobFunction::Shift,
            flash: None,
            last_knob: 0,
        }
    }

    #[test]
    fn contrast_percent_maps_onto_usable_range() {
        assert_eq!(contrast_raw(100), 0);
        assert_eq!(contrast_raw(0), 80);
        assert_eq!(contrast_raw(50), 40);
        assert_eq!(contrast_raw(200), 0);
    }

    #[test]
    fn s_meter_scale() {
        assert_eq!(bars_for_dbm(-127.0), 0);
        assert_eq!(bars_for_dbm(-121.0), 1);
        assert_eq!(bars_for_dbm(-73.0), 9);
        assert_eq!(bars_for_dbm(-53.0), 11);
        assert_eq!(bars_for_dbm(0.0), 15);
        assert_eq!(s_units_text(-80.0), "7");
        assert_eq!(s_units_text(-73.0), "9");
        assert_eq!(s_units_text(-63.0), "+10");
        assert_eq!(s_units_text(-50.0), "+20");
    }

    #[test]
    fn front_end_lamps_follow_the_radios_own_lists() {
        let map = SegmentMap::builtin();
        let cfg = Config::default();
        let radio = |caps: &[&str]| {
            let mut s = RadioState {
                connected: true,
                ..Default::default()
            };
            let mut m = crate::Manifest::default();
            for line in caps {
                let args: Vec<String> = line.split(',').map(str::to_string).collect();
                assert!(m.apply(&args), "{line}");
            }
            s.tcix = Some(m);
            s.trx_mut(0).vfo[0] = 14_074_000;
            s
        };
        let lit = |s: &RadioState| {
            let mut out = OutputReport::default();
            render(s, &view(), &cfg.display, &map, &mut out);
            [
                "att", "att.1", "att.2", "pre", "pre.1", "pre.2", "left.ant", "lp", "err",
            ]
            .into_iter()
            .filter(|n| out.segment(map.get(n).unwrap()))
            .collect::<Vec<_>>()
        };
        // An IC-7100: two preamps, one attenuator step, two antennas.
        let mut s = radio(&[
            "preamp,rw,enum,off,p1,p2",
            "attenuator,rw,enum,0,12",
            "antenna,rw,enum,ant1,ant2",
        ]);
        assert!(lit(&s).is_empty());
        let t = s.trx_mut(0);
        (t.preamp, t.attenuator, t.antenna) = (Some("off".into()), Some("0".into()), Some("ant1".into()));
        assert!(lit(&s).is_empty());
        s.trx_mut(0).preamp = Some("p1".into());
        assert_eq!(lit(&s), ["pre", "pre.1"]);
        let t = s.trx_mut(0);
        (t.preamp, t.attenuator, t.antenna) = (Some("p2".into()), Some("12".into()), Some("ant2".into()));
        // One attenuator step: ATT with no number.
        assert_eq!(lit(&s), ["att", "pre", "pre.2", "left.ant"]);
        // Four attenuator steps (a Yaesu): 1, then 2 for the rest.
        let mut y = radio(&["attenuator,rw,enum,0,6,12,18", "preamp,rw,enum,off,p1"]);
        y.trx_mut(0).attenuator = Some("6".into());
        y.trx_mut(0).preamp = Some("p1".into());
        assert_eq!(lit(&y), ["att", "att.1", "pre"]);
        y.trx_mut(0).attenuator = Some("18".into());
        assert_eq!(lit(&y)[..2], ["att", "att.2"]);
        // A FlexRadio's preamp is a gain in dB: PRE above 0, ATT below.
        let mut f = radio(&["preamp,rw,enum,-20,-10,0,10,20,30"]);
        f.trx_mut(0).preamp = Some("20".into());
        assert_eq!(lit(&f), ["pre"]);
        f.trx_mut(0).preamp = Some("-10".into());
        assert_eq!(lit(&f), ["att"]);
        f.trx_mut(0).preamp = Some("0".into());
        // The PA off, the receive antenna, and a radio that isn't answering.
        f.trx_mut(0).pa_enable = Some(false);
        f.trx_mut(0).rx_antenna = true;
        f.radio_state = Some("no_response".into());
        assert_eq!(lit(&f), ["left.ant", "lp", "err"]);
        f.radio_state = Some("connected".into());
        assert_eq!(lit(&f), ["left.ant", "lp"]);
    }

    #[test]
    fn receive_screen() {
        let map = SegmentMap::builtin();
        let cfg = Config::default();
        let mut s = RadioState {
            connected: true,
            ..Default::default()
        };
        let t = s.trx_mut(0);
        t.vfo[0] = 14_074_000;
        t.mode = Some(Mode::CwU);
        t.rx_dbm = Some(-72.6);
        let mut out = OutputReport::default();
        render(&s, &view(), &cfg.display, &map, &mut out);
        let lit = |n: &str| out.segment(map.get(n).unwrap());
        for n in [
            "unit.hz",
            "underline.3",
            "mode.cw",
            "mode.cw.plus",
            "vfo.a",
            "rx",
            "bar.9",
            "smeter.minus",
            "unit.dbm",
            "e1.vol",
            "e2.shift",
            "scale.60",
        ] {
            assert!(lit(n), "{n} should be lit");
        }
        for n in ["unit.k", "scale.10", "bar.10", "tx", "mode.usb", "vfo.b"] {
            assert!(!lit(n), "{n} should be off");
        }
    }

    #[test]
    fn transmit_shows_power_without_the_s_scale() {
        let map = SegmentMap::builtin();
        let mut cfg = Config::default();
        let mut s = RadioState {
            connected: true,
            ..Default::default()
        };
        let t = s.trx_mut(0);
        t.tx = true;
        t.rx_dbm = Some(-50.0);
        t.tx_power_w = Some(52.4);
        t.tx_swr = Some(1.24);
        let mut out = OutputReport::default();
        render(&s, &view(), &cfg.display, &map, &mut out);
        let lit = |out: &OutputReport, n: &str| out.segment(map.get(n).unwrap());
        assert_eq!(out.rgb(), (255, 0, 0));
        assert!(lit(&out, "tx") && lit(&out, "unit.w"));
        // 52 W of 100: 8 of 15 bars.
        assert!(lit(&out, "bar.8") && !lit(&out, "bar.9"));
        for n in ["scale.s", "scale.9", "scale.60", "unit.dbm", "smeter.minus"] {
            assert!(!lit(&out, n), "{n} should be off on transmit");
        }

        cfg.display.tx_meter = TxMeter::Swr;
        cfg.display.tx_readout = TxReadout::Swr;
        render(&s, &view(), &cfg.display, &map, &mut out);
        // SWR 1.24: 3 bars; readout 1_2 without the W unit.
        assert!(lit(&out, "bar.3") && !lit(&out, "bar.4"));
        assert!(!lit(&out, "unit.w"));
        assert!(lit(&out, "smeter.3.b") && lit(&out, "smeter.2.d") && !lit(&out, "smeter.2.g"));
    }

    #[test]
    fn plain_cw_has_no_sideband_and_fm_no_w() {
        let map = SegmentMap::builtin();
        let cfg = Config::default();
        let mut s = RadioState {
            connected: true,
            ..Default::default()
        };
        let mut out = OutputReport::default();
        let mut lit = |m: Mode, n: &str| {
            s.trx_mut(0).mode = Some(m);
            render(&s, &view(), &cfg.display, &map, &mut out);
            out.segment(map.get(n).unwrap())
        };
        assert!(lit(Mode::Cw, "mode.cw"));
        assert!(!lit(Mode::Cw, "mode.cw.plus") && !lit(Mode::Cw, "mode.cw.minus"));
        assert!(lit(Mode::CwL, "mode.cw.minus") && lit(Mode::CwU, "mode.cw.plus"));
        for m in [Mode::Fm, Mode::Nfm] {
            assert!(lit(m, "mode.fm") && !lit(m, "mode.fm.w"), "{m:?}");
        }
        assert!(lit(Mode::Wfm, "mode.fm") && lit(Mode::Wfm, "mode.fm.w"));
    }

    #[test]
    fn tx_meter_scales() {
        assert_eq!(bars_for_power(0.0, 100), 0);
        assert_eq!(bars_for_power(100.0, 100), 15);
        assert_eq!(bars_for_power(150.0, 100), 15);
        assert_eq!(bars_for_power(5.0, 10), 8);
        assert_eq!(bars_for_swr(1.0), 0);
        assert_eq!(bars_for_swr(1.5), 6);
        assert_eq!(bars_for_swr(1.8), 9, "1.8:1 sits at S9");
        assert_eq!(bars_for_swr(2.0), 10);
        assert_eq!(bars_for_swr(3.0), 15);
        assert_eq!(bars_for_swr(3.5), 15);
        assert_eq!(swr_text(1.0), "1_0");
        assert_eq!(swr_text(1.24), "1_2");
        assert_eq!(swr_text(2.96), "3_0");
        assert_eq!(swr_text(12.0), "Hi");
    }

    /// The small readout's digits show exactly `text`.
    fn readout_is(map: &SegmentMap, out: &OutputReport, text: &str) -> bool {
        let mut want = OutputReport::default();
        map.draw_text(&mut want, "smeter", text);
        (1..=3).all(|d| {
            "abcdefg".chars().all(|seg| {
                let a = map.get(&format!("smeter.{d}.{seg}")).unwrap();
                out.segment(a) == want.segment(a)
            })
        })
    }

    #[test]
    fn readout_names_a_knob_on_a_function_without_a_label() {
        let map = SegmentMap::builtin();
        let cfg = Config::default();
        let mut s = RadioState {
            connected: true,
            ..Default::default()
        };
        s.trx_mut(0).rx_dbm = Some(-73.0);
        let mut out = OutputReport::default();
        let lit = |out: &OutputReport, n: &str| out.segment(map.get(n).unwrap());
        for (f, name, minus) in [
            (KnobFunction::RxVol, "AF", false),
            (KnobFunction::MonVol, "non", true),
            (KnobFunction::CwSpeed, "SPd", false),
            (KnobFunction::TuneDrive, "tdr", false),
        ] {
            let v = View { e1: f, ..view() };
            render(&s, &v, &cfg.display, &map, &mut out);
            assert!(readout_is(&map, &out, name), "{f:?} shows {name}");
            assert_eq!(lit(&out, "smeter.minus"), minus, "{f:?} minus");
            assert!(!lit(&out, "unit.dbm"), "{f:?}: no dBm unit");
            assert!(lit(&out, "bar.1"), "{f:?}: the S-meter bars still work");
        }
        // On a labelled function: -73 dBm as usual.
        render(&s, &view(), &cfg.display, &map, &mut out);
        assert!(readout_is(&map, &out, "73") && lit(&out, "smeter.minus") && lit(&out, "unit.dbm"));
        // Both knobs unlabelled: the one touched last.
        let both = View {
            e1: KnobFunction::CwSpeed,
            e2: KnobFunction::TuneDrive,
            last_knob: 1,
            ..view()
        };
        render(&s, &both, &cfg.display, &map, &mut out);
        assert!(readout_is(&map, &out, "tdr"));
        // Only E2's marker: the name is E2's, and E1's function isn't shown.
        assert!(lit(&out, "e2") && !lit(&out, "e1"));
        let both_e1 = View {
            last_knob: 0,
            ..both.clone()
        };
        render(&s, &both_e1, &cfg.display, &map, &mut out);
        assert!(readout_is(&map, &out, "SPd") && lit(&out, "e1") && !lit(&out, "e2"));
        // E2 touched last but labelled: E1's name.
        let e1_only = View {
            e1: KnobFunction::CwSpeed,
            last_knob: 1,
            ..view()
        };
        render(&s, &e1_only, &cfg.display, &map, &mut out);
        assert!(readout_is(&map, &out, "SPd"));
        // E2 on a labelled function keeps its marker and label.
        assert!(lit(&out, "e1") && lit(&out, "e2") && lit(&out, "e2.shift"));
        // Transmitting: watts as before.
        s.trx_mut(0).tx = true;
        s.trx_mut(0).tx_power_w = Some(50.0);
        render(
            &s,
            &View {
                e1: KnobFunction::CwSpeed,
                ..view()
            },
            &cfg.display,
            &map,
            &mut out,
        );
        assert!(readout_is(&map, &out, "50") && lit(&out, "unit.w"));
        // No name on transmit, so both markers stay lit.
        render(&s, &both, &cfg.display, &map, &mut out);
        assert!(lit(&out, "e1") && lit(&out, "e2"));
    }

    #[test]
    fn last_knob_follows_turns_and_pushes() {
        use crate::engine::Engine;
        use tmate2_hid::{Button, Encoder, InputEvent};
        let mut e = Engine::new(Config::default());
        let mut s = RadioState::default();
        let now = std::time::Instant::now();
        assert_eq!(e.view(now, &s).last_knob, 0);
        input(
            &mut e,
            InputEvent::Turned {
                encoder: Encoder::E2,
                delta: 2,
            },
            now,
            &mut s,
        );
        assert_eq!(e.view(now, &s).last_knob, 1);
        input(&mut e, InputEvent::Pressed(Button::E1Push), now, &mut s);
        assert_eq!(e.view(now, &s).last_knob, 0);
        input(
            &mut e,
            InputEvent::Turned {
                encoder: Encoder::Main,
                delta: 1,
            },
            now,
            &mut s,
        );
        assert_eq!(e.view(now, &s).last_knob, 0, "the main knob doesn't count");
    }
}
