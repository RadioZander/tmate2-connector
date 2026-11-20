//! `tmate2`: the TMATE-2 connector (headless for now).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use clap::Parser;
use tokio::sync::mpsc;
use tracing::{info, warn};

use tmate2_backends::{tci_client, SharedState};
use tmate2_core::config::{Config, DEFAULT_TOML};
use tmate2_core::render::render;
use tmate2_core::{Command, Engine, RadioState};
use tmate2_hid::hidapi::HidApi;
use tmate2_hid::{InputTracker, SegmentMap, Tmate2};

const FRAME: Duration = Duration::from_millis(50);
const READ_TIMEOUT_MS: i32 = 10;

#[derive(Parser)]
#[command(version, about = "Connects the ELAD / Woodbox TMATE-2 to your radio")]
struct Cli {
    /// Configuration file (default: the per-user config folder; created with
    /// commented defaults if it does not exist).
    #[arg(long)]
    config: Option<PathBuf>,
    /// LCD segment map overriding entries of the built-in one.
    #[arg(long)]
    segmap: Option<PathBuf>,
    /// Print the default configuration and exit.
    #[arg(long)]
    print_default_config: bool,
    /// More logging (-v: debug, -vv: trace). RUST_LOG also works.
    #[arg(short, long, action = clap::ArgAction::Count)]
    verbose: u8,
}

fn default_config_path() -> PathBuf {
    directories::ProjectDirs::from("", "", "tmate2-connector")
        .map(|d| d.config_dir().join("config.toml"))
        .unwrap_or_else(|| PathBuf::from("tmate2.toml"))
}

fn load_config(path: &Path) -> Result<Config> {
    if !path.exists() {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        std::fs::write(path, DEFAULT_TOML).with_context(|| format!("writing {}", path.display()))?;
        info!("created default configuration at {}", path.display());
    }
    let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    Config::parse(&text).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    if cli.print_default_config {
        print!("{DEFAULT_TOML}");
        return Ok(());
    }
    let level = ["info", "debug", "trace"][cli.verbose.min(2) as usize];
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(level)),
        )
        .init();

    let path = cli.config.unwrap_or_else(default_config_path);
    let cfg = load_config(&path)?;
    info!("configuration: {}", path.display());

    let mut map = SegmentMap::builtin();
    if let Some(p) = &cli.segmap {
        let text = std::fs::read_to_string(p).with_context(|| format!("reading {}", p.display()))?;
        map.merge(&SegmentMap::from_toml(&text)?);
    }

    let state: SharedState = Arc::new(Mutex::new(RadioState::default()));
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
    let stop = Arc::new(AtomicBool::new(false));

    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    rt.spawn(tci_client::run(cfg.radio.clone(), state.clone(), cmd_rx));
    {
        let stop = stop.clone();
        rt.spawn(async move {
            if tokio::signal::ctrl_c().await.is_ok() {
                info!("stopping");
                stop.store(true, Ordering::SeqCst);
            }
        });
    }

    device_loop(cfg, &map, &state, &cmd_tx, &stop)?;
    rt.shutdown_timeout(Duration::from_millis(500));
    Ok(())
}

/// Owns the TMATE-2: reads input, runs the engine, draws the display.
/// Reconnects if the device is unplugged.
fn device_loop(
    cfg: Config,
    map: &SegmentMap,
    state: &SharedState,
    cmd_tx: &mpsc::UnboundedSender<Command>,
    stop: &AtomicBool,
) -> Result<()> {
    let mut api = HidApi::new().context("initialising HID")?;
    let mut engine = Engine::new(cfg.clone());
    let mut warned = false;

    while !stop.load(Ordering::SeqCst) {
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

        let mut tracker = InputTracker::new();
        let mut events = Vec::new();
        let mut last_draw = Instant::now() - FRAME;
        let send = |cmds: Vec<Command>| {
            for c in cmds {
                let _ = cmd_tx.send(c);
            }
        };

        let lost = loop {
            if stop.load(Ordering::SeqCst) {
                send(engine.release_all(Instant::now(), &mut state.lock().unwrap()));
                let o = dev.output_mut();
                o.clear_segments();
                o.set_rgb(0, 0, 0);
                o.set_usb_led(false);
                o.set_lock_led(false);
                let _ = dev.flush();
                return Ok(());
            }
            match dev.read(READ_TIMEOUT_MS) {
                Ok(Some(report)) => {
                    events.clear();
                    tracker.feed(report, &mut events);
                    let now = Instant::now();
                    for ev in events.drain(..) {
                        let cmds = engine.handle(ev, now, &mut state.lock().unwrap());
                        send(cmds);
                    }
                }
                Ok(None) => {}
                Err(e) => break e,
            }
            let now = Instant::now();
            send(engine.tick(now, &mut state.lock().unwrap()));
            if now.duration_since(last_draw) >= FRAME {
                let snapshot = state.lock().unwrap().clone();
                render(&snapshot, &engine.view(now), &cfg.display, map, dev.output_mut());
                if let Err(e) = dev.flush() {
                    break e;
                }
                last_draw = now;
            }
        };
        warn!("TMATE-2 disconnected: {lost}");
        // Never leave the radio transmitting because a PTT button was held
        // when the device went away.
        for c in engine.release_all(Instant::now(), &mut state.lock().unwrap()) {
            let _ = cmd_tx.send(c);
        }
        let _ = api.refresh_devices();
    }
    Ok(())
}
