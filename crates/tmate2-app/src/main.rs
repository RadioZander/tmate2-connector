//! `tmate2`: the TMATE-2 connector.

mod device;
#[cfg(feature = "gui")]
mod gui;
mod shared;

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use clap::Parser;
use tokio::sync::watch;
use tracing::{error, info};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::Layer;

use tmate2_backends::tci_client;
use tmate2_core::config::{Config, DEFAULT_TOML};
use tmate2_core::RadioState;
use tmate2_hid::{OutputReport, SegmentMap};

use crate::shared::{LogBuffer, Shared};

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
    /// Run without a window (for servers and the Raspberry Pi).
    #[arg(long)]
    headless: bool,
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

    let log = Arc::new(Mutex::new(VecDeque::new()));
    let level = ["info", "debug", "trace"][cli.verbose.min(2) as usize];
    let filter = || {
        tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(level))
    };
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().with_filter(filter()))
        .with(
            LogBuffer {
                lines: log.clone(),
                start: Instant::now(),
            }
            .with_filter(tracing_subscriber::EnvFilter::new("info")),
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

    let (cmd_tx, cmd_rx) = tokio::sync::mpsc::unbounded_channel();
    let (radio_cfg_tx, radio_cfg_rx) = watch::channel(cfg.radio.clone());
    let (config_tx, config_rx) = mpsc::channel();
    let shared = Arc::new(Shared {
        radio: Arc::new(Mutex::new(RadioState::default())),
        lcd: Mutex::new(OutputReport::default()),
        device_connected: AtomicBool::new(false),
        last_input: Mutex::new(None),
        log,
        stop: AtomicBool::new(false),
        all_segments_until: Mutex::new(None),
        config_tx: Mutex::new(config_tx),
    });

    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    rt.spawn(tci_client::run(radio_cfg_rx, shared.radio.clone(), cmd_rx));
    {
        let shared = shared.clone();
        rt.spawn(async move {
            if tokio::signal::ctrl_c().await.is_ok() {
                info!("stopping");
                shared.stop.store(true, Ordering::SeqCst);
            }
        });
    }

    let device = {
        let shared = shared.clone();
        let cfg = cfg.clone();
        std::thread::Builder::new()
            .name("tmate2-device".into())
            .spawn(move || {
                if let Err(e) = device::run(cfg, map, &shared, cmd_tx, config_rx) {
                    error!("device loop failed: {e:#}");
                }
            })?
    };

    #[cfg(feature = "gui")]
    if !cli.headless {
        gui::run(shared.clone(), cfg, path, radio_cfg_tx)?;
        shared.stop.store(true, Ordering::SeqCst);
    }
    // Keep the settings channel open for the lifetime of the program.
    #[cfg(not(feature = "gui"))]
    let _radio_cfg_tx = (cli.headless, radio_cfg_tx);

    let _ = device.join();
    rt.shutdown_timeout(Duration::from_millis(500));
    Ok(())
}
