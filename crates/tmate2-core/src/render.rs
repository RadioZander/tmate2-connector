//! Draws the radio state on the TMATE-2 LCD, the way a radio would show it.

use tmate2_hid::{OutputReport, SegmentMap};

use crate::config::{DisplayConfig, KnobFunction, Readout, TxMeter, TxReadout};
use crate::engine::View;
use crate::state::{Mode, RadioState};

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
        // No W: neither Thetis nor piHPSDR has a wide-FM mode.
        Mode::Fm | Mode::Nfm => &["mode.fm"],
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
        map.draw_text(out, "main", "no radio");
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
    out.set_lock_led(trx.lock);

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
    // Knob labels. Functions without a label of their own light only E1 / E2.
    on("e1");
    if view.e1.has_lcd_label() {
        on(&format!("e1.{}", view.e1.name()));
    }
    on("e2");
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

    fn view() -> View {
        View {
            trx: 0,
            active_vfo: 0,
            step_hz: 100,
            e1: KnobFunction::Vol,
            e2: KnobFunction::Shift,
            flash: None,
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
}
