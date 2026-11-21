//! The mapping engine: TMATE-2 input events in, radio commands out.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use tmate2_hid::{Button, Encoder, InputEvent};

use crate::bands::{band_of, step_band, step_in_list, BANDS};
use crate::command::Command;
use crate::config::{Action, Config, KnobFunction, VolumeScale, AGC_CYCLE, BACKLIGHT_PRESETS};
use crate::state::{Mode, RadioState};

/// How long VFO echoes from the radio are ignored after local tuning.
const VFO_GUARD: Duration = Duration::from_millis(400);
/// The same for a value set with E1 or E2 (AF gain, filter width, …), at
/// the longest: it ends as soon as the radio reports the value we hold. A
/// radio on a serial link can be half a second behind a fast-turned knob.
const KNOB_GUARD: Duration = Duration::from_millis(1500);
/// Window for measuring main-knob speed (acceleration).
const ACCEL_WINDOW: Duration = Duration::from_millis(100);

fn enc_index(e: Encoder) -> usize {
    match e {
        Encoder::Main => 0,
        Encoder::E1 => 1,
        Encoder::E2 => 2,
    }
}

fn push_of(e: Encoder) -> Button {
    match e {
        Encoder::Main => Button::MainPush,
        Encoder::E1 => Button::E1Push,
        Encoder::E2 => Button::E2Push,
    }
}

fn encoder_of(b: Button) -> Option<Encoder> {
    match b {
        Button::MainPush => Some(Encoder::Main),
        Button::E1Push => Some(Encoder::E1),
        Button::E2Push => Some(Encoder::E2),
        _ => None,
    }
}

/// What the renderer needs from the engine besides the radio state.
#[derive(Debug, Clone, PartialEq)]
pub struct View {
    pub trx: usize,
    pub active_vfo: usize,
    pub step_hz: u64,
    pub e1: KnobFunction,
    pub e2: KnobFunction,
    /// Short text to show on the main display instead of the frequency.
    pub flash: Option<String>,
}

/// What a button keys for as long as it is held.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Held {
    /// Push-to-talk (`ptt`).
    Mox,
    /// Tune carrier (`tune_hold`).
    Tune,
}

impl Held {
    fn of(a: &Action) -> Option<Held> {
        match a {
            Action::Ptt => Some(Held::Mox),
            Action::TuneHold => Some(Held::Tune),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Press {
    at: Instant,
    /// Tap / hold already decided (no further action on release).
    done: bool,
    /// Transmitting because of this press: unkey on release.
    held: Option<Held>,
    /// Next auto-repeat of the hold action while still held.
    repeat_at: Option<Instant>,
}

pub struct Engine {
    cfg: Config,
    step_idx: usize,
    active_vfo: usize,
    e_idx: [usize; 2],
    e_accum: [i32; 2],
    pressed: HashMap<Button, Press>,
    /// When each encoder (Main, E1, E2) last turned, for the push lockout.
    last_turn: [Option<Instant>; 3],
    band_mem: Vec<Option<u64>>,
    recent: VecDeque<(Instant, i32)>,
    flash: Option<(String, Instant)>,
    /// `fast`: main knob step times `tuning.fast_multiplier`.
    fast: bool,
    /// `knob_rx2`: control the other receiver instead of `radio.trx`.
    other_rx: bool,
    /// The last `RadioState::alert_seq` shown.
    alert_seen: u64,
}

impl Engine {
    pub fn new(cfg: Config) -> Engine {
        let e_idx = [cfg.e1.start_index(), cfg.e2.start_index()];
        let step_idx = cfg
            .tuning
            .steps_hz
            .iter()
            .position(|s| *s == cfg.tuning.step_hz)
            .unwrap_or(0);
        Engine {
            cfg,
            step_idx,
            active_vfo: 0,
            e_idx,
            e_accum: [0, 0],
            pressed: HashMap::new(),
            last_turn: [None; 3],
            band_mem: vec![None; BANDS.len()],
            recent: VecDeque::new(),
            flash: None,
            fast: false,
            other_rx: false,
            alert_seen: 0,
        }
    }

    pub fn config(&self) -> &Config {
        &self.cfg
    }

    /// Apply new settings while running, keeping the current step where
    /// possible, the active VFO and the knob functions' positions.
    pub fn set_config(&mut self, cfg: Config) {
        // Stay on the current knob functions if they are still in the lists.
        let current = [self.function(0), self.function(1)];
        for (k, list) in [&cfg.e1, &cfg.e2].into_iter().enumerate() {
            self.e_idx[k] = list
                .functions
                .iter()
                .position(|f| *f == current[k])
                .unwrap_or_else(|| list.start_index());
        }
        let step = self.step_hz();
        self.step_idx = cfg
            .tuning
            .steps_hz
            .iter()
            .position(|s| *s == step)
            .or_else(|| cfg.tuning.steps_hz.iter().position(|s| *s == cfg.tuning.step_hz))
            .unwrap_or(0);
        self.cfg = cfg;
    }

    /// The receiver being controlled: `radio.trx`, or the other one of the
    /// first two after `knob_rx2`.
    fn trx(&self) -> usize {
        let base = self.cfg.radio.trx;
        match (self.other_rx, base) {
            (false, _) => base,
            (true, 0) => 1,
            (true, _) => 0,
        }
    }

    fn step_hz(&self) -> u64 {
        self.cfg.tuning.steps_hz[self.step_idx]
    }

    fn function(&self, knob: usize) -> KnobFunction {
        let list = if knob == 0 {
            &self.cfg.e1.functions
        } else {
            &self.cfg.e2.functions
        };
        list[self.e_idx[knob] % list.len()]
    }

    /// Move knob `k` on to its next function the radio can do. Leaves it
    /// where it is if none of them can be done.
    fn next_function(&mut self, k: usize, state: &RadioState) {
        let len = if k == 0 {
            self.cfg.e1.functions.len()
        } else {
            self.cfg.e2.functions.len()
        };
        for _ in 0..len {
            self.e_idx[k] = self.e_idx[k].wrapping_add(1);
            if available(self.function(k), state) {
                return;
            }
        }
    }

    pub fn view(&self, now: Instant) -> View {
        let flash_for = Duration::from_millis(self.cfg.display.flash_ms);
        View {
            trx: self.trx(),
            active_vfo: self.active_vfo,
            step_hz: self.step_hz(),
            e1: self.function(0),
            e2: self.function(1),
            flash: self
                .flash
                .as_ref()
                .filter(|(_, at)| now.duration_since(*at) < flash_for)
                .map(|(t, _)| t.clone()),
        }
    }

    /// Handle one input event. Commands are also applied to `state` at once.
    pub fn handle(&mut self, ev: InputEvent, now: Instant, state: &mut RadioState) -> Vec<Command> {
        let mut out = Vec::new();
        // Touching the controller is the user coming back after the server
        // closed us for being idle (TCI-X §4.4): the client may reconnect.
        state.idle_closed = false;
        match ev {
            InputEvent::Turned { encoder, delta } => {
                self.last_turn[enc_index(encoder)] = Some(now);
                // Turning a knob cancels a push of that same knob in progress.
                if let Some(p) = self.pressed.get_mut(&push_of(encoder)) {
                    p.done = true;
                }
                match encoder {
                    Encoder::Main => self.tune(delta, now, state, &mut out),
                    Encoder::E1 => self.knob(0, delta, now, state, &mut out),
                    Encoder::E2 => self.knob(1, delta, now, state, &mut out),
                }
            }
            InputEvent::Pressed(b) => {
                // A knob push right after turning that knob is almost always
                // accidental: ignore it.
                let lockout = Duration::from_millis(self.cfg.buttons.push_lockout_ms);
                let accidental = encoder_of(b)
                    .and_then(|e| self.last_turn[enc_index(e)])
                    .is_some_and(|t| now.duration_since(t) < lockout);
                let act = self.cfg.buttons.get(b).clone();
                let held = act.tap.as_ref().and_then(Held::of).filter(|_| !accidental);
                let immediate = accidental || held.is_some() || act.hold.is_none();
                if let Some(h) = held {
                    self.key(h, true, now, state, &mut out);
                } else if immediate && !accidental {
                    if let Some(a) = &act.tap {
                        self.action(a, now, state, &mut out);
                    }
                }
                self.pressed.insert(
                    b,
                    Press {
                        at: now,
                        done: immediate,
                        held,
                        repeat_at: None,
                    },
                );
            }
            InputEvent::Released(b) => {
                if let Some(p) = self.pressed.remove(&b) {
                    if let Some(h) = p.held {
                        self.key(h, false, now, state, &mut out);
                    } else if !p.done {
                        if let Some(a) = self.cfg.buttons.get(b).tap.clone() {
                            self.action(&a, now, state, &mut out);
                        }
                    }
                }
            }
        }
        out
    }

    /// Call regularly (e.g. every 10-20 ms) to detect held buttons and
    /// auto-repeat hold actions.
    pub fn tick(&mut self, now: Instant, state: &mut RadioState) -> Vec<Command> {
        let mut out = Vec::new();
        // A knob left on a function the radio can't do (e.g. after TCI-X
        // negotiation) moves on to one it can.
        for k in 0..2 {
            if !available(self.function(k), state) {
                self.next_function(k, state);
            }
        }
        // Show each new alert (a refusal, a control request) once.
        if state.alert_seq != self.alert_seen {
            self.alert_seen = state.alert_seq;
            if let Some(text) = &state.alert {
                self.flash = Some((text.clone(), now));
            }
        }
        // Follow the radio's own active VFO where it reports one.
        if state.tcix_offers("vfo_select") {
            if let Some(ch) = state.trx(self.trx()).and_then(|x| x.vfo_selected) {
                self.active_vfo = ch.min(1);
            }
        }
        let hold = Duration::from_millis(self.cfg.buttons.hold_ms);
        let repeat = Duration::from_millis(self.cfg.buttons.hold_repeat_ms);
        let due: Vec<Button> = self
            .pressed
            .iter()
            .filter(|(_, p)| {
                (!p.done && now.duration_since(p.at) >= hold) || p.repeat_at.is_some_and(|r| now >= r)
            })
            .map(|(b, _)| *b)
            .collect();
        for b in due {
            let hold_action = self.cfg.buttons.get(b).hold.clone();
            let p = self.pressed.get_mut(&b).expect("due buttons are pressed");
            let first = !p.done;
            p.done = true;
            let Some(a) = hold_action else { continue };
            if let Some(h) = Held::of(&a) {
                if first {
                    p.held = Some(h);
                    self.key(h, true, now, state, &mut out);
                }
                continue;
            }
            p.repeat_at = (a.repeats() && !repeat.is_zero()).then(|| now + repeat);
            self.action(&a, now, state, &mut out);
        }
        out
    }

    /// Unkey anything keyed by a held button and forget held buttons
    /// (device lost, quitting).
    pub fn release_all(&mut self, now: Instant, state: &mut RadioState) -> Vec<Command> {
        let mut out = Vec::new();
        let held: Vec<Held> = self.pressed.drain().filter_map(|(_, p)| p.held).collect();
        for h in [Held::Mox, Held::Tune] {
            if held.contains(&h) {
                self.key(h, false, now, state, &mut out);
            }
        }
        out
    }

    fn key(&self, h: Held, on: bool, now: Instant, state: &mut RadioState, out: &mut Vec<Command>) {
        if state.connected || !on {
            let trx = self.trx();
            let cmd = match h {
                Held::Mox => Command::Mox { trx, on },
                Held::Tune => Command::Tune { trx, on },
            };
            self.emit(cmd, now, state, out);
        }
    }

    fn emit(&self, cmd: Command, now: Instant, state: &mut RadioState, out: &mut Vec<Command>) {
        // With TCI-X, send only what the server's manifest takes.
        if !state.allows(&cmd) {
            return;
        }
        state.apply(&cmd);
        if let Command::Vfo { trx, .. } = cmd {
            state.trx_mut(trx).vfo_guard_until = Some(now + VFO_GUARD);
        }
        if let (true, Some(target)) = (cmd.knob_value(), cmd.target()) {
            state.guard(target, now + KNOB_GUARD);
        }
        out.push(cmd);
    }

    fn tune(&mut self, delta: i32, now: Instant, state: &mut RadioState, out: &mut Vec<Command>) {
        let t = self.trx();
        if !state.connected || state.dial_lock || state.trx(t).is_none_or(|x| x.lock) {
            return;
        }
        // Acceleration from the number of counts in the last ACCEL_WINDOW.
        self.recent.push_back((now, delta.abs()));
        while self
            .recent
            .front()
            .is_some_and(|(at, _)| now.duration_since(*at) > ACCEL_WINDOW)
        {
            self.recent.pop_front();
        }
        let rate: i32 = self.recent.iter().map(|(_, n)| n).sum();
        // Setting 0..10; 5 (the default) is coefficient 4.
        let accel = self.cfg.tuning.acceleration.clamp(0.0, 10.0) * 0.8;
        let mult = (1.0 + accel * (rate as f32 / 8.0).powi(2)).min(500.0) as i64;

        let fast = if self.fast {
            self.cfg.tuning.fast_multiplier.max(1)
        } else {
            1
        };
        let step = (self.step_hz() * fast) as i64;
        let cur = state.trx(t).map_or(0, |x| x.vfo[self.active_vfo]) as i64;
        let target = cur + delta as i64 * step * mult;
        let snapped = ((target as f64 / step as f64).round() as i64) * step;
        let (lo, hi) = state.dial_limits();
        let hz = (snapped.max(0) as u64).clamp(lo, hi);
        if hz != cur as u64 {
            self.emit(
                Command::Vfo {
                    trx: t,
                    vfo: self.active_vfo,
                    hz,
                },
                now,
                state,
                out,
            );
        }
    }

    fn knob(&mut self, k: usize, delta: i32, now: Instant, state: &mut RadioState, out: &mut Vec<Command>) {
        let cpd = self.cfg.tuning.counts_per_detent;
        self.e_accum[k] += delta;
        let d = self.e_accum[k] / cpd;
        self.e_accum[k] -= d * cpd;
        if d == 0 || !state.connected {
            return;
        }
        let t = self.trx();
        let f = self.function(k);
        let x = state.trx_mut(t).clone();
        let fs = self.cfg.tuning.filter_step_hz * d;
        let rs = self.cfg.tuning.rit_step_hz * d;
        let cmd = match f {
            KnobFunction::Vol if radio_af_gain(state) => Some(Command::AfGain {
                trx: t,
                pct: (x.af_gain.unwrap_or(50) + d).clamp(0, 100),
            }),
            KnobFunction::Sql if state.tcix_offers("squelch") => Some(Command::SquelchKnob {
                trx: t,
                pct: (x.squelch.unwrap_or(0) + d).clamp(0, 100),
            }),
            KnobFunction::Rfg if state.tcix_offers("rf_gain") => Some(Command::RfGain {
                trx: t,
                pct: (x.rf_gain.unwrap_or(100) + d).clamp(0, 100),
            }),
            // A radio with named filter presets only: step through them, in
            // the server's order.
            KnobFunction::Width if uses_presets(state) => {
                let list = presets(state);
                let pos = x
                    .filter_preset
                    .as_ref()
                    .and_then(|p| list.iter().position(|n| n.eq_ignore_ascii_case(p)))
                    .unwrap_or(0) as i32;
                let next = (pos + d).clamp(0, list.len() as i32 - 1);
                (next != pos || x.filter_preset.is_none()).then(|| Command::FilterPreset {
                    trx: t,
                    name: list[next as usize].clone(),
                })
            }
            KnobFunction::Vol => {
                let cur = state.volume_db.unwrap_or(-30.0);
                let db = match self.cfg.display.volume_scale {
                    VolumeScale::Percent => volume_db((volume_pct(cur) + d).clamp(0, 100)),
                    VolumeScale::Db => (cur.round() + d as f32).clamp(-60.0, 0.0),
                };
                Some(Command::Volume { db })
            }
            KnobFunction::Sql => Some(Command::SquelchLevel {
                trx: t,
                db: (x.sql_level.unwrap_or(-100) + d).clamp(-140, 0),
            }),
            KnobFunction::Rfg => Some(Command::AgcGain {
                trx: t,
                value: (x.agc_gain.unwrap_or(80) + d).clamp(-20, 120),
            }),
            KnobFunction::Drv => Some(Command::Drive {
                trx: t,
                pct: (x.drive.unwrap_or(0) as i32 + d).clamp(0, 100) as u8,
            }),
            KnobFunction::Nr => Some(Command::Nr { trx: t, on: d > 0 }),
            KnobFunction::Nb => Some(Command::Nb { trx: t, on: d > 0 }),
            KnobFunction::An => Some(Command::Anf { trx: t, on: d > 0 }),
            KnobFunction::Width | KnobFunction::High | KnobFunction::Low | KnobFunction::Shift => {
                filter_change(f, fs, t, &x)
            }
            KnobFunction::Rit => Some(Command::RitOffset {
                trx: t,
                hz: (x.rit_offset + rs).clamp(-9990, 9990),
            }),
            KnobFunction::Xit => Some(Command::XitOffset {
                trx: t,
                hz: (x.xit_offset + rs).clamp(-9990, 9990),
            }),
            KnobFunction::MonVol => {
                let cur = state.mon_volume_db.unwrap_or(-30.0);
                let db = match self.cfg.display.volume_scale {
                    VolumeScale::Percent => volume_db((volume_pct(cur) + d).clamp(0, 100)),
                    VolumeScale::Db => (cur.round() + d as f32).clamp(-60.0, 0.0),
                };
                Some(Command::MonVolume { db })
            }
            KnobFunction::CwSpeed => Some(Command::CwSpeed(
                (state.cw_speed.unwrap_or(20) as i32 + d).clamp(1, 60) as u32,
            )),
            KnobFunction::TuneDrive => Some(Command::TuneDrive {
                trx: t,
                pct: (x.tune_drive.unwrap_or(0) as i32 + d).clamp(0, 100) as u8,
            }),
            KnobFunction::RxVol => {
                let cur = rx_volume_pct(x.rx_volume_db.unwrap_or(-6.0));
                Some(Command::RxVolume {
                    trx: t,
                    db: rx_volume_db((cur + d).clamp(0, 100)),
                })
            }
        };
        // At the end of a knob's travel there is nothing new to send.
        if let Some(c) = cmd.filter(|c| !state.holds(c)) {
            self.emit(c, now, state, out);
        }
        self.flash_value(f, now, state);
    }

    /// Show "LABEL  value" on the main display for a moment.
    fn flash_text(&mut self, label: &str, value: &str, now: Instant) {
        self.flash = Some((format!("{label:<4}{value:>5}"), now));
    }

    fn flash_value(&mut self, f: KnobFunction, now: Instant, state: &RadioState) {
        let text = value_text(f, state, self.trx(), self.cfg.display.volume_scale);
        self.flash = Some((text, now));
    }

    fn action(&mut self, a: &Action, now: Instant, state: &mut RadioState, out: &mut Vec<Command>) {
        let t = self.trx();
        if !state.connected {
            // Only local UI actions make sense without a radio.
            if !matches!(
                a,
                Action::StepNext
                    | Action::StepPrev
                    | Action::E1Next
                    | Action::E2Next
                    | Action::Fast
                    | Action::KnobRx2
                    | Action::BacklightNext
            ) {
                return;
            }
        }
        let x = state.trx_mut(t).clone();
        let av = self.active_vfo;
        let cmds: Vec<Command> = match a {
            // The radio's own band key and band stack, where it offers one.
            Action::BandUp | Action::BandDown if state.tcix_offers("band") => {
                let list = state
                    .tcix
                    .as_ref()
                    .and_then(|m| m.enum_values("band"))
                    .unwrap_or_default();
                let dir = if *a == Action::BandUp { 1 } else { -1 };
                step_in_list(list, x.vfo[av], dir)
                    .map(|i| Command::Band {
                        trx: t,
                        name: list[i].clone(),
                    })
                    .into_iter()
                    .collect()
            }
            Action::BandUp | Action::BandDown => {
                let cur = x.vfo[av];
                if let Some(b) = band_of(cur) {
                    self.band_mem[b] = Some(cur);
                }
                let i = step_band(cur, if *a == Action::BandUp { 1 } else { -1 });
                let hz = self.band_mem[i].unwrap_or(BANDS[i].default_hz);
                vec![Command::Vfo { trx: t, vfo: av, hz }]
            }
            Action::ModeNext | Action::ModePrev => {
                let mut cycle: Vec<Mode> = self
                    .cfg
                    .tuning
                    .mode_cycle
                    .iter()
                    .map(|m| Mode::from_name(m))
                    .collect();
                cycle.retain(|m| state.offers_mode(*m));
                if cycle.is_empty() {
                    vec![]
                } else {
                    let n = cycle.len() as i32;
                    let forward = *a == Action::ModeNext;
                    // Plain "CW" can match both a CWL and a CWU entry: step on
                    // from the last match going forward and the first going
                    // back, so the cycle never stalls between the two.
                    let pos = x.mode.and_then(|m| {
                        let mut hits = cycle
                            .iter()
                            .enumerate()
                            .filter(|(_, c)| c.matches(m))
                            .map(|(i, _)| i);
                        if forward {
                            hits.next_back()
                        } else {
                            hits.next()
                        }
                    });
                    let next = match (pos, forward) {
                        (Some(p), true) => (p as i32 + 1).rem_euclid(n),
                        (Some(p), false) => (p as i32 - 1).rem_euclid(n),
                        (None, _) => 0,
                    };
                    vec![Command::Mode {
                        trx: t,
                        mode: cycle[next as usize],
                    }]
                }
            }
            Action::Mode(name) => vec![Command::Mode {
                trx: t,
                mode: Mode::from_name(name),
            }],
            Action::VfoToggle if state.tcix_offers("vfo_select") => {
                self.active_vfo ^= 1;
                vec![Command::VfoSelect {
                    trx: t,
                    ch: self.active_vfo,
                }]
            }
            Action::VfoToggle => {
                self.active_vfo ^= 1;
                vec![]
            }
            // The next of the radio's own settings, round again.
            Action::Preamp | Action::Att => {
                let (cap, cur) = match a {
                    Action::Preamp => ("preamp", &x.preamp),
                    _ => ("attenuator", &x.attenuator),
                };
                let list = state
                    .tcix
                    .as_ref()
                    .filter(|m| m.writable(cap))
                    .and_then(|m| m.enum_values(cap))
                    .unwrap_or_default();
                let at = cur
                    .as_ref()
                    .and_then(|c| list.iter().position(|v| v.eq_ignore_ascii_case(c)));
                let next = at.map_or(0, |i| (i + 1) % list.len().max(1));
                list.get(next)
                    .map(|value| match a {
                        Action::Preamp => Command::Preamp {
                            trx: t,
                            value: value.clone(),
                        },
                        _ => Command::Attenuator {
                            trx: t,
                            value: value.clone(),
                        },
                    })
                    .into_iter()
                    .collect()
            }
            Action::Control if state.in_control() => vec![Command::ControlRelease],
            Action::Control => vec![Command::ControlRequest],
            Action::VfoAToB => vec![Command::Vfo {
                trx: t,
                vfo: 1,
                hz: x.vfo[0],
            }],
            Action::VfoBToA => vec![Command::Vfo {
                trx: t,
                vfo: 0,
                hz: x.vfo[1],
            }],
            Action::Fast => {
                self.fast = !self.fast;
                self.flash_text("FASt", on_off(self.fast), now);
                vec![]
            }
            Action::Rx2 => {
                let on = !state.trx(1).is_some_and(|r| r.rx_enable);
                self.flash_text("rX2", on_off(on), now);
                vec![Command::RxEnable { trx: 1, on }]
            }
            Action::SubRx => {
                self.flash_text("Sub", on_off(!x.sub_rx), now);
                vec![Command::SubRx {
                    trx: t,
                    on: !x.sub_rx,
                }]
            }
            Action::KnobRx2 => {
                self.other_rx = !self.other_rx;
                self.active_vfo = 0;
                let n = self.trx() + 1;
                self.flash_text("rX", &n.to_string(), now);
                vec![]
            }
            Action::FilterDefault => {
                let cmds: Vec<Command> = x
                    .mode
                    .and_then(|m| self.cfg.default_filter(m.name()))
                    .map(|(low, high)| Command::Filter { trx: t, low, high })
                    .into_iter()
                    .collect();
                self.emit_all(cmds, now, state, out);
                self.flash_value(KnobFunction::Width, now, state);
                vec![]
            }
            Action::FilterWider | Action::FilterNarrower => {
                let fs = self.cfg.tuning.filter_step_hz;
                let fs = if *a == Action::FilterWider { fs } else { -fs };
                let cmds: Vec<Command> = filter_change(KnobFunction::Width, fs, t, &x)
                    .into_iter()
                    .collect();
                self.emit_all(cmds, now, state, out);
                self.flash_value(KnobFunction::Width, now, state);
                vec![]
            }
            Action::VfoSwap => vec![
                Command::Vfo {
                    trx: t,
                    vfo: 0,
                    hz: x.vfo[1],
                },
                Command::Vfo {
                    trx: t,
                    vfo: 1,
                    hz: x.vfo[0],
                },
            ],
            Action::Split => vec![Command::Split { trx: t, on: !x.split }],
            Action::Rit => vec![Command::Rit { trx: t, on: !x.rit }],
            Action::Xit => vec![Command::Xit { trx: t, on: !x.xit }],
            Action::RitClear => vec![Command::RitOffset { trx: t, hz: 0 }],
            Action::XitClear => vec![Command::XitOffset { trx: t, hz: 0 }],
            Action::RitXitClear => vec![
                Command::RitOffset { trx: t, hz: 0 },
                Command::XitOffset { trx: t, hz: 0 },
            ],
            Action::Lock if state.tcix_offers("dial_lock") => vec![Command::DialLock(!state.dial_lock)],
            Action::Lock => vec![Command::Lock { trx: t, on: !x.lock }],
            Action::Mute => vec![Command::Mute(!state.mute)],
            Action::Rx1Mute | Action::Rx2Mute => {
                let r = if *a == Action::Rx1Mute { 0 } else { 1 };
                let on = !state.trx(r).is_some_and(|x| x.rx_mute);
                vec![Command::RxMute { trx: r, on }]
            }
            Action::Nr => vec![Command::Nr { trx: t, on: !x.nr }],
            Action::Nb => vec![Command::Nb { trx: t, on: !x.nb }],
            Action::Anf => vec![Command::Anf { trx: t, on: !x.anf }],
            // TCI-X's notch carries its frequency: keep the one the radio has.
            Action::Notch if state.tcix_offers("notch") => x
                .notch_hz
                .map(|hz| Command::NotchAt {
                    trx: t,
                    on: !x.notch,
                    hz,
                })
                .into_iter()
                .collect(),
            Action::Notch => vec![Command::Notch { trx: t, on: !x.notch }],
            Action::Squelch => {
                self.emit(
                    Command::Squelch {
                        trx: t,
                        enable: !x.sql_enable,
                    },
                    now,
                    state,
                    out,
                );
                self.flash_value(KnobFunction::Sql, now, state);
                vec![]
            }
            Action::AgcNext | Action::Agc(_) => {
                let mode = match a {
                    Action::Agc(m) => m.clone(),
                    _ => {
                        let pos = x
                            .agc_mode
                            .as_deref()
                            .and_then(|m| AGC_CYCLE.iter().position(|c| *c == m));
                        let next = pos.map_or(0, |p| (p + 1) % AGC_CYCLE.len());
                        AGC_CYCLE[next].to_string()
                    }
                };
                self.flash_text("AGC", agc_text(&mode), now);
                vec![Command::AgcMode { trx: t, mode }]
            }
            Action::Apf => {
                self.flash_text("APF", on_off(!x.apf), now);
                vec![Command::Apf { trx: t, on: !x.apf }]
            }
            Action::Binaural => {
                self.flash_text("bin", on_off(!x.binaural), now);
                vec![Command::Binaural {
                    trx: t,
                    on: !x.binaural,
                }]
            }
            Action::Monitor => {
                self.flash_text("nnon", on_off(!state.monitor), now);
                vec![Command::Monitor(!state.monitor)]
            }
            // No carrier tune, but an antenna tuner: start a tune cycle, or
            // stop transmitting if one is under way.
            Action::Tune if !state.can_send("tune") && state.tcix_offers("atu_tune") => {
                if x.tx {
                    vec![Command::Mox { trx: t, on: false }]
                } else {
                    vec![Command::AtuTune { trx: t }]
                }
            }
            Action::Tune => vec![Command::Tune { trx: t, on: !x.tune }],
            Action::Mox => vec![Command::Mox { trx: t, on: !x.tx }],
            // Unknown power state counts as running, so the first press stops.
            Action::Power => vec![Command::Power(!state.power.unwrap_or(true))],
            Action::Focus => vec![Command::Focus],
            Action::BacklightNext => {
                let cur = self.cfg.display.backlight;
                let i = BACKLIGHT_PRESETS.iter().position(|(_, c)| *c == cur);
                let next = i.map_or(0, |i| (i + 1) % BACKLIGHT_PRESETS.len());
                self.cfg.display.backlight = BACKLIGHT_PRESETS[next].1;
                vec![]
            }
            Action::StepNext | Action::StepPrev => {
                let n = self.cfg.tuning.steps_hz.len() as i32;
                let dir = if *a == Action::StepNext { 1 } else { -1 };
                self.step_idx = (self.step_idx as i32 + dir).rem_euclid(n) as usize;
                vec![]
            }
            Action::E1Next | Action::E2Next => {
                let k = if *a == Action::E1Next { 0 } else { 1 };
                self.next_function(k, state);
                self.e_accum[k] = 0;
                if state.connected {
                    self.flash_value(self.function(k), now, state);
                }
                vec![]
            }
            Action::E1Toggle | Action::E2Toggle => {
                let k = if *a == Action::E1Toggle { 0 } else { 1 };
                let f = self.function(k);
                let cmds = match f {
                    KnobFunction::Vol => vec![Command::Mute(!state.mute)],
                    KnobFunction::Sql => vec![Command::Squelch {
                        trx: t,
                        enable: !x.sql_enable,
                    }],
                    KnobFunction::Nr => vec![Command::Nr { trx: t, on: !x.nr }],
                    KnobFunction::Nb => vec![Command::Nb { trx: t, on: !x.nb }],
                    KnobFunction::An => vec![Command::Anf { trx: t, on: !x.anf }],
                    KnobFunction::Rit => vec![Command::Rit { trx: t, on: !x.rit }],
                    KnobFunction::Xit => vec![Command::Xit { trx: t, on: !x.xit }],
                    // Filter functions have nothing to toggle: restore the
                    // mode's default filter instead.
                    KnobFunction::Width | KnobFunction::Shift | KnobFunction::High | KnobFunction::Low => x
                        .mode
                        .and_then(|m| self.cfg.default_filter(m.name()))
                        .map(|(low, high)| vec![Command::Filter { trx: t, low, high }])
                        .unwrap_or_default(),
                    _ => vec![],
                };
                for c in cmds {
                    self.emit(c, now, state, out);
                }
                self.flash_value(f, now, state);
                vec![]
            }
            Action::Raw(text) => vec![Command::Raw(text.clone())],
            // Push-to-talk and tune_hold are handled on press / release.
            Action::Ptt | Action::TuneHold => vec![],
            Action::None => vec![],
        };
        self.emit_all(cmds, now, state, out);
    }

    fn emit_all(&self, cmds: Vec<Command>, now: Instant, state: &mut RadioState, out: &mut Vec<Command>) {
        for c in cmds {
            self.emit(c, now, state, out);
        }
    }
}

fn on_off(b: bool) -> &'static str {
    if b {
        "on"
    } else {
        "off"
    }
}

/// AGC mode as seven segments can show it ("m" can't be drawn).
fn agc_text(mode: &str) -> &'static str {
    match mode {
        "off" => "off",
        "long" => "Long",
        "slow" => "SLo",
        "normal" => "nor",
        "fast" => "FASt",
        "custom" => "CUSt",
        _ => "--",
    }
}

/// New filter after moving it by `fs` Hz as filter function `f` does, or
/// `None` without a filter. Never narrower than 50 Hz.
fn filter_change(f: KnobFunction, fs: i32, trx: usize, x: &crate::state::Trx) -> Option<Command> {
    let lsb = x.mode.is_some_and(Mode::is_lower_sideband);
    x.filter.map(|(lo, hi)| {
        let (mut lo, mut hi) = (lo, hi);
        match (f, lsb) {
            // Each edge moves half a step, so the width changes by one
            // filter step per click about a fixed centre.
            (KnobFunction::Width, _) => (lo, hi) = (lo - fs / 2, hi + fs / 2),
            (KnobFunction::High, false) => hi += fs,
            (KnobFunction::High, true) => lo -= fs,
            (KnobFunction::Low, false) => lo += fs,
            (KnobFunction::Low, true) => hi -= fs,
            (_, false) => (lo, hi) = (lo + fs, hi + fs),
            (_, true) => (lo, hi) = (lo - fs, hi - fs),
        }
        if hi - lo < 50 {
            (lo, hi) = x.filter.unwrap();
        }
        Command::Filter {
            trx,
            low: lo,
            high: hi,
        }
    })
}

/// A receiver's own volume (Thetis RX1 / RX2 AF) as the 0-100 Thetis shows:
/// amplitude in percent, so -6 dB is 50.
pub fn rx_volume_pct(db: f32) -> i32 {
    (10f32.powf(db.clamp(-60.0, 0.0) / 20.0) * 100.0).round() as i32
}

/// Inverse of [`rx_volume_pct`], aimed a little above the step because
/// Thetis truncates when converting back (see [`volume_db`]).
pub fn rx_volume_db(pct: i32) -> f32 {
    match pct.clamp(0, 100) {
        0 => -60.0,
        p => (20.0 * ((p as f32 + 0.05) / 100.0).log10()).min(0.0),
    }
}

/// Whether the radio can do what a knob function changes. Without TCI-X
/// every function is offered, as before; with it, only those the manifest
/// lets us change.
pub fn available(f: KnobFunction, s: &RadioState) -> bool {
    match f {
        KnobFunction::Vol => s.can_send("volume") || s.tcix_offers("af_gain"),
        KnobFunction::Sql => s.can_send("sql_level") || s.tcix_offers("squelch"),
        KnobFunction::Rfg => s.can_send("agc_gain") || s.tcix_offers("rf_gain"),
        KnobFunction::Drv => s.can_send("drive"),
        KnobFunction::Nr => s.can_send("rx_nr_enable"),
        KnobFunction::Nb => s.can_send("rx_nb_enable"),
        KnobFunction::An => s.can_send("rx_anf_enable"),
        KnobFunction::Width => s.can_send("rx_filter_band") || uses_presets(s),
        KnobFunction::High | KnobFunction::Low | KnobFunction::Shift => s.can_send("rx_filter_band"),
        KnobFunction::Rit => s.can_send("rit_offset"),
        KnobFunction::Xit => s.can_send("xit_offset"),
        KnobFunction::RxVol => s.can_send("rx_volume"),
        KnobFunction::MonVol => s.can_send("mon_volume"),
        KnobFunction::CwSpeed => s.can_send("cw_keyer_speed"),
        KnobFunction::TuneDrive => s.can_send("tune_drive"),
    }
}

/// VOL is the radio's own AF gain (TCI-X `af_gain`): where the server
/// offers that and no `volume`. A server with a `volume` of its own is the
/// program the operator listens through (KADRAD's relay), and VOL is that.
fn radio_af_gain(s: &RadioState) -> bool {
    s.tcix_offers("af_gain") && !s.tcix_offers("volume")
}

/// The radio has named filter presets (TCI-X `filter`) and no continuously
/// adjustable passband.
fn uses_presets(s: &RadioState) -> bool {
    !s.can_send("rx_filter_band") && !presets(s).is_empty()
}

fn presets(s: &RadioState) -> &[String] {
    s.tcix
        .as_ref()
        .filter(|m| m.writable("filter"))
        .and_then(|m| m.enum_values("filter"))
        .unwrap_or_default()
}

/// Master volume as the 0-100 Thetis shows: a straight line from -60 dB (0)
/// to 0 dB (100).
pub fn volume_pct(db: f32) -> i32 {
    ((db.clamp(-60.0, 0.0) + 60.0) / 60.0 * 100.0).round() as i32
}

/// Inverse of [`volume_pct`]. Thetis converts dB back to its slider by
/// truncating, so a value exactly on a step (e.g. -58.2 dB for 3) can come out
/// one low through floating-point error. Aim 0.03 dB above the step instead.
pub fn volume_db(pct: i32) -> f32 {
    (pct.clamp(0, 100) as f32 * 0.6 - 60.0 + 0.03).min(0.0)
}

/// Text for the main display when a knob value changes, e.g. "VOL   -12".
pub fn value_text(f: KnobFunction, state: &RadioState, trx: usize, volume: VolumeScale) -> String {
    let x = state.trx(trx).cloned().unwrap_or_default();
    let on_off = |b: bool| if b { "on" } else { "off" }.to_string();
    let lsb = x.mode.is_some_and(Mode::is_lower_sideband);
    let num = |v: Option<i32>| v.map_or("--".to_string(), |v| v.to_string());
    let (label, value) = match f {
        KnobFunction::Vol if radio_af_gain(state) => ("VOL", num(x.af_gain)),
        KnobFunction::Sql if state.tcix_offers("squelch") => ("SQL", num(x.squelch)),
        KnobFunction::Rfg if state.tcix_offers("rf_gain") => ("RFG", num(x.rf_gain)),
        // Presets show as their place in the list, 1 first.
        KnobFunction::Width if uses_presets(state) => (
            "FIL",
            num(x
                .filter_preset
                .as_ref()
                .and_then(|p| presets(state).iter().position(|n| n.eq_ignore_ascii_case(p)))
                .map(|i| i as i32 + 1)),
        ),
        KnobFunction::Vol if state.mute => ("VOL", "off".to_string()),
        KnobFunction::Vol => (
            "VOL",
            num(state.volume_db.map(|db| match volume {
                VolumeScale::Percent => volume_pct(db),
                VolumeScale::Db => db.round() as i32,
            })),
        ),
        KnobFunction::Sql if !x.sql_enable => ("SQL", "off".to_string()),
        KnobFunction::Sql => ("SQL", num(x.sql_level)),
        KnobFunction::Rfg => ("AGC", num(x.agc_gain)),
        KnobFunction::Drv => ("drU", num(x.drive.map(i32::from))),
        KnobFunction::Nr => ("nr", on_off(x.nr)),
        KnobFunction::Nb => ("nb", on_off(x.nb)),
        KnobFunction::An => ("AnF", on_off(x.anf)),
        KnobFunction::High => ("HI", num(x.filter.map(|(lo, hi)| if lsb { -lo } else { hi }))),
        KnobFunction::Low => ("LO", num(x.filter.map(|(lo, hi)| if lsb { -hi } else { lo }))),
        KnobFunction::Width => ("FIL", num(x.filter.map(|(lo, hi)| hi - lo))),
        KnobFunction::Shift => ("SH", num(x.filter.map(|(lo, hi)| ((lo + hi) / 2).abs()))),
        KnobFunction::Rit if !x.rit => ("rit", "off".to_string()),
        KnobFunction::Rit => ("rit", num(Some(x.rit_offset))),
        KnobFunction::Xit if !x.xit => ("Hit", "off".to_string()),
        KnobFunction::Xit => ("Hit", num(Some(x.xit_offset))),
        KnobFunction::MonVol => (
            "nnon",
            num(state.mon_volume_db.map(|db| match volume {
                VolumeScale::Percent => volume_pct(db),
                VolumeScale::Db => db.round() as i32,
            })),
        ),
        KnobFunction::CwSpeed => ("SPd", num(state.cw_speed.map(|w| w as i32))),
        KnobFunction::TuneDrive => ("tdr", num(x.tune_drive.map(i32::from))),
        KnobFunction::RxVol => ("AF", num(x.rx_volume_db.map(rx_volume_pct))),
    };
    format!("{label:<4}{value:>5}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn connected() -> RadioState {
        let mut s = RadioState {
            connected: true,
            ..Default::default()
        };
        let t = s.trx_mut(0);
        t.vfo = [14_074_000, 7_074_000];
        t.mode = Some(Mode::Usb);
        t.filter = Some((100, 2800));
        s.volume_db = Some(-20.0);
        s
    }

    fn turn(encoder: Encoder, delta: i32) -> InputEvent {
        InputEvent::Turned { encoder, delta }
    }

    #[test]
    fn main_knob_tunes_active_vfo_by_step() {
        let mut e = Engine::new(Config::default());
        let mut s = connected();
        let now = Instant::now();
        let cmds = e.handle(turn(Encoder::Main, 1), now, &mut s);
        assert_eq!(
            cmds,
            vec![Command::Vfo {
                trx: 0,
                vfo: 0,
                hz: 14_074_010
            }]
        );
        assert_eq!(s.trx[0].vfo[0], 14_074_010);
        // Echo of an older frequency during the guard window is ignored.
        s.remote_vfo(0, 0, 14_074_000, now + Duration::from_millis(50));
        assert_eq!(s.trx[0].vfo[0], 14_074_010);
        s.remote_vfo(0, 0, 14_074_000, now + Duration::from_secs(1));
        assert_eq!(s.trx[0].vfo[0], 14_074_000);
    }

    #[test]
    fn volume_in_db_mode_steps_one_db() {
        let mut cfg = Config::default();
        cfg.display.volume_scale = VolumeScale::Db;
        let mut e = Engine::new(cfg);
        let mut s = connected();
        let now = Instant::now();
        e.handle(turn(Encoder::E1, 2), now, &mut s);
        assert_eq!(s.volume_db, Some(-19.0));
        assert_eq!(e.view(now).flash.as_deref(), Some("VOL   -19"));
    }

    #[test]
    fn fast_spinning_accelerates() {
        let mut e = Engine::new(Config::default());
        let mut s = connected();
        let now = Instant::now();
        for i in 0..20 {
            e.handle(turn(Encoder::Main, 1), now + Duration::from_millis(i * 2), &mut s);
        }
        assert!(s.trx[0].vfo[0] > 14_074_000 + 20 * 10, "got {}", s.trx[0].vfo[0]);
    }

    #[test]
    fn e1_needs_two_counts_per_click_and_changes_volume() {
        let mut e = Engine::new(Config::default());
        let mut s = connected();
        let now = Instant::now();
        assert!(e.handle(turn(Encoder::E1, 1), now, &mut s).is_empty());
        assert_eq!(
            e.handle(turn(Encoder::E1, 1), now, &mut s),
            vec![Command::Volume { db: volume_db(68) }]
        );
        assert_eq!(e.view(now).flash.as_deref(), Some("VOL    68"));
        // Thetis's own reading: -53.4 dB shows as 11.
        assert_eq!(volume_pct(-53.4), 11);
        assert_eq!(volume_pct(volume_db(37)), 37);
        // Thetis truncates (dB + 60) / 0.6 of the two-decimal value it receives.
        for pct in 0..=100 {
            let sent: f64 = format!("{:.2}", volume_db(pct)).parse().unwrap();
            assert_eq!(((sent + 60.0) / 0.6).floor() as i32, pct, "step {pct}");
        }
    }

    #[test]
    fn tap_vs_hold() {
        let mut cfg = Config::default();
        cfg.buttons.f3.tap = Some(Action::ModeNext);
        cfg.buttons.f3.hold = Some(Action::ModePrev);
        let mut e = Engine::new(cfg);
        let mut s = connected();
        let t0 = Instant::now();
        // F3: tap = mode_next (USB -> CWU), hold = mode_prev (USB -> LSB).
        e.handle(InputEvent::Pressed(Button::F3), t0, &mut s);
        let c = e.handle(
            InputEvent::Released(Button::F3),
            t0 + Duration::from_millis(100),
            &mut s,
        );
        assert_eq!(
            c,
            vec![Command::Mode {
                trx: 0,
                mode: Mode::CwU
            }]
        );
        e.handle(InputEvent::Pressed(Button::F3), t0, &mut s);
        let c = e.tick(t0 + Duration::from_millis(600), &mut s);
        assert_eq!(
            c,
            vec![Command::Mode {
                trx: 0,
                mode: Mode::Usb
            }]
        );
        let c = e.handle(
            InputEvent::Released(Button::F3),
            t0 + Duration::from_millis(700),
            &mut s,
        );
        assert!(c.is_empty(), "no tap after a hold");
    }

    #[test]
    fn band_up_remembers_frequency() {
        let mut e = Engine::new(Config::default());
        let mut s = connected();
        let now = Instant::now();
        s.trx_mut(0).vfo[0] = 14_200_000;
        e.handle(InputEvent::Pressed(Button::F6), now, &mut s);
        assert_eq!(s.trx[0].vfo[0], 18_100_000);
        e.handle(InputEvent::Pressed(Button::F5), now, &mut s);
        assert_eq!(s.trx[0].vfo[0], 14_200_000);
    }

    #[test]
    fn push_right_after_turning_is_ignored() {
        let mut e = Engine::new(Config::default());
        let mut s = connected();
        let t0 = Instant::now();
        e.handle(turn(Encoder::Main, 1), t0, &mut s);
        let ms = |n| t0 + Duration::from_millis(n);
        e.handle(InputEvent::Pressed(Button::MainPush), ms(100), &mut s);
        e.handle(InputEvent::Released(Button::MainPush), ms(150), &mut s);
        assert_eq!(e.view(ms(150)).active_vfo, 0, "push 100 ms after turning ignored");
        e.handle(InputEvent::Pressed(Button::MainPush), ms(1000), &mut s);
        e.handle(InputEvent::Released(Button::MainPush), ms(1100), &mut s);
        assert_eq!(
            e.view(ms(1100)).active_vfo,
            1,
            "deliberate push switches to VFO B"
        );
    }

    #[test]
    fn turning_while_pushed_cancels_the_push() {
        let mut e = Engine::new(Config::default());
        let mut s = connected();
        let t0 = Instant::now();
        let ms = |n| t0 + Duration::from_millis(n);
        e.handle(InputEvent::Pressed(Button::MainPush), t0, &mut s);
        e.handle(turn(Encoder::Main, 1), ms(50), &mut s);
        e.handle(InputEvent::Released(Button::MainPush), ms(100), &mut s);
        assert_eq!(e.view(ms(100)).active_vfo, 0);
        assert!(e.tick(ms(900), &mut s).is_empty(), "no hold either");
    }

    #[test]
    fn ptt_transmits_only_while_held() {
        let mut cfg = Config::default();
        cfg.buttons.f1.tap = Some(Action::Ptt);
        let mut e = Engine::new(cfg);
        let mut s = connected();
        let t0 = Instant::now();
        let c = e.handle(InputEvent::Pressed(Button::F1), t0, &mut s);
        assert_eq!(c, vec![Command::Mox { trx: 0, on: true }]);
        assert!(e.tick(t0 + Duration::from_secs(2), &mut s).is_empty());
        let c = e.handle(
            InputEvent::Released(Button::F1),
            t0 + Duration::from_secs(3),
            &mut s,
        );
        assert_eq!(c, vec![Command::Mox { trx: 0, on: false }]);
        // Device lost mid-transmission: release_all unkeys.
        e.handle(InputEvent::Pressed(Button::F1), t0, &mut s);
        assert_eq!(
            e.release_all(t0, &mut s),
            vec![Command::Mox { trx: 0, on: false }]
        );
    }

    #[test]
    fn holding_main_push_keeps_cycling_steps() {
        let mut e = Engine::new(Config::default());
        let mut s = connected();
        let t0 = Instant::now();
        let ms = |n| t0 + Duration::from_millis(n);
        assert_eq!(e.view(t0).step_hz, 10);
        e.handle(InputEvent::Pressed(Button::MainPush), t0, &mut s);
        for t in (0..=1200).step_by(10) {
            e.tick(ms(t), &mut s);
        }
        // Hold fires at 500 ms, then repeats at 1000 ms: 2 steps.
        assert_eq!(e.view(ms(1200)).step_hz, 1_000);
        e.handle(InputEvent::Released(Button::MainPush), ms(1210), &mut s);
        assert_eq!(e.view(ms(1210)).active_vfo, 0, "no tap after a hold");
    }

    #[test]
    fn f1_is_mox_and_fires_on_press() {
        let mut e = Engine::new(Config::default());
        let mut s = connected();
        let c = e.handle(InputEvent::Pressed(Button::F1), Instant::now(), &mut s);
        assert_eq!(c, vec![Command::Mox { trx: 0, on: true }]);
    }

    #[test]
    fn knobs_start_on_configured_function_and_keep_it_on_reload() {
        let mut cfg = Config::default();
        assert_eq!(
            Engine::new(cfg.clone()).view(Instant::now()).e2,
            KnobFunction::Width
        );
        cfg.e1.start = Some(KnobFunction::Sql);
        let mut e = Engine::new(cfg.clone());
        assert_eq!(e.view(Instant::now()).e1, KnobFunction::Sql);
        // Reordering the list keeps the knob on SQL.
        cfg.e1.functions.reverse();
        e.set_config(cfg);
        assert_eq!(e.view(Instant::now()).e1, KnobFunction::Sql);
    }

    #[test]
    fn holding_e2_on_a_filter_function_restores_the_default() {
        let mut e = Engine::new(Config::default());
        let mut s = connected();
        s.trx_mut(0).filter = Some((300, 1800));
        let t0 = Instant::now();
        e.handle(InputEvent::Pressed(Button::E2Push), t0, &mut s);
        let c = e.tick(t0 + Duration::from_millis(600), &mut s);
        assert_eq!(
            c,
            vec![Command::Filter {
                trx: 0,
                low: 100,
                high: 2500
            }]
        );
    }

    #[test]
    fn width_changes_both_edges_about_the_centre() {
        let mut cfg = Config::default();
        cfg.e2.functions = vec![KnobFunction::Width];
        let mut e = Engine::new(cfg);
        let mut s = connected();
        let now = Instant::now();
        let c = e.handle(turn(Encoder::E2, 2), now, &mut s);
        assert_eq!(
            c,
            vec![Command::Filter {
                trx: 0,
                low: 75,
                high: 2825
            }]
        );
        assert_eq!(e.view(now).flash.as_deref(), Some("FIL  2750"));
        // Narrowing never goes below 50 Hz.
        s.trx_mut(0).filter = Some((1000, 1060));
        e.handle(turn(Encoder::E2, -2), now, &mut s);
        assert_eq!(s.trx[0].filter, Some((1000, 1060)));
    }

    #[test]
    fn lsb_filter_high_moves_the_negative_edge() {
        let mut cfg = Config::default();
        cfg.e2.functions = vec![KnobFunction::High];
        let mut e = Engine::new(cfg);
        let mut s = connected();
        s.trx_mut(0).mode = Some(Mode::Lsb);
        s.trx_mut(0).filter = Some((-2800, -100));
        let c = e.handle(turn(Encoder::E2, 2), Instant::now(), &mut s);
        assert_eq!(
            c,
            vec![Command::Filter {
                trx: 0,
                low: -2850,
                high: -100
            }]
        );
    }

    /// Tap F4 (configured to `a`, no hold) and return what was sent.
    fn tap(e: &mut Engine, s: &mut RadioState, now: Instant) -> Vec<Command> {
        let mut c = e.handle(InputEvent::Pressed(Button::F4), now, s);
        c.extend(e.handle(InputEvent::Released(Button::F4), now, s));
        c
    }

    fn engine_with_f4(a: Action) -> Engine {
        let mut cfg = Config::default();
        cfg.buttons.f4.tap = Some(a);
        cfg.buttons.f4.hold = None;
        Engine::new(cfg)
    }

    #[test]
    fn fast_multiplies_the_step() {
        let mut e = engine_with_f4(Action::Fast);
        let mut s = connected();
        let now = Instant::now();
        assert!(tap(&mut e, &mut s, now).is_empty(), "fast is local only");
        let c = e.handle(turn(Encoder::Main, 1), now, &mut s);
        // Main knob is not inverted at this level: +1 count = +10 Hz x 10.
        assert_eq!(
            c,
            vec![Command::Vfo {
                trx: 0,
                vfo: 0,
                hz: 14_074_100
            }]
        );
        tap(&mut e, &mut s, now + Duration::from_secs(1));
        let c = e.handle(turn(Encoder::Main, 1), now + Duration::from_secs(2), &mut s);
        assert_eq!(
            c,
            vec![Command::Vfo {
                trx: 0,
                vfo: 0,
                hz: 14_074_110
            }]
        );
    }

    #[test]
    fn tune_hold_tunes_only_while_held() {
        let mut cfg = Config::default();
        cfg.buttons.f2.tap = None;
        cfg.buttons.f2.hold = Some(Action::TuneHold);
        let mut e = Engine::new(cfg);
        let mut s = connected();
        let t0 = Instant::now();
        assert!(e.handle(InputEvent::Pressed(Button::F2), t0, &mut s).is_empty());
        let c = e.tick(t0 + Duration::from_millis(600), &mut s);
        assert_eq!(c, vec![Command::Tune { trx: 0, on: true }]);
        assert!(
            e.tick(t0 + Duration::from_secs(2), &mut s).is_empty(),
            "no repeat"
        );
        let c = e.handle(
            InputEvent::Released(Button::F2),
            t0 + Duration::from_secs(3),
            &mut s,
        );
        assert_eq!(c, vec![Command::Tune { trx: 0, on: false }]);
        // Device lost while tuning: release_all stops the carrier.
        e.handle(InputEvent::Pressed(Button::F2), t0, &mut s);
        e.tick(t0 + Duration::from_millis(600), &mut s);
        assert_eq!(
            e.release_all(t0, &mut s),
            vec![Command::Tune { trx: 0, on: false }]
        );
    }

    #[test]
    fn rit_and_xit_clear_separately_or_together() {
        let now = Instant::now();
        let cases = [
            (Action::RitClear, vec![Command::RitOffset { trx: 0, hz: 0 }]),
            (Action::XitClear, vec![Command::XitOffset { trx: 0, hz: 0 }]),
            (
                Action::RitXitClear,
                vec![
                    Command::RitOffset { trx: 0, hz: 0 },
                    Command::XitOffset { trx: 0, hz: 0 },
                ],
            ),
        ];
        for (a, want) in cases {
            let mut e = engine_with_f4(a);
            let mut s = connected();
            assert_eq!(tap(&mut e, &mut s, now), want);
        }
    }

    #[test]
    fn knob_rx2_moves_control_to_the_other_receiver() {
        let mut e = engine_with_f4(Action::KnobRx2);
        let mut s = connected();
        s.trx_mut(1).vfo = [3_573_000, 0];
        let now = Instant::now();
        tap(&mut e, &mut s, now);
        assert_eq!(e.view(now).trx, 1);
        let c = e.handle(turn(Encoder::Main, 1), now, &mut s);
        assert_eq!(
            c,
            vec![Command::Vfo {
                trx: 1,
                vfo: 0,
                hz: 3_573_010
            }]
        );
        tap(&mut e, &mut s, now);
        assert_eq!(e.view(now).trx, 0);
    }

    #[test]
    fn filter_wider_and_narrower_step_the_width() {
        let now = Instant::now();
        let mut e = engine_with_f4(Action::FilterWider);
        let mut s = connected();
        assert_eq!(
            tap(&mut e, &mut s, now),
            vec![Command::Filter {
                trx: 0,
                low: 75,
                high: 2825
            }]
        );
        let mut e = engine_with_f4(Action::FilterNarrower);
        let mut s = connected();
        assert_eq!(
            tap(&mut e, &mut s, now),
            vec![Command::Filter {
                trx: 0,
                low: 125,
                high: 2775
            }]
        );
    }

    #[test]
    fn filter_default_restores_the_saved_filter() {
        let mut e = engine_with_f4(Action::FilterDefault);
        let mut s = connected();
        assert_eq!(
            tap(&mut e, &mut s, Instant::now()),
            vec![Command::Filter {
                trx: 0,
                low: 100,
                high: 2500
            }]
        );
    }

    #[test]
    fn agc_next_cycles_and_agc_sets() {
        let now = Instant::now();
        let mut e = engine_with_f4(Action::AgcNext);
        let mut s = connected();
        s.trx_mut(0).agc_mode = Some("normal".into());
        let agc = |m: &str| {
            vec![Command::AgcMode {
                trx: 0,
                mode: m.into(),
            }]
        };
        assert_eq!(tap(&mut e, &mut s, now), agc("fast"));
        assert_eq!(e.view(now).flash.as_deref(), Some("AGC  FASt"));
        assert_eq!(tap(&mut e, &mut s, now), agc("long"), "wraps round");
        let mut e = engine_with_f4(Action::Agc("off".into()));
        assert_eq!(tap(&mut e, &mut s, now), agc("off"));
    }

    #[test]
    fn toggles_send_the_opposite_state() {
        let now = Instant::now();
        let mut s = connected();
        s.trx_mut(1).rx_enable = false;
        s.trx_mut(1).rx_mute = true;
        s.monitor = true;
        let cases = [
            (Action::Rx2, Command::RxEnable { trx: 1, on: true }),
            (Action::SubRx, Command::SubRx { trx: 0, on: true }),
            (Action::Apf, Command::Apf { trx: 0, on: true }),
            (Action::Binaural, Command::Binaural { trx: 0, on: true }),
            (Action::Rx1Mute, Command::RxMute { trx: 0, on: true }),
            (Action::Rx2Mute, Command::RxMute { trx: 1, on: false }),
            (Action::Monitor, Command::Monitor(false)),
            (Action::Squelch, Command::Squelch { trx: 0, enable: true }),
            (
                Action::VfoBToA,
                Command::Vfo {
                    trx: 0,
                    vfo: 0,
                    hz: 7_074_000,
                },
            ),
            (Action::Power, Command::Power(false)),
            (Action::Focus, Command::Focus),
        ];
        for (a, want) in cases {
            let mut e = engine_with_f4(a.clone());
            let mut st = s.clone();
            assert_eq!(tap(&mut e, &mut st, now), vec![want], "{a}");
        }
    }

    #[test]
    fn backlight_next_cycles_presets_without_a_radio() {
        let mut e = engine_with_f4(Action::BacklightNext);
        let mut s = RadioState::default();
        assert_eq!(e.config().display.backlight, [255, 255, 255], "white");
        assert!(tap(&mut e, &mut s, Instant::now()).is_empty());
        assert_eq!(e.config().display.backlight, [255, 180, 90], "warm white");
    }

    #[test]
    fn unlabelled_knob_functions() {
        let mut cfg = Config::default();
        cfg.e1.functions = vec![
            KnobFunction::CwSpeed,
            KnobFunction::RxVol,
            KnobFunction::TuneDrive,
            KnobFunction::MonVol,
        ];
        cfg.e1.start = None;
        let mut e = Engine::new(cfg);
        let mut s = connected();
        s.cw_speed = Some(22);
        s.trx_mut(0).rx_volume_db = Some(-1.94); // Thetis RX1 AF 80
        s.trx_mut(0).tune_drive = Some(28);
        s.mon_volume_db = Some(-30.0);
        let now = Instant::now();
        let click = |e: &mut Engine, s: &mut RadioState| e.handle(turn(Encoder::E1, 2), now, s);
        assert_eq!(click(&mut e, &mut s), vec![Command::CwSpeed(23)]);
        assert_eq!(e.view(now).flash.as_deref(), Some("SPd    23"));
        e.handle(
            InputEvent::Pressed(Button::E1Push),
            now + Duration::from_secs(1),
            &mut s,
        );
        e.handle(
            InputEvent::Released(Button::E1Push),
            now + Duration::from_secs(1),
            &mut s,
        );
        let c = click(&mut e, &mut s);
        let Command::RxVolume { trx: 0, db } = c[0] else {
            panic!("{c:?}")
        };
        assert_eq!(rx_volume_pct(db), 81);
        e.handle(
            InputEvent::Pressed(Button::E1Push),
            now + Duration::from_secs(2),
            &mut s,
        );
        e.handle(
            InputEvent::Released(Button::E1Push),
            now + Duration::from_secs(2),
            &mut s,
        );
        assert_eq!(
            click(&mut e, &mut s),
            vec![Command::TuneDrive { trx: 0, pct: 29 }]
        );
    }

    #[test]
    fn rx_volume_matches_thetis_every_step() {
        assert_eq!(rx_volume_pct(-6.02), 50);
        assert_eq!(rx_volume_pct(-1.94), 80);
        for pct in 0..=100 {
            let db = rx_volume_db(pct);
            // Thetis truncates 10^(dB/20) x 100 to a whole number.
            let thetis = (10f64.powf(db as f64 / 20.0) * 100.0) as i32;
            let want = if pct == 0 { 0 } else { pct };
            assert_eq!(thetis, want, "{pct}% -> {db} dB");
            assert_eq!(rx_volume_pct(db), want);
        }
    }

    fn mode(m: Mode) -> Vec<Command> {
        vec![Command::Mode { trx: 0, mode: m }]
    }

    /// piHPSDR lists "CW" and "FMN" and reports CWL / CWU both as "CW".
    #[test]
    fn mode_cycle_with_plain_cw_and_fmn() {
        let now = Instant::now();
        let mut next = engine_with_f4(Action::ModeNext);
        let mut prev = engine_with_f4(Action::ModePrev);
        let mut s = connected();
        s.modes = "LSB,USB,DSB,CW,FMN,AM,DIGU,SPEC,DIGL,SAM,DRM"
            .split(',')
            .map(Mode::from_name)
            .collect();
        // Default cycle: LSB, USB, CWU, AM, FM, DIGU.
        assert_eq!(tap(&mut next, &mut s, now), mode(Mode::CwU));
        s.trx_mut(0).mode = Some(Mode::Cw); // what piHPSDR reports back
        assert_eq!(tap(&mut next, &mut s, now), mode(Mode::Am));
        s.trx_mut(0).mode = Some(Mode::Am);
        assert_eq!(tap(&mut next, &mut s, now), mode(Mode::Fm));
        s.trx_mut(0).mode = Some(Mode::Cw);
        assert_eq!(tap(&mut prev, &mut s, now), mode(Mode::Usb));
    }

    #[test]
    fn plain_cw_never_stalls_between_cwl_and_cwu() {
        let now = Instant::now();
        let with = |a: Action| {
            let mut cfg = Config::default();
            cfg.buttons.f4.tap = Some(a);
            cfg.buttons.f4.hold = None;
            cfg.tuning.mode_cycle = ["USB", "CWL", "CWU", "AM"].map(String::from).to_vec();
            Engine::new(cfg)
        };
        let mut s = connected();
        s.modes = vec![Mode::Usb, Mode::Cw, Mode::Am];
        s.trx_mut(0).mode = Some(Mode::Cw);
        assert_eq!(tap(&mut with(Action::ModeNext), &mut s, now), mode(Mode::Am));
        s.trx_mut(0).mode = Some(Mode::Cw);
        assert_eq!(tap(&mut with(Action::ModePrev), &mut s, now), mode(Mode::Usb));
    }

    /// piHPSDR's CW edges are relative to the CW pitch, so plain CW gets a
    /// filter centred on 0 that suits CWL and CWU alike.
    #[test]
    fn filter_default_in_plain_cw_is_centred_on_the_pitch() {
        let mut e = engine_with_f4(Action::FilterDefault);
        let mut s = connected();
        s.trx_mut(0).mode = Some(Mode::Cw);
        assert_eq!(
            tap(&mut e, &mut s, Instant::now()),
            vec![Command::Filter {
                trx: 0,
                low: -250,
                high: 250
            }]
        );
    }

    /// A connected IC-7100 on kadrad-hub, as its TCI-X manifest describes it
    /// (tci-x/examples/ic7100_tcix.txt, the lines that matter here): named
    /// filter presets, no continuous passband, AF gain and squelch as 0-100,
    /// no RF gain, lock, mute or volume.
    fn ic7100(extra: &[&str]) -> RadioState {
        let mut s = connected();
        let mut m = crate::Manifest::default();
        for line in [
            "vfo,rw,range,30000,470000000,1,hz",
            "modulation,rw,enum,lsb,usb,am,cw,cwr,rtty,rttyr,nfm,wfm,digl,digu,digfm,dv",
            "trx,rw,bool",
            "split_enable,rw,bool",
            "drive,rw,range,0,100,1,pct",
            "filter,rw,enum,fil1,fil2,fil3",
            "rx_nb_enable,rw,bool",
            "rx_nr_enable,rw,bool",
            "rx_anf_enable,rw,bool",
            "rit_offset,rw,range,-9999,9999,1,hz",
            "af_gain,rw,range,0,100,1,pct",
            "squelch,rw,range,0,100,1,pct",
            "band,w,enum,160m,80m,40m,30m,20m,17m,15m,12m,10m,6m,2m,70cm",
        ]
        .iter()
        .chain(extra)
        {
            let args: Vec<String> = line.split(',').map(str::to_string).collect();
            assert!(m.apply(&args), "{line}");
        }
        s.tcix = Some(m);
        let t = s.trx_mut(0);
        t.filter_preset = Some("fil1".into());
        t.af_gain = Some(30);
        s
    }

    fn act(e: &mut Engine, a: Action, s: &mut RadioState) -> Vec<Command> {
        let mut out = Vec::new();
        e.action(&a, Instant::now(), s, &mut out);
        out
    }

    #[test]
    fn tcix_knobs_use_the_extensions() {
        let mut e = Engine::new(Config::default());
        let mut s = ic7100(&[]);
        let now = Instant::now();
        // E1 VOL is the radio's AF gain.
        assert_eq!(
            e.handle(turn(Encoder::E1, 2), now, &mut s),
            vec![Command::AfGain { trx: 0, pct: 31 }]
        );
        assert_eq!(e.view(now).flash.as_deref(), Some("VOL    31"));
        // E2 WIDTH steps the filter presets, and stops at the last.
        assert_eq!(
            e.handle(turn(Encoder::E2, 2), now, &mut s),
            vec![Command::FilterPreset {
                trx: 0,
                name: "fil2".into()
            }]
        );
        assert_eq!(e.view(now).flash.as_deref(), Some("FIL     2"));
        e.handle(turn(Encoder::E2, 4), now, &mut s);
        assert_eq!(s.trx[0].filter_preset.as_deref(), Some("fil3"));
        assert!(e.handle(turn(Encoder::E2, 2), now, &mut s).is_empty());
    }

    #[test]
    fn tcix_knobs_skip_what_the_radio_cant_do() {
        let mut e = Engine::new(Config::default());
        let mut s = ic7100(&[]);
        let now = Instant::now();
        assert!(!available(KnobFunction::Rfg, &s) && !available(KnobFunction::High, &s));
        // E1 on RFG (no rf_gain) moves on to DRV; E2 on HIGH moves on to WIDTH.
        e.e_idx = [2, 0];
        e.tick(now, &mut s);
        assert_eq!(
            (e.view(now).e1, e.view(now).e2),
            (KnobFunction::Drv, KnobFunction::Width)
        );
        // Stepping E2 from WIDTH skips SHIFT (no passband) to RIT, then XIT
        // (not offered) back round to WIDTH.
        act(&mut e, Action::E2Next, &mut s);
        assert_eq!(e.view(now).e2, KnobFunction::Rit);
        act(&mut e, Action::E2Next, &mut s);
        assert_eq!(e.view(now).e2, KnobFunction::Width);
        // Without TCI-X every function is offered, as before.
        assert!(available(KnobFunction::Rfg, &connected()));
    }

    #[test]
    fn tcix_sends_only_what_the_manifest_takes() {
        let mut e = Engine::new(Config::default());
        let mut s = ic7100(&[]);
        assert!(act(&mut e, Action::Mute, &mut s).is_empty());
        assert!(act(&mut e, Action::Lock, &mut s).is_empty());
        assert!(act(&mut e, Action::Notch, &mut s).is_empty());
        assert!(!s.mute && !s.trx[0].lock, "nothing applied either");
        assert_eq!(
            act(&mut e, Action::Mox, &mut s),
            vec![Command::Mox { trx: 0, on: true }]
        );
        // Unkeying is never refused, even by a manifest without `trx`.
        let mut bare = connected();
        bare.tcix = Some(crate::Manifest::default());
        assert!(bare.allows(&Command::Mox { trx: 0, on: false }));
        assert!(!bare.allows(&Command::Mox { trx: 0, on: true }));
    }

    #[test]
    fn tcix_band_keys_use_the_radio_band_stack() {
        let mut e = Engine::new(Config::default());
        let mut s = ic7100(&[]);
        assert_eq!(
            act(&mut e, Action::BandUp, &mut s),
            vec![Command::Band {
                trx: 0,
                name: "17m".into()
            }]
        );
        // The frequency is left for the radio to report.
        assert_eq!(s.trx[0].vfo[0], 14_074_000);
        s.trx[0].vfo[0] = 50_313_000;
        assert_eq!(
            act(&mut e, Action::BandUp, &mut s),
            vec![Command::Band {
                trx: 0,
                name: "2m".into()
            }]
        );
    }

    #[test]
    fn tcix_lock_notch_and_tuner() {
        let mut e = Engine::new(Config::default());
        let mut s = ic7100(&[
            "dial_lock,rw,bool",
            "notch,rw,range,-1040,4040,1,hz",
            "atu_tune,w,action",
        ]);
        let now = Instant::now();
        assert_eq!(act(&mut e, Action::Lock, &mut s), vec![Command::DialLock(true)]);
        assert!(
            e.handle(turn(Encoder::Main, 1), now, &mut s).is_empty(),
            "dial locked"
        );
        // No notch frequency known yet: nothing to send.
        assert!(act(&mut e, Action::Notch, &mut s).is_empty());
        s.trx[0].notch_hz = Some(1200);
        assert_eq!(
            act(&mut e, Action::Notch, &mut s),
            vec![Command::NotchAt {
                trx: 0,
                on: true,
                hz: 1200
            }]
        );
        assert_eq!(
            act(&mut e, Action::Tune, &mut s),
            vec![Command::AtuTune { trx: 0 }]
        );
        s.trx[0].tx = true;
        assert_eq!(
            act(&mut e, Action::Tune, &mut s),
            vec![Command::Mox { trx: 0, on: false }]
        );
    }

    #[test]
    fn alerts_flash_once() {
        let mut e = Engine::new(Config::default());
        let mut s = ic7100(&[]);
        let t0 = Instant::now();
        s.push_alert("busy");
        e.tick(t0, &mut s);
        assert_eq!(e.view(t0).flash.as_deref(), Some("busy"));
        // Gone after flash_ms, and not shown again by later ticks.
        let later = t0 + Duration::from_secs(5);
        e.tick(later, &mut s);
        assert_eq!(e.view(later).flash, None);
    }

    #[test]
    fn any_input_wakes_after_an_idle_close() {
        let mut e = Engine::new(Config::default());
        let mut s = RadioState {
            idle_closed: true,
            ..Default::default()
        };
        e.handle(InputEvent::Pressed(Button::F2), Instant::now(), &mut s);
        assert!(!s.idle_closed);
    }

    #[test]
    fn vfo_select_follows_and_sets_the_radio() {
        let mut e = Engine::new(Config::default());
        let mut s = ic7100(&["vfo_select,rw,enum,0,1"]);
        let now = Instant::now();
        assert_eq!(
            act(&mut e, Action::VfoToggle, &mut s),
            vec![Command::VfoSelect { trx: 0, ch: 1 }]
        );
        assert_eq!(e.view(now).active_vfo, 1);
        // The radio switches back to A (its own VFO key): we follow.
        s.trx[0].vfo_selected = Some(0);
        e.tick(now, &mut s);
        assert_eq!(e.view(now).active_vfo, 0);
    }

    #[test]
    fn knob_values_are_guarded_and_stop_at_the_end() {
        let mut e = Engine::new(Config::default());
        let mut s = ic7100(&[]);
        let now = Instant::now();
        assert_eq!(
            e.handle(turn(Encoder::E1, 2), now, &mut s),
            vec![Command::AfGain { trx: 0, pct: 31 }]
        );
        // The radio's reports of it are ignored while the knob is in use.
        let g = |s: &RadioState, name: &str, t: Instant| s.guarding(name, &["0".into()], t).is_some();
        assert!(g(&s, "af_gain", now + Duration::from_millis(100)));
        assert!(!g(&s, "af_gain", now + Duration::from_secs(2)));
        assert!(!g(&s, "squelch", now));
        assert!(
            s.guarding("af_gain", &["1".into()], now).is_none(),
            "RX2 not guarded"
        );
        // At the top of its travel there's nothing more to send.
        s.trx[0].af_gain = Some(100);
        assert!(e.handle(turn(Encoder::E1, 2), now, &mut s).is_empty());
        assert_eq!(e.view(now).flash.as_deref(), Some("VOL   100"));
    }

    #[test]
    fn mode_stepping_uses_the_modes_the_server_has() {
        // kadrad-hub's IC-7100: `cw` and `cwr`, `nfm` and `wfm`, no `FM`.
        let mut e = Engine::new(Config::default());
        let mut s = connected();
        s.mode_names = "lsb,usb,am,cw,cwr,rtty,rttyr,nfm,wfm,digl,digu,digfm,dv"
            .split(',')
            .map(str::to_string)
            .collect();
        s.modes = s.mode_names.iter().map(|n| Mode::from_name(n)).collect();
        // The default cycle: LSB, USB, CWU, AM, FM, DIGU.
        let mut seen = Vec::new();
        s.trx[0].mode = Some(Mode::Usb);
        for _ in 0..6 {
            let out = act(&mut e, Action::ModeNext, &mut s);
            let [Command::Mode { mode, .. }] = out[..] else {
                panic!("{out:?}")
            };
            let name = s.mode_wire_name(mode);
            // The radio reports the mode by its own name.
            s.trx[0].mode = Some(Mode::from_name(&name));
            seen.push(name);
        }
        assert_eq!(seen, ["cw", "am", "nfm", "digu", "lsb", "usb"]);
        // A server with Thetis's names is sent those.
        let mut thetis = connected();
        thetis.mode_names = vec![
            "LSB".into(),
            "USB".into(),
            "CWL".into(),
            "CWU".into(),
            "FM".into(),
        ];
        thetis.modes = thetis.mode_names.iter().map(|n| Mode::from_name(n)).collect();
        assert_eq!(thetis.mode_wire_name(Mode::CwU), "CWU");
        assert_eq!(thetis.mode_wire_name(Mode::Fm), "FM");
        // With no list, our own names.
        assert_eq!(connected().mode_wire_name(Mode::CwU), "CWU");
    }

    #[test]
    fn vol_is_the_programs_volume_where_the_server_has_one() {
        // KADRAD's relay: the hub's manifest plus its own `volume`.
        let mut e = Engine::new(Config::default());
        let mut s = ic7100(&["volume,rw,range,-60,0,0.1,db"]);
        s.volume_db = Some(-30.0);
        let now = Instant::now();
        let out = e.handle(turn(Encoder::E1, 2), now, &mut s);
        assert!(
            matches!(out[..], [Command::Volume { db }] if (-29.5..-29.3).contains(&db)),
            "{out:?}"
        );
        assert_eq!(e.view(now).flash.as_deref(), Some("VOL    51"));
        // The radio's AF gain is left alone.
        assert_eq!(s.trx[0].af_gain, Some(30));
    }

    #[test]
    fn preamp_and_att_buttons_step_the_radios_settings() {
        let mut e = Engine::new(Config::default());
        let mut s = ic7100(&["preamp,rw,enum,off,p1,p2", "attenuator,rw,enum,0,12"]);
        s.trx[0].preamp = Some("off".into());
        let mut seen = Vec::new();
        for _ in 0..4 {
            act(&mut e, Action::Preamp, &mut s);
            seen.push(s.trx[0].preamp.clone().unwrap());
        }
        assert_eq!(seen, ["p1", "p2", "off", "p1"]);
        // Not yet reported by the radio: the first in its list.
        assert_eq!(
            act(&mut e, Action::Att, &mut s),
            vec![Command::Attenuator {
                trx: 0,
                value: "0".into()
            }]
        );
        assert_eq!(
            act(&mut e, Action::Att, &mut s),
            vec![Command::Attenuator {
                trx: 0,
                value: "12".into()
            }]
        );
        // A radio without them: nothing to send.
        let mut plain = ic7100(&[]);
        assert!(act(&mut e, Action::Preamp, &mut plain).is_empty());
        assert!(act(&mut e, Action::Att, &mut plain).is_empty());
    }

    #[test]
    fn the_dial_goes_as_far_as_the_manifest_says() {
        // A FlexRadio with a 2 m transverter: the greeting's limits stop at
        // 54 MHz, and the manifest's `vfo` range, sent once the server knows
        // the radio's transverters, reaches 2 m.
        let mut e = Engine::new(Config::default());
        let mut s = connected();
        s.vfo_limits = Some((30_000, 54_000_000));
        s.trx[0].vfo[0] = 144_300_000;
        let now = Instant::now();
        // Plain TCI: held to the greeting's limits, as before.
        assert_eq!(s.dial_limits(), (30_000, 54_000_000));
        let mut m = crate::Manifest::default();
        for line in [
            "vfo,rw,range,30000,54000000,1,hz",
            "vfo,rw,range,30000,148000000,1,hz",
        ] {
            let args: Vec<String> = line.split(',').map(str::to_string).collect();
            assert!(m.apply(&args));
        }
        s.tcix = Some(m);
        assert_eq!(s.dial_limits(), (30_000, 148_000_000));
        let out = e.handle(turn(Encoder::Main, 1), now, &mut s);
        let [Command::Vfo { hz, .. }] = out[..] else {
            panic!("{out:?}")
        };
        assert!((144_300_000..144_301_000).contains(&hz), "stayed on 2 m: {hz}");
        // A manifest without a range for `vfo` falls back to the greeting's.
        s.tcix = Some(crate::Manifest::default());
        assert_eq!(s.dial_limits(), (30_000, 54_000_000));
    }

    #[test]
    fn control_button_asks_or_gives_up() {
        let mut e = Engine::new(Config::default());
        let mut s = ic7100(&["control_request,w,action", "control_release,w,action"]);
        s.login_user = Some("g0kad".into());
        s.control = Some("g4abc".into());
        assert_eq!(
            act(&mut e, Action::Control, &mut s),
            vec![Command::ControlRequest]
        );
        s.control = Some("g0kad".into());
        assert_eq!(
            act(&mut e, Action::Control, &mut s),
            vec![Command::ControlRelease]
        );
        // A server without control: nothing to send.
        let mut plain = ic7100(&[]);
        assert!(act(&mut e, Action::Control, &mut plain).is_empty());
    }
}
