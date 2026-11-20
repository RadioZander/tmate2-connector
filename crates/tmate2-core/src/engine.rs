//! The mapping engine: TMATE-2 input events in, radio commands out.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use tmate2_hid::{Button, Encoder, InputEvent};

use crate::bands::{band_of, step_band, step_in_list, BANDS};
use crate::command::Command;
use crate::config::{Action, Config, KnobFunction, VolumeScale};
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

#[derive(Debug, Clone, Copy)]
struct Press {
    at: Instant,
    /// Tap / hold already decided (no further action on release).
    done: bool,
    /// Transmitting because of this press (push-to-talk): unkey on release.
    ptt: bool,
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

    fn trx(&self) -> usize {
        self.cfg.radio.trx
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
                let ptt = !accidental && act.tap == Some(Action::Ptt);
                let immediate = accidental || ptt || act.hold.is_none();
                if ptt {
                    self.key(true, now, state, &mut out);
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
                        ptt,
                        repeat_at: None,
                    },
                );
            }
            InputEvent::Released(b) => {
                if let Some(p) = self.pressed.remove(&b) {
                    if p.ptt {
                        self.key(false, now, state, &mut out);
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
            if a == Action::Ptt {
                if first {
                    p.ptt = true;
                    self.key(true, now, state, &mut out);
                }
                continue;
            }
            p.repeat_at = (a.repeats() && !repeat.is_zero()).then(|| now + repeat);
            self.action(&a, now, state, &mut out);
        }
        out
    }

    /// Unkey any push-to-talk and forget held buttons (device lost, quitting).
    pub fn release_all(&mut self, now: Instant, state: &mut RadioState) -> Vec<Command> {
        let mut out = Vec::new();
        if self.pressed.drain().any(|(_, p)| p.ptt) {
            self.key(false, now, state, &mut out);
        }
        out
    }

    fn key(&self, on: bool, now: Instant, state: &mut RadioState, out: &mut Vec<Command>) {
        if state.connected || !on {
            self.emit(Command::Mox { trx: self.trx(), on }, now, state, out);
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
        if let (true, Some(name)) = (cmd.knob_value(), cmd.wire_name()) {
            state.guard(name, now + KNOB_GUARD);
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

        let step = self.step_hz() as i64;
        let cur = state.trx(t).map_or(0, |x| x.vfo[self.active_vfo]) as i64;
        let target = cur + delta as i64 * step * mult;
        let snapped = ((target as f64 / step as f64).round() as i64) * step;
        let (lo, hi) = state.vfo_limits.unwrap_or((0, 1_000_000_000));
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
        let lsb = x.mode.is_some_and(Mode::is_lower_sideband);
        let cmd = match f {
            KnobFunction::Vol if state.tcix_offers("af_gain") => Some(Command::AfGain {
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
                x.filter.map(|(lo, hi)| {
                    let (mut lo, mut hi) = (lo, hi);
                    match (f, lsb) {
                        // Each edge moves half a step, so the width changes by
                        // one filter step per click about a fixed centre.
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
                        trx: t,
                        low: lo,
                        high: hi,
                    }
                })
            }
            KnobFunction::Rit => Some(Command::RitOffset {
                trx: t,
                hz: (x.rit_offset + rs).clamp(-9990, 9990),
            }),
            KnobFunction::Xit => Some(Command::XitOffset {
                trx: t,
                hz: (x.xit_offset + rs).clamp(-9990, 9990),
            }),
        };
        // At the end of a knob's travel there is nothing new to send.
        if let Some(c) = cmd.filter(|c| !state.holds(c)) {
            self.emit(c, now, state, out);
        }
        self.flash_value(f, now, state);
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
                Action::StepNext | Action::StepPrev | Action::E1Next | Action::E2Next
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
                if !state.modes.is_empty() {
                    cycle.retain(|m| state.modes.contains(m));
                }
                if cycle.is_empty() {
                    vec![]
                } else {
                    let n = cycle.len() as i32;
                    let pos = x.mode.and_then(|m| cycle.iter().position(|c| *c == m));
                    let next = match (pos, *a == Action::ModeNext) {
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
            Action::Control if state.in_control() => vec![Command::ControlRelease],
            Action::Control => vec![Command::ControlRequest],
            Action::VfoAToB => vec![Command::Vfo {
                trx: t,
                vfo: 1,
                hz: x.vfo[0],
            }],
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
            Action::RitClear => vec![
                Command::RitOffset { trx: t, hz: 0 },
                Command::XitOffset { trx: t, hz: 0 },
            ],
            Action::Lock if state.tcix_offers("dial_lock") => vec![Command::DialLock(!state.dial_lock)],
            Action::Lock => vec![Command::Lock { trx: t, on: !x.lock }],
            Action::Mute => vec![Command::Mute(!state.mute)],
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
            // Push-to-talk is handled on press / release, not here.
            Action::Ptt => vec![],
            Action::None => vec![],
        };
        for c in cmds {
            self.emit(c, now, state, out);
        }
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
    }
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
        KnobFunction::Vol if state.tcix_offers("af_gain") => ("VOL", num(x.af_gain)),
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
        assert!(s.guarded("af_gain", now + Duration::from_millis(100)));
        assert!(!s.guarded("af_gain", now + Duration::from_secs(2)));
        assert!(!s.guarded("squelch", now));
        // At the top of its travel there's nothing more to send.
        s.trx[0].af_gain = Some(100);
        assert!(e.handle(turn(Encoder::E1, 2), now, &mut s).is_empty());
        assert_eq!(e.view(now).flash.as_deref(), Some("VOL   100"));
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
