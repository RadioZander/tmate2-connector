//! Owns the TMATE-2: reads input, runs the engine, draws the display.
//! Reconnects if the device is unplugged; picks up new settings live.

use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use tokio::sync::mpsc::UnboundedSender;
use tracing::{debug, info, trace, warn};

use tmate2_core::render::render;
use tmate2_core::{Command, Config, Engine};
use tmate2_hid::hidapi::HidApi;
use tmate2_hid::{InputTracker, SegmentMap, Tmate2};

use crate::shared::Shared;

const FRAME: Duration = Duration::from_millis(50);
const READ_TIMEOUT_MS: i32 = 10;

pub fn run(
    cfg: Config,
    map: SegmentMap,
    shared: &Shared,
    cmd_tx: UnboundedSender<Command>,
    config_rx: mpsc::Receiver<Config>,
) -> Result<()> {
    let mut api = HidApi::new().context("initialising HID")?;
    let mut engine = Engine::new(cfg);
    let mut warned = false;
    let send = |cmds: Vec<Command>| {
        for c in cmds {
            let _ = cmd_tx.send(c);
        }
    };

    while !shared.stop.load(Ordering::SeqCst) {
        while let Ok(c) = config_rx.try_recv() {
            engine.set_config(c);
        }
        let mut dev = match Tmate2::open(&api) {
            Ok(d) => d,
            Err(e) => {
                if !warned {
                    warn!("TMATE-2 not available ({e}); waiting for it");
                    warned = true;
                }
                std::thread::sleep(Duration::from_secs(1));
                let _ = api.refresh_devices();
                continue;
            }
        };
        warned = false;
        info!("TMATE-2 connected");
        shared.device_connected.store(true, Ordering::SeqCst);

        let mut tracker = InputTracker::new();
        let mut events = Vec::new();
        let mut last_draw = Instant::now() - FRAME;

        let lost = loop {
            if shared.stop.load(Ordering::SeqCst) {
                send(engine.release_all(Instant::now(), &mut shared.radio.lock().unwrap()));
                let o = dev.output_mut();
                o.clear_segments();
                o.set_rgb(0, 0, 0);
                o.set_usb_led(false);
                o.set_lock_led(false);
                let _ = dev.flush();
                shared.device_connected.store(false, Ordering::SeqCst);
                return Ok(());
            }
            while let Ok(c) = config_rx.try_recv() {
                debug!("settings applied");
                engine.set_config(c);
            }
            match dev.read(READ_TIMEOUT_MS) {
                Ok(Some(report)) => {
                    events.clear();
                    tracker.feed(report, &mut events);
                    let now = Instant::now();
                    for ev in events.drain(..) {
                        trace!("input {ev:?}");
                        *shared.last_input.lock().unwrap() = Some((ev, now));
                        let cmds = engine.handle(ev, now, &mut shared.radio.lock().unwrap());
                        send(cmds);
                    }
                }
                Ok(None) => {}
                Err(e) => break e,
            }
            let now = Instant::now();
            send(engine.tick(now, &mut shared.radio.lock().unwrap()));
            if now.duration_since(last_draw) >= FRAME {
                let snapshot = shared.radio.lock().unwrap().clone();
                let cfg = engine.config().display.clone();
                render(&snapshot, &engine.view(now), &cfg, &map, dev.output_mut());
                if shared.all_segments_until.lock().unwrap().is_some_and(|t| now < t) {
                    dev.output_mut().all_segments();
                }
                if let Err(e) = dev.flush() {
                    break e;
                }
                *shared.lcd.lock().unwrap() = dev.output().clone();
                last_draw = now;
            }
        };
        warn!("TMATE-2 disconnected: {lost}");
        shared.device_connected.store(false, Ordering::SeqCst);
        // Never leave the radio transmitting because a PTT button was held
        // when the device went away.
        send(engine.release_all(Instant::now(), &mut shared.radio.lock().unwrap()));
        let _ = api.refresh_devices();
    }
    Ok(())
}
