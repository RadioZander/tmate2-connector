//! `tmate2`: the TMATE-2 connector.

// Windows release builds with the GUI start without a console window; see
// `windows_console` for how terminal output still works.
#![cfg_attr(
    all(windows, feature = "gui", not(debug_assertions)),
    windows_subsystem = "windows"
)]

mod device;
#[cfg(feature = "gui")]
mod gui;
mod launch;
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
#[command(
    name = "tmate2",
    version,
    about = "Connects the ELAD / Woodbox TMATE-2 to your radio"
)]
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
    /// TCI server host for this run, over [radio] host in the configuration
    /// (which is left as it is).
    #[arg(long, value_name = "HOST")]
    host: Option<String>,
    /// TCI server port for this run, over [radio] port in the configuration
    /// (which is left as it is).
    #[arg(long, value_name = "PORT")]
    port: Option<u16>,
    /// Start with the window minimised.
    #[arg(long, visible_alias = "minimised")]
    minimized: bool,
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

/// A program built for the Windows GUI subsystem has no console. Attach to the
/// terminal it was started from, so `--help` and logs still appear there;
/// for `--headless` started without a terminal, open a console for the log.
#[cfg(all(windows, feature = "gui", not(debug_assertions)))]
fn windows_console() {
    use std::ptr::null;
    use windows_sys::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows_sys::Win32::System::Console::{
        AllocConsole, AttachConsole, GetStdHandle, SetStdHandle, ATTACH_PARENT_PROCESS, STD_ERROR_HANDLE,
        STD_OUTPUT_HANDLE,
    };
    let headless = std::env::args().any(|a| a == "--headless");
    // SAFETY: Win32 calls with a valid NUL-terminated name and null optional
    // pointers; any failure simply leaves the program without a console.
    unsafe {
        let have_console = AttachConsole(ATTACH_PARENT_PROCESS) != 0 || (headless && AllocConsole() != 0);
        if !have_console {
            return;
        }
        // Output that is already redirected (pipe / file) keeps its handle;
        // otherwise point stdout / stderr at the console.
        let name: Vec<u16> = "CONOUT$\0".encode_utf16().collect();
        for std_handle in [STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
            let h = GetStdHandle(std_handle);
            if h.is_null() || h == INVALID_HANDLE_VALUE {
                let con = CreateFileW(
                    name.as_ptr(),
                    GENERIC_READ | GENERIC_WRITE,
                    FILE_SHARE_READ | FILE_SHARE_WRITE,
                    null(),
                    OPEN_EXISTING,
                    0,
                    std::ptr::null_mut(),
                );
                if con != INVALID_HANDLE_VALUE {
                    SetStdHandle(std_handle, con);
                }
            }
        }
    }
}

fn main() -> Result<()> {
    #[cfg(all(windows, feature = "gui", not(debug_assertions)))]
    windows_console();
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

    // One copy at a time: two would fight over the TMATE-2. Held until the
    // program ends.
    let _lock = launch::one_copy(&launch::lock_path()).map_err(|_| {
        error!("the TMATE-2 connector is already running; this copy is stopping");
        anyhow::anyhow!("the TMATE-2 connector is already running")
    })?;

    let path = cli.config.unwrap_or_else(default_config_path);
    let cfg = load_config(&path)?;
    info!("configuration: {}", path.display());
    let overrides = launch::Overrides {
        host: cli.host.map(|h| h.trim().to_string()).filter(|h| !h.is_empty()),
        port: cli.port,
    };
    if let Some(given) = overrides.describe() {
        info!("connecting where the command line says ({given}), not where the configuration does");
    }

    let mut map = SegmentMap::builtin();
    if let Some(p) = &cli.segmap {
        let text = std::fs::read_to_string(p).with_context(|| format!("reading {}", p.display()))?;
        map.merge(&SegmentMap::from_toml(&text)?);
    }

    let (cmd_tx, cmd_rx) = tokio::sync::mpsc::unbounded_channel();
    let (radio_cfg_tx, radio_cfg_rx) = watch::channel(overrides.apply(&cfg.radio));
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
        gui::run(
            shared.clone(),
            cfg,
            path,
            radio_cfg_tx,
            gui::Start {
                overrides,
                minimized: cli.minimized,
            },
        )?;
        shared.stop.store(true, Ordering::SeqCst);
    }
    // Keep the settings channel open for the lifetime of the program.
    #[cfg(not(feature = "gui"))]
    let _radio_cfg_tx = (cli.headless, cli.minimized, radio_cfg_tx);

    let _ = device.join();
    rt.shutdown_timeout(Duration::from_millis(500));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn the_launch_options_parse_as_kadrad_sends_them() {
        let cli =
            Cli::try_parse_from(["tmate2", "--minimized", "--host", "127.0.0.1", "--port", "40002"]).unwrap();
        assert_eq!(
            (cli.host.as_deref(), cli.port, cli.minimized),
            (Some("127.0.0.1"), Some(40002), true)
        );
        assert!(Cli::try_parse_from(["tmate2", "--minimised"]).unwrap().minimized);
        assert!(Cli::try_parse_from(["tmate2", "--port", "70000"]).is_err());
        // KADRAD passes an option only once --help lists it.
        let help = Cli::command().render_long_help().to_string();
        for option in ["--host", "--port", "--minimized", "minimised"] {
            assert!(help.contains(option), "{option} missing from --help:\n{help}");
        }
    }
}
