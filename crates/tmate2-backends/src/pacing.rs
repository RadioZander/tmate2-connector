//! Pacing of dial and knob values (`vfo`, volume, filter edges, ...).
//!
//! A radio on the same PC takes each value as fast as the dial makes them.
//! A radio a network away (a FlexRadio through a hub) can take longer over
//! each step than the dial takes to make one, and was still tuning long
//! after the dial had stopped. The pacer keeps one value in flight per
//! [`Target`] (command, transceiver, VFO): the next goes when the server
//! has echoed the last, or after [`ECHO_WAIT`]; the newest waits and the
//! steps in between are dropped.
//!
//! With [`Pacing::Auto`] that happens only for a target whose echoes have
//! fallen behind ([`SLOW`]), so a server that keeps up, or never echoes,
//! gets every value at once.

use std::time::{Duration, Instant};

use tmate2_core::config::Pacing;
use tmate2_core::{Command, Target};

/// While paced, values go no more often than this (25 a second)…
pub const PACE: Duration = Duration::from_millis(40);
/// …and not before the server has echoed the last one, or this long has
/// passed without an echo.
pub const ECHO_WAIT: Duration = Duration::from_millis(300);
/// `auto`: a target is paced once its echoes take this long on average…
pub const SLOW: Duration = Duration::from_millis(80);
/// …and sent freely again once they are back under this.
pub const FAST: Duration = Duration::from_millis(30);

pub struct Pacer {
    mode: Pacing,
    slots: Vec<Slot>,
}

struct Slot {
    target: Target,
    /// When the last value went.
    sent: Instant,
    /// The server hasn't echoed since `sent`.
    awaiting: bool,
    /// The oldest value sent since the last echo: how far behind the
    /// server is shows in how long its next echo takes from here.
    since: Option<Instant>,
    /// Smoothed echo delay.
    lag: Option<Duration>,
    /// `auto`: the echoes have fallen behind, so this target is paced.
    slow: bool,
    /// The newest value not yet sent.
    pending: Option<Command>,
}

impl Slot {
    fn free_at(&self) -> Instant {
        self.sent + if self.awaiting { ECHO_WAIT } else { PACE }
    }

    fn sent_now(&mut self, now: Instant) {
        self.sent = now;
        self.awaiting = true;
        self.since.get_or_insert(now);
    }
}

impl Pacer {
    pub fn new(mode: Pacing) -> Pacer {
        Pacer {
            mode,
            slots: Vec::new(),
        }
    }

    /// What a command is paced by; `None` for those that always go at once
    /// (keys, modes, keying).
    fn target(cmd: &Command) -> Option<Target> {
        match cmd {
            Command::Vfo { .. } => cmd.target(),
            c if c.knob_value() => c.target(),
            _ => None,
        }
    }

    fn paced(&self, slot: &Slot) -> bool {
        match self.mode {
            Pacing::Off => false,
            Pacing::Auto => slot.slow,
            Pacing::On => true,
        }
    }

    /// Offer a command: returned if it may go now, else kept as the newest
    /// for its target (replacing any older one waiting there).
    pub fn offer(&mut self, cmd: Command, now: Instant) -> Option<Command> {
        if self.mode == Pacing::Off {
            return Some(cmd);
        }
        let Some(target) = Pacer::target(&cmd) else {
            return Some(cmd);
        };
        let Some(i) = self.slots.iter().position(|s| s.target == target) else {
            self.slots.push(Slot {
                target,
                sent: now,
                awaiting: true,
                since: Some(now),
                lag: None,
                slow: false,
                pending: None,
            });
            return Some(cmd);
        };
        if self.paced(&self.slots[i]) && now < self.slots[i].free_at() {
            self.slots[i].pending = Some(cmd);
            return None;
        }
        let slot = &mut self.slots[i];
        slot.pending = None;
        slot.sent_now(now);
        Some(cmd)
    }

    /// The server reported `name:args;`: what was in flight for that
    /// target has landed (or the radio changed it itself).
    pub fn echoed(&mut self, name: &str, args: &[String], now: Instant) {
        for slot in self.slots.iter_mut().filter(|s| s.target.reported_by(name, args)) {
            slot.awaiting = false;
            if let Some(t0) = slot.since.take() {
                let sample = now.saturating_duration_since(t0);
                let lag = match slot.lag {
                    None => sample,
                    Some(l) => l.mul_f32(0.7) + sample.mul_f32(0.3),
                };
                slot.lag = Some(lag);
                if lag >= SLOW {
                    slot.slow = true;
                } else if lag <= FAST {
                    slot.slow = false;
                }
            }
        }
    }

    /// The waiting values that may go now.
    pub fn due(&mut self, now: Instant) -> Vec<Command> {
        let mut out = Vec::new();
        for slot in &mut self.slots {
            if slot.pending.is_some() && now >= slot.free_at() {
                out.extend(slot.pending.take());
                slot.sent_now(now);
            }
        }
        out
    }

    /// When the next waiting value may go, if any is waiting.
    pub fn next_due(&self) -> Option<Instant> {
        self.slots
            .iter()
            .filter(|s| s.pending.is_some())
            .map(Slot::free_at)
            .min()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    fn vfo(trx: usize, hz: u64) -> Command {
        Command::Vfo { trx, vfo: 0, hz }
    }

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    /// Echo of `vfo(trx, ..)` from the server.
    fn echo(p: &mut Pacer, trx: usize, at: Instant) {
        p.echoed("vfo", &args(&[&trx.to_string(), "0", "14074000"]), at);
    }

    #[test]
    fn off_sends_everything() {
        let mut p = Pacer::new(Pacing::Off);
        let t0 = Instant::now();
        for i in 0..10 {
            assert!(p.offer(vfo(0, i), t0).is_some());
        }
        assert_eq!(p.next_due(), None);
    }

    #[test]
    fn on_keeps_one_in_flight_and_sends_the_newest() {
        let mut p = Pacer::new(Pacing::On);
        let t0 = Instant::now();
        assert_eq!(p.offer(vfo(0, 1), t0), Some(vfo(0, 1)));
        assert_eq!(p.offer(vfo(0, 2), t0 + ms(5)), None);
        assert_eq!(p.offer(vfo(0, 3), t0 + ms(10)), None);
        // No echo: the newest goes after ECHO_WAIT, the one between never.
        assert_eq!(p.next_due(), Some(t0 + ECHO_WAIT));
        assert_eq!(p.due(t0 + ECHO_WAIT), vec![vfo(0, 3)]);
        // With an echo: PACE after the last send.
        let t1 = t0 + ECHO_WAIT;
        assert_eq!(p.offer(vfo(0, 4), t1 + ms(1)), None);
        echo(&mut p, 0, t1 + ms(10));
        assert_eq!(p.next_due(), Some(t1 + PACE));
        assert_eq!(p.due(t1 + PACE), vec![vfo(0, 4)]);
    }

    #[test]
    fn auto_sends_freely_while_the_server_keeps_up() {
        let mut p = Pacer::new(Pacing::Auto);
        let t0 = Instant::now();
        for i in 0..50u64 {
            let at = t0 + ms(10 * i);
            assert!(p.offer(vfo(0, i), at).is_some(), "step {i}");
            echo(&mut p, 0, at + ms(3));
        }
    }

    #[test]
    fn auto_never_holds_back_a_server_that_does_not_echo() {
        let mut p = Pacer::new(Pacing::Auto);
        let t0 = Instant::now();
        for i in 0..50u64 {
            assert!(p.offer(vfo(0, i), t0 + ms(10 * i)).is_some());
        }
    }

    #[test]
    fn auto_paces_once_echoes_fall_behind_and_recovers() {
        let mut p = Pacer::new(Pacing::Auto);
        let t0 = Instant::now();
        // A remote radio: the first echo comes 150 ms after the first step.
        assert!(p.offer(vfo(0, 1), t0).is_some());
        assert!(p.offer(vfo(0, 2), t0 + ms(10)).is_some(), "not known slow yet");
        echo(&mut p, 0, t0 + ms(150));
        // Now paced: one in flight.
        let t1 = t0 + ms(160);
        assert!(p.offer(vfo(0, 3), t1).is_some());
        assert_eq!(p.offer(vfo(0, 4), t1 + ms(10)), None);
        assert_eq!(p.offer(vfo(0, 5), t1 + ms(20)), None);
        echo(&mut p, 0, t1 + ms(120));
        assert_eq!(p.due(t1 + ms(120)), vec![vfo(0, 5)]);
        // The link gets quick again: a few fast echoes bring it back.
        let mut t = t1 + ms(120);
        for i in 0..10u64 {
            echo(&mut p, 0, t + ms(2));
            t += ms(50);
            p.offer(vfo(0, 100 + i), t);
        }
        assert!(p.offer(vfo(0, 200), t + ms(1)).is_some(), "sent freely again");
    }

    #[test]
    fn receivers_and_vfos_are_paced_apart() {
        let mut p = Pacer::new(Pacing::On);
        let t0 = Instant::now();
        assert!(p.offer(vfo(0, 1), t0).is_some());
        assert!(p.offer(vfo(1, 1), t0).is_some(), "RX2 has its own slot");
        let b = Command::Vfo {
            trx: 0,
            vfo: 1,
            hz: 7,
        };
        assert!(p.offer(b, t0).is_some(), "VFO B has its own slot");
        assert_eq!(p.offer(vfo(0, 2), t0 + ms(1)), None);
        // RX2's echo doesn't free RX1.
        echo(&mut p, 1, t0 + ms(5));
        assert_eq!(p.next_due(), Some(t0 + ECHO_WAIT));
        echo(&mut p, 0, t0 + ms(6));
        assert_eq!(p.next_due(), Some(t0 + PACE));
    }

    #[test]
    fn keys_and_modes_are_never_paced() {
        let mut p = Pacer::new(Pacing::On);
        let t0 = Instant::now();
        for _ in 0..3 {
            assert!(p.offer(Command::Mox { trx: 0, on: true }, t0).is_some());
            assert!(p.offer(Command::Nr { trx: 0, on: true }, t0).is_some());
        }
    }
}
