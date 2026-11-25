//! Write a [`Config`] back into the user's TOML file, keeping their comments,
//! ordering and formatting. Only values change.

use toml_edit::{Array, DocumentMut, InlineTable, Item, Value};

use tmate2_core::config::{
    Action, Backend, ButtonAction, Config, KnobFunction, Pacing, Readout, TcixMode, TxMeter, TxReadout,
    VolumeScale,
};

/// Set `table.key = v`, keeping the comments above the key and the spacing /
/// trailing comment around the old value.
fn set(doc: &mut DocumentMut, table: &str, key: &str, v: impl Into<Value>) {
    let mut new: Value = v.into();
    let t = doc.entry(table).or_insert(toml_edit::table());
    let Some(tbl) = t.as_table_like_mut() else { return };
    match tbl.get_mut(key) {
        Some(item) => {
            if let Some(old) = item.as_value() {
                *new.decor_mut() = old.decor().clone();
            }
            *item = Item::Value(new);
        }
        None => {
            tbl.insert(key, Item::Value(new));
        }
    }
}

fn int_array(v: impl IntoIterator<Item = i64>) -> Value {
    Value::Array(Array::from_iter(v))
}

fn str_array<'a>(v: impl IntoIterator<Item = &'a str>) -> Value {
    Value::Array(Array::from_iter(v))
}

fn number(x: f32) -> Value {
    if x.fract() == 0.0 {
        Value::from(x as i64)
    } else {
        Value::from((x as f64 * 100.0).round() / 100.0)
    }
}

fn action(a: &Option<Action>) -> String {
    a.as_ref().map_or_else(|| "none".to_string(), Action::to_string)
}

fn button(b: &ButtonAction) -> Value {
    let mut t = InlineTable::new();
    t.insert("tap", Value::from(action(&b.tap)));
    t.insert("hold", Value::from(action(&b.hold)));
    t.fmt();
    Value::InlineTable(t)
}

pub fn tcix_str(m: TcixMode) -> &'static str {
    match m {
        TcixMode::Off => "off",
        TcixMode::Auto => "auto",
        TcixMode::On => "on",
    }
}

pub fn pacing_str(p: Pacing) -> &'static str {
    match p {
        Pacing::Off => "off",
        Pacing::Auto => "auto",
        Pacing::On => "on",
    }
}

pub fn readout_str(r: Readout) -> &'static str {
    match r {
        Readout::Dbm => "dbm",
        Readout::SUnits => "s_units",
        Readout::Off => "off",
    }
}

fn functions(f: &[KnobFunction]) -> Value {
    str_array(f.iter().map(|x| x.name()))
}

pub fn write(doc: &mut DocumentMut, c: &Config) {
    let r = &c.radio;
    set(
        doc,
        "radio",
        "backend",
        match r.backend {
            Backend::Tci => "tci",
        },
    );
    set(doc, "radio", "host", r.host.as_str());
    set(doc, "radio", "port", r.port as i64);
    set(doc, "radio", "tcix", tcix_str(r.tcix));
    set(doc, "radio", "pacing", pacing_str(r.pacing));
    set(doc, "radio", "poll_ms", r.poll_ms as i64);
    set(doc, "radio", "trx", r.trx as i64);

    let d = &c.display;
    set(doc, "display", "contrast", d.contrast as i64);
    set(doc, "display", "backlight", int_array(d.backlight.map(i64::from)));
    set(
        doc,
        "display",
        "backlight_tx",
        int_array(d.backlight_tx.map(i64::from)),
    );
    set(
        doc,
        "display",
        "backlight_disconnected",
        int_array(d.backlight_disconnected.map(i64::from)),
    );
    set(doc, "display", "readout", readout_str(d.readout));
    set(doc, "display", "flash_ms", d.flash_ms as i64);
    set(
        doc,
        "display",
        "tx_meter",
        match d.tx_meter {
            TxMeter::Power => "power",
            TxMeter::Swr => "swr",
            TxMeter::Off => "off",
        },
    );
    set(
        doc,
        "display",
        "tx_readout",
        match d.tx_readout {
            TxReadout::Watts => "watts",
            TxReadout::Swr => "swr",
            TxReadout::Off => "off",
        },
    );
    set(doc, "display", "tx_power_max_w", d.tx_power_max_w as i64);
    set(
        doc,
        "display",
        "volume_scale",
        match d.volume_scale {
            VolumeScale::Percent => "percent",
            VolumeScale::Db => "db",
        },
    );

    let t = &c.tuning;
    set(
        doc,
        "tuning",
        "steps_hz",
        int_array(t.steps_hz.iter().map(|s| *s as i64)),
    );
    set(doc, "tuning", "step_hz", t.step_hz as i64);
    set(doc, "tuning", "acceleration", number(t.acceleration));
    set(doc, "tuning", "counts_per_detent", t.counts_per_detent as i64);
    set(doc, "tuning", "filter_step_hz", t.filter_step_hz as i64);
    set(doc, "tuning", "rit_step_hz", t.rit_step_hz as i64);
    set(doc, "tuning", "fast_multiplier", t.fast_multiplier as i64);
    set(
        doc,
        "tuning",
        "mode_cycle",
        str_array(t.mode_cycle.iter().map(String::as_str)),
    );

    set(doc, "e1", "functions", functions(&c.e1.functions));
    set(doc, "e2", "functions", functions(&c.e2.functions));
    for (t, k) in [("e1", &c.e1), ("e2", &c.e2)] {
        set(doc, t, "start", k.functions[k.start_index()].name());
    }

    let b = &c.buttons;
    set(doc, "buttons", "hold_ms", b.hold_ms as i64);
    set(doc, "buttons", "push_lockout_ms", b.push_lockout_ms as i64);
    set(doc, "buttons", "hold_repeat_ms", b.hold_repeat_ms as i64);
    for (k, v) in [
        ("f1", &b.f1),
        ("f2", &b.f2),
        ("f3", &b.f3),
        ("f4", &b.f4),
        ("f5", &b.f5),
        ("f6", &b.f6),
        ("main_push", &b.main_push),
        ("e1_push", &b.e1_push),
        ("e2_push", &b.e2_push),
    ] {
        set(doc, "buttons", k, button(v));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tmate2_core::config::DEFAULT_TOML;

    #[test]
    fn round_trip_keeps_comments_and_applies_changes() {
        let mut doc: DocumentMut = DEFAULT_TOML.parse().unwrap();
        let mut c = Config::default();
        c.radio.port = 40001;
        c.tuning.acceleration = 7.0;
        c.buttons.f2.tap = Some(Action::Mode("USB".into()));
        write(&mut doc, &c);
        let text = doc.to_string();
        assert!(text.contains("# TCI server address. Thetis: Setup > Serial/Network > TCI (port 50001)."));
        assert!(text.contains("port = 40001"));
        assert!(text.contains("acceleration = 7\n"));
        assert!(
            text.contains(r#"f2 = { tap = "mode:USB", hold = "none" }"#),
            "{text}"
        );
        let again = Config::parse(&text).unwrap();
        assert_eq!(again.radio.port, 40001);
        assert_eq!(again.buttons.f2.tap, Some(Action::Mode("USB".into())));
    }

    #[test]
    fn unchanged_config_writes_identical_text() {
        let mut doc: DocumentMut = DEFAULT_TOML.parse().unwrap();
        write(&mut doc, &Config::default());
        assert_eq!(doc.to_string(), DEFAULT_TOML);
    }
}
