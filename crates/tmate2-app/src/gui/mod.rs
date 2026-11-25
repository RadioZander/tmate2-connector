//! Desktop window: live LCD mirror, status, and a settings editor that saves
//! into the user's config file (keeping their comments) and applies at once.

mod config_edit;
mod lcd;

use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use eframe::egui::{self, Color32, RichText};
use tokio::sync::watch;

use tmate2_core::config::{
    server_label, Action, ButtonAction, Config, KnobFunction, Pacing, RadioConfig, Readout, TcixMode,
    TxMeter, TxReadout, VolumeScale, WindowMode, AGC_MODES, BACKLIGHT_PRESETS as PRESETS, POLL_MIN_MS,
};
use tmate2_hid::{Button, Encoder, InputEvent, SegmentMap};

use crate::shared::Shared;

// Each knob's labelled functions in LCD order, then the ones without a label.
const E1_CHOICES: [KnobFunction; 11] = [
    KnobFunction::Vol,
    KnobFunction::Rfg,
    KnobFunction::Sql,
    KnobFunction::Drv,
    KnobFunction::Nr,
    KnobFunction::Nb,
    KnobFunction::An,
    KnobFunction::RxVol,
    KnobFunction::MonVol,
    KnobFunction::CwSpeed,
    KnobFunction::TuneDrive,
];
const E2_CHOICES: [KnobFunction; 10] = [
    KnobFunction::High,
    KnobFunction::Low,
    KnobFunction::Width,
    KnobFunction::Shift,
    KnobFunction::Rit,
    KnobFunction::Xit,
    KnobFunction::RxVol,
    KnobFunction::MonVol,
    KnobFunction::CwSpeed,
    KnobFunction::TuneDrive,
];
const STEP_CHOICES: [u64; 7] = [1, 10, 100, 1_000, 10_000, 100_000, 1_000_000];
const HIGHLIGHT: Duration = Duration::from_millis(1500);
const ALL_SEGMENTS_FOR: Duration = Duration::from_secs(20);

#[derive(PartialEq, Eq, Clone, Copy, Debug)]
enum Tab {
    Buttons,
    Knobs,
    Tuning,
    Display,
    Radio,
    Log,
}

const TABS: [(Tab, &str); 6] = [
    (Tab::Buttons, "Buttons"),
    (Tab::Knobs, "Knobs"),
    (Tab::Tuning, "Tuning"),
    (Tab::Display, "Display"),
    (Tab::Radio, "Radio"),
    (Tab::Log, "Log"),
];

/// Full mode: opens at a size where every tab fits without scrolling.
const FULL_DEFAULT: egui::Vec2 = egui::vec2(620.0, 740.0);
const FULL_MIN: egui::Vec2 = egui::vec2(560.0, 520.0);
/// Mini mode: the status line and the mimic.
const MINI_DEFAULT: egui::Vec2 = egui::vec2(480.0, 225.0);
const MINI_MIN: egui::Vec2 = egui::vec2(220.0, 100.0);
/// The mimic's height in full mode: dragged with the bar under it. It
/// can't grow past what the window's width allows.
const LCD_DEFAULT_H: f32 = 200.0;
const LCD_MIN_H: f32 = 60.0;
/// The grab bar under the mimic.
const GRIP_H: f32 = 12.0;

/// How the window looks on this machine. Kept in eframe's own storage
/// (`%APPDATA%\tmate2-connector\data` on Windows, `~/.local/share/
/// tmate2-connector` on Linux), not in config.toml: screens differ between
/// PCs, and the config file can be copied from one to another.
#[derive(Clone, Copy, Debug, PartialEq)]
struct WindowState {
    mini: bool,
    /// Mini mode stays above other windows.
    on_top: bool,
    /// Each mode's last size, so switching goes back to it.
    full: Option<egui::Vec2>,
    mini_size: Option<egui::Vec2>,
    tab: Tab,
    /// The mimic's height in full mode.
    lcd_h: f32,
}

impl Default for WindowState {
    fn default() -> Self {
        WindowState {
            mini: false,
            on_top: false,
            full: None,
            mini_size: None,
            tab: Tab::Buttons,
            lcd_h: LCD_DEFAULT_H,
        }
    }
}

const WINDOW_KEY: &str = "tmate2-window";

impl WindowState {
    /// `mini=0;on_top=0;full=620x740;mini_size=480x225;tab=Buttons;lcd_h=200`
    fn to_text(self) -> String {
        let size = |s: Option<egui::Vec2>| s.map_or_else(String::new, |s| format!("{:.0}x{:.0}", s.x, s.y));
        format!(
            "mini={};on_top={};full={};mini_size={};tab={:?};lcd_h={:.0}",
            u8::from(self.mini),
            u8::from(self.on_top),
            size(self.full),
            size(self.mini_size),
            self.tab,
            self.lcd_h
        )
    }

    /// Anything missing or unreadable keeps its default.
    fn parse(text: &str) -> WindowState {
        let mut w = WindowState::default();
        let size = |v: &str| {
            let (x, y) = v.split_once('x')?;
            let s = egui::vec2(x.trim().parse().ok()?, y.trim().parse().ok()?);
            (s.x >= 50.0 && s.y >= 50.0 && s.x <= 10_000.0 && s.y <= 10_000.0).then_some(s)
        };
        for (k, v) in text.split(';').filter_map(|kv| kv.split_once('=')) {
            match k.trim() {
                "mini" => w.mini = v.trim() == "1",
                "on_top" => w.on_top = v.trim() == "1",
                "full" => w.full = size(v),
                "mini_size" => w.mini_size = size(v),
                "lcd_h" => {
                    if let Ok(h) = v.trim().parse::<f32>() {
                        w.lcd_h = h.clamp(LCD_MIN_H, 10_000.0);
                    }
                }
                "tab" => {
                    if let Some((t, _)) = TABS.iter().find(|(t, _)| format!("{t:?}") == v.trim()) {
                        w.tab = *t;
                    }
                }
                _ => {}
            }
        }
        w
    }

    fn size_now(&mut self) -> &mut Option<egui::Vec2> {
        if self.mini {
            &mut self.mini_size
        } else {
            &mut self.full
        }
    }
}

struct App {
    shared: Arc<Shared>,
    path: PathBuf,
    map: SegmentMap,
    /// As stored in the config file.
    saved: Config,
    /// Being edited; applied to the TMATE-2 live, written on Save.
    draft: Config,
    /// What the device loop is currently running with.
    applied: Config,
    mode_cycle_text: String,
    status: Option<(String, bool)>,
    radio_cfg_tx: watch::Sender<RadioConfig>,
    window: WindowState,
    /// The first frame has set the window's minimum size and stacking.
    window_set_up: bool,
    /// A mode switch asked for this size: until the window has it (or a
    /// second has passed), its size isn't recorded for the new mode.
    resizing: Option<(egui::Vec2, std::time::Instant)>,
}

pub fn run(
    shared: Arc<Shared>,
    cfg: Config,
    path: PathBuf,
    radio_cfg_tx: watch::Sender<RadioConfig>,
) -> Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("TMATE-2 Connector")
            // Names eframe's storage folder (see WindowState).
            .with_app_id("tmate2-connector")
            .with_inner_size(FULL_DEFAULT)
            // The smaller mode's minimum, so a window last closed in mini
            // mode comes back at its size; the first frame sets full mode's.
            .with_min_inner_size(MINI_MIN),
        ..Default::default()
    };
    let mut app = App {
        shared,
        path,
        map: SegmentMap::builtin(),
        mode_cycle_text: cfg.tuning.mode_cycle.join(", "),
        saved: cfg.clone(),
        applied: cfg.clone(),
        draft: cfg,
        status: None,
        radio_cfg_tx,
        window: WindowState::default(),
        window_set_up: false,
        resizing: None,
    };
    eframe::run_native(
        "TMATE-2 Connector",
        options,
        Box::new(|cc| {
            if let Some(text) = cc.storage.and_then(|s| s.get_string(WINDOW_KEY)) {
                app.window = WindowState::parse(&text);
            }
            Ok(Box::new(app))
        }),
    )
    .map_err(|e| anyhow::anyhow!("GUI failed: {e}"))
}

fn dot(ui: &mut egui::Ui, ok: bool) {
    let c = if ok {
        Color32::from_rgb(60, 200, 90)
    } else {
        Color32::from_rgb(220, 70, 60)
    };
    // Painted rather than a "●" character: egui's bundled fonts lack it.
    let h = ui.text_style_height(&egui::TextStyle::Body);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(h, h), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), h * 0.35, c);
}

fn button_label(b: Button) -> &'static str {
    match b {
        Button::F1 => "F1",
        Button::F2 => "F2",
        Button::F3 => "F3",
        Button::F4 => "F4",
        Button::F5 => "F5",
        Button::F6 => "F6",
        Button::MainPush => "Main knob push",
        Button::E1Push => "E1 push",
        Button::E2Push => "E2 push",
    }
}

/// Pick an action: "none", any plain action, "mode:…", "agc:…" or "raw:…".
fn action_picker(ui: &mut egui::Ui, id: impl egui::AsIdSalt, value: &mut Option<Action>) {
    let text = value
        .as_ref()
        .map_or_else(|| "none".to_string(), Action::to_string);
    let id = ui.make_persistent_id(id);
    egui::ComboBox::from_id_salt(id)
        .width(150.0)
        .selected_text(text)
        .show_ui(ui, |ui| {
            ui.selectable_value(value, None, "none");
            for a in Action::simple().filter(|a| **a != Action::None) {
                ui.selectable_value(value, Some(a.clone()), a.to_string());
            }
            if ui
                .selectable_label(matches!(value, Some(Action::Mode(_))), "mode:…")
                .clicked()
            {
                *value = Some(Action::Mode("USB".into()));
            }
            if ui
                .selectable_label(matches!(value, Some(Action::Agc(_))), "agc:…")
                .clicked()
            {
                *value = Some(Action::Agc("fast".into()));
            }
            if ui
                .selectable_label(matches!(value, Some(Action::Raw(_))), "raw:…")
                .clicked()
            {
                *value = Some(Action::Raw(String::new()));
            }
        });
    match value {
        Some(Action::Mode(m)) => {
            ui.add(egui::TextEdit::singleline(m).desired_width(60.0).hint_text("USB"));
        }
        Some(Action::Agc(m)) => {
            egui::ComboBox::from_id_salt(id.with("agc"))
                .width(80.0)
                .selected_text(m.as_str())
                .show_ui(ui, |ui| {
                    for a in AGC_MODES {
                        ui.selectable_value(m, a.to_string(), a);
                    }
                });
        }
        Some(Action::Raw(r)) => {
            ui.add(
                egui::TextEdit::singleline(r)
                    .desired_width(120.0)
                    .hint_text("trx:0,true;"),
            );
        }
        _ => {}
    }
}

/// Colour swatches that set `rgb` when clicked.
fn presets(ui: &mut egui::Ui, id: &str, rgb: &mut [u8; 3]) {
    ui.horizontal(|ui| {
        for (name, c) in PRESETS {
            let [r, g, b] = c;
            let selected = *rgb == c;
            let swatch = egui::Button::new("")
                .fill(Color32::from_rgb(r, g, b))
                .stroke(if selected {
                    egui::Stroke::new(2.0_f32, ui.visuals().strong_text_color())
                } else {
                    egui::Stroke::new(1.0_f32, Color32::from_gray(90))
                })
                .min_size(egui::vec2(20.0, 20.0));
            if ui
                .push_id((id, name), |ui| ui.add(swatch))
                .inner
                .on_hover_text(name)
                .clicked()
            {
                *rgb = c;
            }
        }
    });
}

fn start_picker(ui: &mut egui::Ui, id: &str, knob: &mut tmate2_core::config::KnobConfig) {
    // If the starting function was unticked, start on the first one instead.
    if knob.start.is_some_and(|s| !knob.functions.contains(&s)) {
        knob.start = knob.functions.first().copied();
    }
    let current = knob.functions[knob.start_index()];
    ui.horizontal(|ui| {
        ui.label("Starts on");
        egui::ComboBox::from_id_salt(id)
            .selected_text(current.name().to_uppercase())
            .show_ui(ui, |ui| {
                for f in knob.functions.clone() {
                    if ui
                        .selectable_label(f == current, f.name().to_uppercase())
                        .clicked()
                    {
                        knob.start = Some(f);
                    }
                }
            });
    });
}

fn knob_functions(ui: &mut egui::Ui, choices: &[KnobFunction], selected: &mut Vec<KnobFunction>) {
    ui.horizontal_wrapped(|ui| {
        for (i, f) in choices.iter().enumerate() {
            if i > 0 && !f.has_lcd_label() && choices[i - 1].has_lcd_label() {
                ui.separator();
                ui.label(RichText::new("no LCD label:").weak());
            }
            let mut on = selected.contains(f);
            if ui
                .checkbox(&mut on, f.name().replace('_', " ").to_uppercase())
                .changed()
            {
                if on {
                    selected.push(*f);
                } else if selected.len() > 1 {
                    selected.retain(|x| x != f);
                }
                // Keep the LCD's left-to-right order.
                selected.sort_by_key(|x| choices.iter().position(|c| c == x));
            }
        }
    });
}

impl App {
    fn recently_pressed(&self) -> Option<Button> {
        let last = *self.shared.last_input.lock().unwrap();
        match last {
            Some((InputEvent::Pressed(b), at)) if at.elapsed() < HIGHLIGHT => Some(b),
            Some((InputEvent::Turned { encoder, .. }, at)) if at.elapsed() < HIGHLIGHT => match encoder {
                Encoder::Main => Some(Button::MainPush),
                Encoder::E1 => Some(Button::E1Push),
                Encoder::E2 => Some(Button::E2Push),
            },
            _ => None,
        }
    }

    /// The server connected to: its friendly name from `[radio.names]` (as
    /// being edited, so a new name shows at once), and its address.
    fn server(&self) -> (Option<String>, String) {
        let mut r = self.saved.radio.clone();
        r.names = self.draft.radio.names.clone();
        (r.server_name().map(str::to_string), server_label(&r.host, r.port))
    }

    fn status_bar(&self, ui: &mut egui::Ui) {
        let radio = self.shared.radio.lock().unwrap().clone();
        ui.horizontal_wrapped(|ui| {
            dot(ui, self.shared.device_connected.load(Ordering::SeqCst));
            ui.label("TMATE-2");
            ui.separator();
            dot(ui, radio.connected);
            let r = &self.saved.radio;
            if radio.connected {
                // Not the `device:` the server reports: TCI servers all name
                // a SunSDR model whatever the radio is.
                let protocol = radio.protocol.as_deref().unwrap_or("?");
                ui.label(match self.server() {
                    (Some(name), at) => format!("Radio: {name} (TCI {at}, {protocol})"),
                    (None, at) => format!("Radio: TCI {at} ({protocol})"),
                });
                ui.separator();
                ui.label(match &radio.tcix {
                    Some(m) => format!("TCI-X: yes ({} capabilities)", m.len()),
                    None if r.tcix == TcixMode::Off => "TCI-X: off".into(),
                    None => "TCI-X: not offered".into(),
                });
                if let (Some(user), Some(role)) = (&radio.login_user, &radio.role) {
                    ui.separator();
                    ui.label(format!("Logged in: {user} ({role})"));
                }
                if radio.has_tcix("control_request") {
                    ui.separator();
                    ui.label(match &radio.control {
                        _ if radio.in_control() => "Control: you".to_string(),
                        // Only a login of our own tells us we aren't the
                        // operator: through a relay (KADRAD's) we act
                        // under its login, which may be this one.
                        Some(op) if radio.login_user.is_some() => format!("Control: {op} (you're watching)"),
                        Some(op) => format!("Control: {op}"),
                        None => "Control: nobody".to_string(),
                    });
                }
            } else {
                match radio.notice.as_deref() {
                    Some("login") => ui.label(format!(
                        "Radio: {}:{} needs a TCI-X login (see the log)",
                        r.host, r.port
                    )),
                    Some("idle") => ui.label(format!(
                        "Radio: {}:{} closed the connection after the idle time; touch the TMATE-2 to reconnect",
                        r.host, r.port
                    )),
                    _ => ui.label(format!("Radio: connecting to {}:{} …", r.host, r.port)),
                };
            }
        });
    }

    fn buttons_tab(&mut self, ui: &mut egui::Ui) {
        ui.label("Press a button on the TMATE-2 to highlight its row.");
        ui.add_space(4.0);
        let pressed = self.recently_pressed();
        egui::Grid::new("buttons")
            .striped(true)
            .num_columns(3)
            .spacing([16.0, 6.0])
            .show(ui, |ui| {
                ui.strong("Control");
                ui.strong("Tap (short press)");
                ui.strong("Hold");
                ui.end_row();
                for b in Button::ALL {
                    let label = button_label(b);
                    if pressed == Some(b) {
                        ui.label(
                            RichText::new(format!("> {label}"))
                                .strong()
                                .color(Color32::from_rgb(255, 190, 40)),
                        );
                    } else {
                        ui.label(label);
                    }
                    let ButtonAction { tap, hold } = self.draft.buttons.get_mut(b);
                    // A text box for mode: / agc: / raw: goes under its
                    // picker, so the row never needs more width.
                    ui.vertical(|ui| action_picker(ui, (label, "tap"), tap));
                    ui.vertical(|ui| action_picker(ui, (label, "hold"), hold));
                    ui.end_row();
                }
            });
        ui.add_space(8.0);
        let b = &mut self.draft.buttons;
        egui::Grid::new("timing").num_columns(2).show(ui, |ui| {
            ui.label("Hold after (ms)");
            ui.add(egui::DragValue::new(&mut b.hold_ms).range(150..=3000).speed(10));
            ui.end_row();
            ui.label("Repeat while held (ms, 0 = off)");
            ui.add(
                egui::DragValue::new(&mut b.hold_repeat_ms)
                    .range(0..=3000)
                    .speed(10),
            );
            ui.end_row();
            ui.label("Ignore knob push after turning (ms)");
            ui.add(
                egui::DragValue::new(&mut b.push_lockout_ms)
                    .range(0..=2000)
                    .speed(10),
            );
            ui.end_row();
        });
    }

    fn knobs_tab(&mut self, ui: &mut egui::Ui) {
        ui.label("Push a knob to step through its functions. The LCD's bottom row shows the current one.");
        ui.add_space(6.0);
        ui.strong("E1 (left knob)");
        knob_functions(ui, &E1_CHOICES, &mut self.draft.e1.functions);
        start_picker(ui, "e1start", &mut self.draft.e1);
        ui.add_space(6.0);
        ui.strong("E2 (right knob)");
        knob_functions(ui, &E2_CHOICES, &mut self.draft.e2.functions);
        start_picker(ui, "e2start", &mut self.draft.e2);
        ui.add_space(6.0);
        let t = &mut self.draft.tuning;
        egui::Grid::new("knobsteps").num_columns(2).show(ui, |ui| {
            ui.label("Filter change per click (Hz)");
            ui.add(egui::DragValue::new(&mut t.filter_step_hz).range(1..=1000));
            ui.end_row();
            ui.label("RIT / XIT change per click (Hz)");
            ui.add(egui::DragValue::new(&mut t.rit_step_hz).range(1..=1000));
            ui.end_row();
            ui.label("E1 / E2 counts per click");
            ui.add(egui::DragValue::new(&mut t.counts_per_detent).range(1..=8));
            ui.end_row();
        });
    }

    fn tuning_tab(&mut self, ui: &mut egui::Ui) {
        let t = &mut self.draft.tuning;
        // The steps wrap on a narrow window, so they sit above the grid
        // rather than in it: a grid row centres its cells, and the second
        // line would land on the row below.
        ui.label("Tuning steps");
        ui.horizontal_wrapped(|ui| {
            for s in STEP_CHOICES {
                let mut on = t.steps_hz.contains(&s);
                if ui.checkbox(&mut on, step_text(s)).changed() {
                    if on {
                        t.steps_hz.push(s);
                        t.steps_hz.sort_unstable();
                    } else if t.steps_hz.len() > 1 {
                        t.steps_hz.retain(|x| *x != s);
                    }
                }
            }
        });
        if !t.steps_hz.contains(&t.step_hz) {
            t.step_hz = t.steps_hz[0];
        }
        ui.add_space(6.0);
        egui::Grid::new("tuning")
            .num_columns(2)
            .spacing([16.0, 8.0])
            .show(ui, |ui| {
                ui.label("Acceleration");
                ui.add(
                    egui::Slider::new(&mut t.acceleration, 0.0..=10.0)
                        .step_by(0.5)
                        .text("0 off · 5 normal · 10 strong"),
                );
                ui.end_row();
                ui.label("Starting step");
                egui::ComboBox::from_id_salt("startstep")
                    .selected_text(step_text(t.step_hz))
                    .show_ui(ui, |ui| {
                        for s in t.steps_hz.clone() {
                            ui.selectable_value(&mut t.step_hz, s, step_text(s));
                        }
                    });
                ui.end_row();
                ui.label("fast multiplies the step by");
                ui.add(egui::DragValue::new(&mut t.fast_multiplier).range(1..=1000));
                ui.end_row();
                ui.label("Modes for mode_next / mode_prev");
                ui.text_edit_singleline(&mut self.mode_cycle_text);
                ui.end_row();
            });
    }

    fn display_tab(&mut self, ui: &mut egui::Ui) {
        let d = &mut self.draft.display;
        egui::Grid::new("display")
            .num_columns(2)
            .spacing([16.0, 8.0])
            .show(ui, |ui| {
                ui.label("Contrast");
                ui.add(
                    egui::Slider::new(&mut d.contrast, 0..=100)
                        .suffix(" %")
                        .text("100 % darkest"),
                );
                ui.end_row();
                for (label, id, rgb) in [
                    ("Backlight", "bl", &mut d.backlight),
                    ("Backlight when transmitting", "bltx", &mut d.backlight_tx),
                    ("Backlight with no radio", "blnr", &mut d.backlight_disconnected),
                ] {
                    ui.label(label);
                    ui.horizontal(|ui| {
                        egui::color_picker::color_edit_button_srgb(ui, rgb);
                        presets(ui, id, rgb);
                    });
                    ui.end_row();
                }
                ui.label("Small readout");
                ui.horizontal(|ui| {
                    ui.radio_value(&mut d.readout, Readout::Dbm, "-dBm");
                    ui.radio_value(&mut d.readout, Readout::SUnits, "S units");
                    ui.radio_value(&mut d.readout, Readout::Off, "off");
                });
                ui.end_row();
                ui.label("Volume (E1 VOL)");
                ui.horizontal(|ui| {
                    ui.radio_value(&mut d.volume_scale, VolumeScale::Percent, "0-100 (like Thetis)");
                    ui.radio_value(&mut d.volume_scale, VolumeScale::Db, "dB (-60 to 0)");
                });
                ui.end_row();
                ui.label("Transmit bargraph");
                ui.horizontal(|ui| {
                    ui.radio_value(&mut d.tx_meter, TxMeter::Power, "power");
                    ui.radio_value(&mut d.tx_meter, TxMeter::Swr, "SWR (1.8:1 at S9)");
                    ui.radio_value(&mut d.tx_meter, TxMeter::Off, "off");
                });
                ui.end_row();
                ui.label("Transmit readout");
                ui.horizontal(|ui| {
                    ui.radio_value(&mut d.tx_readout, TxReadout::Watts, "watts");
                    ui.radio_value(&mut d.tx_readout, TxReadout::Swr, "SWR (1_2 = 1.2:1)");
                    ui.radio_value(&mut d.tx_readout, TxReadout::Off, "off");
                });
                ui.end_row();
                ui.label("Full-scale power (W)");
                ui.add(egui::DragValue::new(&mut d.tx_power_max_w).range(1..=2000));
                ui.end_row();
                ui.label("Show knob values for (ms)");
                ui.add(egui::DragValue::new(&mut d.flash_ms).range(0..=5000).speed(50));
                ui.end_row();
                ui.label("This window opens in").on_hover_text(
                    "Mini: the connection status and the LCD. Full: with all the settings.                      Takes effect next time the connector starts.",
                );
                let w = &mut self.draft.window;
                ui.horizontal(|ui| {
                    ui.radio_value(&mut w.start, WindowMode::Mini, "mini");
                    ui.radio_value(&mut w.start, WindowMode::Full, "full");
                });
                ui.end_row();
            });
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            if ui.button("Light all segments for 20 s").clicked() {
                *self.shared.all_segments_until.lock().unwrap() =
                    Some(std::time::Instant::now() + ALL_SEGMENTS_FOR);
            }
            ui.label("Display test.");
        });
    }

    fn radio_tab(&mut self, ui: &mut egui::Ui) {
        let r = &mut self.draft.radio;
        egui::Grid::new("radio")
            .num_columns(2)
            .spacing([16.0, 8.0])
            .show(ui, |ui| {
                ui.label("Connection");
                ui.label("TCI");
                ui.end_row();
                ui.label("Host");
                ui.text_edit_singleline(&mut r.host);
                ui.end_row();
                ui.label("Port");
                ui.add(egui::DragValue::new(&mut r.port).range(1..=65535));
                ui.end_row();
                ui.label("Name").on_hover_text(
                    "Shown instead of the address, e.g. Thetis. Kept in [radio.names] \
                     for this host and port, so each server keeps its own.",
                );
                let mut name = r
                    .name_key()
                    .and_then(|k| r.names.get(k))
                    .cloned()
                    .unwrap_or_default();
                if ui
                    .add(egui::TextEdit::singleline(&mut name).hint_text(server_label(&r.host, r.port)))
                    .changed()
                {
                    r.set_server_name(&name);
                }
                ui.end_row();
                ui.label("TCI-X extensions");
                ui.horizontal(|ui| {
                    ui.radio_value(&mut r.tcix, TcixMode::Off, "off");
                    ui.radio_value(&mut r.tcix, TcixMode::Auto, "auto");
                    ui.radio_value(&mut r.tcix, TcixMode::On, "on");
                });
                ui.end_row();
                ui.label("Pacing").on_hover_text(
                    "For a radio that can't keep up with the dial, e.g. one reached over a network",
                );
                ui.horizontal(|ui| {
                    ui.radio_value(&mut r.pacing, Pacing::Off, "off");
                    ui.radio_value(&mut r.pacing, Pacing::Auto, "auto");
                    ui.radio_value(&mut r.pacing, Pacing::On, "on");
                });
                ui.end_row();
                ui.label("Receiver (trx)");
                ui.add(egui::DragValue::new(&mut r.trx).range(0..=7));
                ui.end_row();
                ui.label("Poll").on_hover_text(
                    "How often to read what the LCD shows from the radio, for software that \
                     doesn't report changes made in its own window. Applies at once.",
                );
                ui.horizontal(|ui| {
                    let mut on = r.poll_ms != 0;
                    if ui.checkbox(&mut on, "every").changed() {
                        r.poll_ms = if on { 1000 } else { 0 };
                    }
                    // The box clamps whatever it holds into its range, even
                    // greyed out, so while off it shows a copy, not the 0.
                    let mut off_copy = 1000;
                    let value = if on { &mut r.poll_ms } else { &mut off_copy };
                    ui.add_enabled(
                        on,
                        egui::DragValue::new(value)
                            .range(POLL_MIN_MS..=10_000)
                            .speed(10)
                            .suffix(" ms"),
                    );
                    if !on {
                        ui.label("off");
                    }
                });
                ui.end_row();
            });
        ui.add_space(6.0);
        ui.label("TCI-X auto: uses TCI-X if the server offers it, otherwise plain TCI.");
        ui.label("Pacing auto: sends every step while the radio keeps up, and waits for its reply once it falls behind.");
    }

    fn log_tab(&mut self, ui: &mut egui::Ui) {
        let lines: Vec<String> = self.shared.log.lock().unwrap().iter().cloned().collect();
        egui::ScrollArea::vertical()
            .stick_to_bottom(true)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for l in lines {
                    ui.monospace(l);
                }
            });
    }

    /// The draft as the device loop should run it right now.
    fn live(&self) -> Config {
        let mut c = self.draft.clone();
        let modes: Vec<String> = self
            .mode_cycle_text
            .split(',')
            .map(|m| m.trim().to_uppercase())
            .filter(|m| !m.is_empty())
            .collect();
        if !modes.is_empty() {
            c.tuning.mode_cycle = modes;
        }
        // Radio connection changes wait for Save (no reconnect per keystroke).
        c.radio = self.saved.radio.clone();
        c.radio.trx = self.draft.radio.trx;
        c.radio.poll_ms = self.draft.radio.poll_ms;
        c
    }

    /// Send the draft to the TMATE-2 whenever it changes.
    fn apply_live(&mut self) {
        let live = self.live();
        if live != self.applied {
            let _ = self.shared.config_tx.lock().unwrap().send(live.clone());
            self.applied = live;
        }
    }

    fn save(&mut self) {
        let radio = self.draft.radio.clone();
        self.draft = self.live();
        self.draft.radio = radio;
        let result = (|| -> Result<Config, String> {
            let text = std::fs::read_to_string(&self.path).map_err(|e| e.to_string())?;
            let mut doc: toml_edit::DocumentMut = text.parse().map_err(|e| format!("{e}"))?;
            config_edit::write(&mut doc, &self.draft);
            let new_text = doc.to_string();
            let cfg = Config::parse(&new_text).map_err(|e| e.to_string())?;
            std::fs::write(&self.path, new_text).map_err(|e| e.to_string())?;
            Ok(cfg)
        })();
        match result {
            Ok(cfg) => {
                // Reconnect only for a change the connection uses: the poll
                // period is the engine's, and already applied live, and the
                // server names are only shown.
                let connection = RadioConfig {
                    poll_ms: self.saved.radio.poll_ms,
                    names: self.saved.radio.names.clone(),
                    ..cfg.radio.clone()
                };
                if connection != self.saved.radio {
                    let _ = self.radio_cfg_tx.send(cfg.radio.clone());
                }
                let _ = self.shared.config_tx.lock().unwrap().send(cfg.clone());
                self.saved = cfg.clone();
                self.applied = cfg.clone();
                self.draft = cfg;
                self.status = Some(("Saved.".into(), false));
            }
            Err(e) => self.status = Some((format!("Not saved: {e}"), true)),
        }
    }

    fn dirty(&self) -> bool {
        self.draft != self.saved || self.mode_cycle_text != self.saved.tuning.mode_cycle.join(", ")
    }
}

fn step_text(hz: u64) -> String {
    match hz {
        h if h >= 1_000_000 && h % 1_000_000 == 0 => format!("{} MHz", h / 1_000_000),
        h if h >= 1_000 && h % 1_000 == 0 => format!("{} kHz", h / 1_000),
        h => format!("{h} Hz"),
    }
}

impl App {
    /// Switch between full and mini mode, each at its own last size. Mini
    /// mode has no title bar.
    fn set_mini(&mut self, ctx: &egui::Context, mini: bool) {
        if mini == self.window.mini {
            return;
        }
        self.window.mini = mini;
        ctx.send_viewport_cmd(egui::ViewportCommand::Decorations(!mini));
        let (size, min) = if mini {
            (self.window.mini_size.unwrap_or(MINI_DEFAULT), MINI_MIN)
        } else {
            (self.window.full.unwrap_or(FULL_DEFAULT), FULL_MIN)
        };
        ctx.send_viewport_cmd(egui::ViewportCommand::MinInnerSize(min));
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
        self.resizing = Some((size, std::time::Instant::now()));
        self.apply_window_level(ctx);
    }

    /// Above other windows only in mini mode, and only if asked for.
    fn apply_window_level(&self, ctx: &egui::Context) {
        let level = if self.window.mini && self.window.on_top {
            egui::WindowLevel::AlwaysOnTop
        } else {
            egui::WindowLevel::Normal
        };
        ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(level));
    }

    /// Remember the window's size for the mode it's in, once a mode switch
    /// has finished resizing it.
    fn note_window_size(&mut self, ctx: &egui::Context) {
        let Some(now) = ctx.input(|i| i.viewport().inner_rect).map(|r| r.size()) else {
            return;
        };
        if let Some((want, since)) = self.resizing {
            let arrived = (now - want).length() < 2.0;
            if !arrived && since.elapsed() < Duration::from_secs(1) {
                return;
            }
            self.resizing = None;
        }
        *self.window.size_now() = Some(now);
    }

    /// Mini mode: the connection status and the mimic, with no title bar.
    /// Drag anywhere but a button to move the window, use the corner grip
    /// to size it, and double-click to go back to full mode.
    fn mini_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let radio = self.shared.radio.lock().unwrap().clone();
        egui::CentralPanel::default().show(ui, |ui| {
            // Behind everything else, so the buttons still take their clicks.
            let back = ui.interact(
                ui.max_rect(),
                egui::Id::unique("mini-move"),
                egui::Sense::click_and_drag(),
            );
            if back.drag_started_by(egui::PointerButton::Primary) {
                ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
            }
            if back.double_clicked() {
                self.set_mini(ctx, false);
            }
            ui.horizontal(|ui| {
                dot(ui, self.shared.device_connected.load(Ordering::SeqCst));
                ui.label("TMATE-2");
                dot(ui, radio.connected);
                // The server's friendly name, or where it is: not the
                // `device:` it reports, as TCI servers all name a SunSDR
                // model whatever the radio is.
                let (name, at) = self.server();
                ui.label(name.as_deref().unwrap_or(&at))
                    .on_hover_text(if radio.connected {
                        format!("Connected to the TCI server at {at}")
                    } else {
                        format!("Connecting to the TCI server at {at} …")
                    });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("×").on_hover_text("Close the connector").clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                    if ui
                        .button("Full")
                        .on_hover_text("All the settings (or double-click)")
                        .clicked()
                    {
                        self.set_mini(ctx, false);
                    }
                    if ui
                        .checkbox(&mut self.window.on_top, "On top")
                        .on_hover_text("Keep this window above the others")
                        .changed()
                    {
                        self.apply_window_level(ctx);
                    }
                });
            });
            let report = self.shared.lcd.lock().unwrap().clone();
            let space = ui.available_size();
            lcd::show(ui, space, &report, &self.map);
            resize_corner(ui, ctx);
        });
    }

    /// Full mode: status, the mimic with a grab bar to size it, the
    /// settings tabs, and Save / Revert.
    fn full_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        egui::Panel::top("status").show(ui, |ui| {
            ui.add_space(2.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                if ui
                    .button("Mini")
                    .on_hover_text("Just the status and the LCD")
                    .clicked()
                {
                    self.set_mini(ctx, true);
                }
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    self.status_bar(ui)
                });
            });
            ui.add_space(2.0);
        });

        // The mimic, with a grab bar under it to size it apart from the
        // window. Its height is kept with the window state.
        egui::Panel::top("lcd").resizable(false).show(ui, |ui| {
            let report = self.shared.lcd.lock().unwrap().clone();
            let width = ui.available_width();
            let fits = width / lcd::ASPECT;
            let h = self.window.lcd_h.clamp(LCD_MIN_H, fits.max(LCD_MIN_H));
            lcd::show(ui, egui::vec2(width, h), &report, &self.map);
            if let Some(dy) = grip(ui) {
                // Start from what is shown, so dragging up from a mimic held
                // back by the window's width moves it at once.
                self.window.lcd_h = (h + dy).clamp(LCD_MIN_H, fits.max(LCD_MIN_H));
            }
        });

        self.actions_bar(ui);

        egui::CentralPanel::default().show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                for (t, name) in TABS {
                    ui.selectable_value(&mut self.window.tab, t, name);
                }
            });
            ui.separator();
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| match self.window.tab {
                    Tab::Buttons => self.buttons_tab(ui),
                    Tab::Knobs => self.knobs_tab(ui),
                    Tab::Tuning => self.tuning_tab(ui),
                    Tab::Display => self.display_tab(ui),
                    Tab::Radio => self.radio_tab(ui),
                    Tab::Log => self.log_tab(ui),
                });
        });
    }

    fn actions_bar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::bottom("actions").show(ui, |ui| {
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                let dirty = self.dirty();
                if ui.add_enabled(dirty, egui::Button::new("Save")).clicked() {
                    self.save();
                }
                if ui.add_enabled(dirty, egui::Button::new("Revert")).clicked() {
                    self.draft = self.saved.clone();
                    self.mode_cycle_text = self.saved.tuning.mode_cycle.join(", ");
                    self.status = None;
                }
                if dirty {
                    ui.label(
                        RichText::new("Unsaved changes (already live on the TMATE-2; Revert undoes them)")
                            .color(Color32::from_rgb(255, 190, 40)),
                    );
                } else if let Some((msg, err)) = &self.status {
                    let c = if *err {
                        Color32::from_rgb(230, 80, 70)
                    } else {
                        Color32::from_rgb(60, 200, 90)
                    };
                    ui.label(RichText::new(msg).color(c));
                }
                // Wraps onto its own line when the window is narrow.
                ui.label(RichText::new(self.path.display().to_string()).small().weak());
            });
            ui.add_space(4.0);
        });
    }
}

/// With no title bar, a grip in the window's lower right corner to size it.
fn resize_corner(ui: &mut egui::Ui, ctx: &egui::Context) {
    let corner = ui.ctx().content_rect().max;
    let rect = egui::Rect::from_min_max(corner - egui::vec2(14.0, 14.0), corner);
    let response = ui
        .interact(rect, egui::Id::unique("mini-resize"), egui::Sense::drag())
        .on_hover_cursor(egui::CursorIcon::ResizeSouthEast);
    if response.drag_started_by(egui::PointerButton::Primary) {
        ctx.send_viewport_cmd(egui::ViewportCommand::BeginResize(
            egui::ResizeDirection::SouthEast,
        ));
    }
    let c = if response.hovered() {
        ui.visuals().strong_text_color()
    } else {
        ui.visuals().weak_text_color()
    };
    let painter = ui.ctx().layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::unique("mini-grip"),
    ));
    for d in [4.0, 8.0, 12.0] {
        painter.line_segment(
            [corner - egui::vec2(d, 2.0), corner - egui::vec2(2.0, d)],
            egui::Stroke::new(1.0, c),
        );
    }
}

/// The grab bar under the mimic: a short double line, and a vertical
/// resize cursor over it. Returns how far it was dragged this frame.
fn grip(ui: &mut egui::Ui) -> Option<f32> {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), GRIP_H), egui::Sense::drag());
    let response = response.on_hover_cursor(egui::CursorIcon::ResizeVertical);
    let c = if response.hovered() || response.dragged() {
        ui.visuals().strong_text_color()
    } else {
        ui.visuals().weak_text_color()
    };
    for dy in [-2.0, 2.0] {
        let y = rect.center().y + dy;
        ui.painter().line_segment(
            [
                egui::pos2(rect.center().x - 20.0, y),
                egui::pos2(rect.center().x + 20.0, y),
            ],
            egui::Stroke::new(1.0, c),
        );
    }
    response.dragged().then(|| response.drag_delta().y)
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if self.shared.stop.load(Ordering::SeqCst) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        ctx.request_repaint_after(Duration::from_millis(50));

        if !self.window_set_up {
            // The window came back as it was last closed (`window.mini`);
            // open it in the mode the config asks for, at that mode's size.
            self.window_set_up = true;
            let min = if self.window.mini { MINI_MIN } else { FULL_MIN };
            ctx.send_viewport_cmd(egui::ViewportCommand::MinInnerSize(min));
            ctx.send_viewport_cmd(egui::ViewportCommand::Decorations(!self.window.mini));
            self.apply_window_level(&ctx);
            self.set_mini(&ctx, self.saved.window.start == WindowMode::Mini);
        }
        self.note_window_size(&ctx);

        if self.window.mini {
            self.mini_ui(ui, &ctx);
        } else {
            self.full_ui(ui, &ctx);
        }

        self.apply_live();
    }

    /// eframe calls this now and then and on exit; it also saves the
    /// window's place and egui's panel sizes (the mimic's height).
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        storage.set_string(WINDOW_KEY, self.window.to_text());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_state_round_trips() {
        let w = WindowState {
            mini: true,
            on_top: true,
            full: Some(egui::vec2(633.0, 712.0)),
            mini_size: Some(egui::vec2(400.0, 160.0)),
            tab: Tab::Radio,
            lcd_h: 150.0,
        };
        assert_eq!(WindowState::parse(&w.to_text()), w);
        assert_eq!(
            WindowState::parse(&WindowState::default().to_text()),
            WindowState::default()
        );
    }

    #[test]
    fn a_damaged_window_state_falls_back_to_defaults() {
        assert_eq!(WindowState::parse(""), WindowState::default());
        let w = WindowState::parse("mini=1;full=junk;mini_size=10x10;tab=Nope;extra=1");
        assert!(w.mini);
        assert_eq!((w.full, w.mini_size, w.tab), (None, None, Tab::Buttons));
    }
}
