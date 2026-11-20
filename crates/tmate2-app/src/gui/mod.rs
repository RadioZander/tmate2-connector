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
    Action, ButtonAction, Config, KnobFunction, RadioConfig, Readout, TcixMode, VolumeScale,
};
use tmate2_hid::{Button, Encoder, InputEvent, SegmentMap};

use crate::shared::Shared;

const E1_CHOICES: [KnobFunction; 7] = [
    KnobFunction::Vol,
    KnobFunction::Sql,
    KnobFunction::Rfg,
    KnobFunction::Drv,
    KnobFunction::Nr,
    KnobFunction::Nb,
    KnobFunction::An,
];
const E2_CHOICES: [KnobFunction; 6] = [
    KnobFunction::High,
    KnobFunction::Low,
    KnobFunction::Width,
    KnobFunction::Shift,
    KnobFunction::Rit,
    KnobFunction::Xit,
];
const STEP_CHOICES: [u64; 7] = [1, 10, 100, 1_000, 10_000, 100_000, 1_000_000];
const HIGHLIGHT: Duration = Duration::from_millis(1500);
const ALL_SEGMENTS_FOR: Duration = Duration::from_secs(20);

/// Backlight presets: (name, colour).
const PRESETS: [(&str, [u8; 3]); 8] = [
    ("White", [255, 255, 255]),
    ("Warm white", [255, 180, 90]),
    ("Amber", [255, 120, 0]),
    ("Red", [255, 0, 0]),
    ("Green", [0, 255, 0]),
    ("Cyan", [0, 255, 255]),
    ("Blue", [0, 0, 255]),
    ("Rich purple", [140, 0, 255]),
];

#[derive(PartialEq, Eq, Clone, Copy)]
enum Tab {
    Buttons,
    Knobs,
    Tuning,
    Display,
    Radio,
    Log,
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
    tab: Tab,
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
            .with_inner_size([760.0, 720.0])
            .with_min_inner_size([560.0, 480.0]),
        ..Default::default()
    };
    let app = App {
        shared,
        path,
        map: SegmentMap::builtin(),
        mode_cycle_text: cfg.tuning.mode_cycle.join(", "),
        saved: cfg.clone(),
        applied: cfg.clone(),
        draft: cfg,
        status: None,
        radio_cfg_tx,
        tab: Tab::Buttons,
    };
    eframe::run_native("TMATE-2 Connector", options, Box::new(|_cc| Ok(Box::new(app))))
        .map_err(|e| anyhow::anyhow!("GUI failed: {e}"))
}

fn dot(ui: &mut egui::Ui, ok: bool) {
    let c = if ok {
        Color32::from_rgb(60, 200, 90)
    } else {
        Color32::from_rgb(220, 70, 60)
    };
    ui.label(RichText::new("●").color(c));
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

/// Pick an action: "none", any plain action, "mode:…" or "raw:…".
fn action_picker(ui: &mut egui::Ui, id: impl egui::AsIdSalt, value: &mut Option<Action>) {
    let text = value
        .as_ref()
        .map_or_else(|| "none".to_string(), Action::to_string);
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
        for f in choices {
            let mut on = selected.contains(f);
            if ui.checkbox(&mut on, f.name().to_uppercase()).changed() {
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

    fn status_bar(&self, ui: &mut egui::Ui) {
        let radio = self.shared.radio.lock().unwrap().clone();
        ui.horizontal(|ui| {
            dot(ui, self.shared.device_connected.load(Ordering::SeqCst));
            ui.label("TMATE-2");
            ui.separator();
            dot(ui, radio.connected);
            let r = &self.saved.radio;
            if radio.connected {
                ui.label(format!(
                    "Radio: {} via TCI {}:{} ({})",
                    radio.device.as_deref().unwrap_or("?"),
                    r.host,
                    r.port,
                    radio.protocol.as_deref().unwrap_or("?")
                ));
                ui.separator();
                ui.label(match &radio.tcix {
                    Some(caps) => format!("TCI-X: yes ({} capabilities)", caps.len()),
                    None if r.tcix == TcixMode::Off => "TCI-X: off".into(),
                    None => "TCI-X: not offered".into(),
                });
            } else {
                ui.label(format!("Radio: connecting to {}:{} …", r.host, r.port));
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
                            RichText::new(format!("▶ {label}"))
                                .strong()
                                .color(Color32::from_rgb(255, 190, 40)),
                        );
                    } else {
                        ui.label(label);
                    }
                    let ButtonAction { tap, hold } = self.draft.buttons.get_mut(b);
                    ui.horizontal(|ui| action_picker(ui, (label, "tap"), tap));
                    ui.horizontal(|ui| action_picker(ui, (label, "hold"), hold));
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
                ui.end_row();
                if !t.steps_hz.contains(&t.step_hz) {
                    t.step_hz = t.steps_hz[0];
                }
                ui.label("Starting step");
                egui::ComboBox::from_id_salt("startstep")
                    .selected_text(step_text(t.step_hz))
                    .show_ui(ui, |ui| {
                        for s in t.steps_hz.clone() {
                            ui.selectable_value(&mut t.step_hz, s, step_text(s));
                        }
                    });
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
                ui.label("Show knob values for (ms)");
                ui.add(egui::DragValue::new(&mut d.flash_ms).range(0..=5000).speed(50));
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
                ui.label("TCI-X extensions");
                ui.horizontal(|ui| {
                    ui.radio_value(&mut r.tcix, TcixMode::Off, "off");
                    ui.radio_value(&mut r.tcix, TcixMode::Auto, "auto");
                    ui.radio_value(&mut r.tcix, TcixMode::On, "on");
                });
                ui.end_row();
                ui.label("Receiver (trx)");
                ui.add(egui::DragValue::new(&mut r.trx).range(0..=7));
                ui.end_row();
            });
        ui.add_space(6.0);
        ui.label("auto: uses TCI-X if the server offers it, otherwise plain TCI.");
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
                if cfg.radio != self.saved.radio {
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

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if self.shared.stop.load(Ordering::SeqCst) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        ctx.request_repaint_after(Duration::from_millis(50));

        egui::Panel::top("status").show(ui, |ui| {
            ui.add_space(2.0);
            self.status_bar(ui);
            ui.add_space(2.0);
        });

        egui::Panel::bottom("actions").show(ui, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
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
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(RichText::new(self.path.display().to_string()).small().weak());
                });
            });
            ui.add_space(4.0);
        });

        egui::CentralPanel::default().show(ui, |ui| {
            let report = self.shared.lcd.lock().unwrap().clone();
            ui.vertical_centered(|ui| lcd::show(ui, &report, &self.map));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                for (t, name) in [
                    (Tab::Buttons, "Buttons"),
                    (Tab::Knobs, "Knobs"),
                    (Tab::Tuning, "Tuning"),
                    (Tab::Display, "Display"),
                    (Tab::Radio, "Radio"),
                    (Tab::Log, "Log"),
                ] {
                    ui.selectable_value(&mut self.tab, t, name);
                }
            });
            ui.separator();
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| match self.tab {
                    Tab::Buttons => self.buttons_tab(ui),
                    Tab::Knobs => self.knobs_tab(ui),
                    Tab::Tuning => self.tuning_tab(ui),
                    Tab::Display => self.display_tab(ui),
                    Tab::Radio => self.radio_tab(ui),
                    Tab::Log => self.log_tab(ui),
                });
        });

        self.apply_live();
    }
}
