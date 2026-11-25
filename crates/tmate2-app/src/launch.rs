//! Start-up options for another program that starts the connector (KADRAD,
//! say): a server address for this run only, and one copy at a time.

use std::fs::{File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};

use tmate2_core::config::RadioConfig;

/// `--host` / `--port`: where to connect on this run, over `[radio]`. They
/// are never written to the configuration file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Overrides {
    pub host: Option<String>,
    pub port: Option<u16>,
}

impl Overrides {
    /// `radio` with the overrides in place.
    pub fn apply(&self, radio: &RadioConfig) -> RadioConfig {
        let mut r = radio.clone();
        if let Some(h) = &self.host {
            r.host = h.clone();
        }
        if let Some(p) = self.port {
            r.port = p;
        }
        r
    }

    /// What was given, for the log and the window: `--host … --port …`.
    pub fn describe(&self) -> Option<String> {
        let parts: Vec<String> = [
            self.host.as_ref().map(|h| format!("--host {h}")),
            self.port.map(|p| format!("--port {p}")),
        ]
        .into_iter()
        .flatten()
        .collect();
        (!parts.is_empty()).then(|| parts.join(" "))
    }
}

/// Where the lock that keeps to one copy at a time lives.
pub fn lock_path() -> PathBuf {
    directories::ProjectDirs::from("", "", "tmate2-connector")
        .map(|d| d.data_local_dir().join("tmate2.lock"))
        .unwrap_or_else(|| std::env::temp_dir().join("tmate2-connector.lock"))
}

/// Why this copy shouldn't run.
#[derive(Debug, PartialEq, Eq)]
pub struct AlreadyRunning;

/// Hold the lock for as long as the returned file is kept. `Err` if another
/// copy holds it. A lock that can't be made at all (no writable folder) is
/// only warned about: better two copies than none.
pub fn one_copy(path: &Path) -> Result<Option<File>, AlreadyRunning> {
    let open = || -> std::io::Result<File> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(path)
    };
    let file = match open() {
        Ok(f) => f,
        Err(e) => {
            tracing::warn!(
                "can't make the lock {} ({e}); not checking for another copy",
                path.display()
            );
            return Ok(None);
        }
    };
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(TryLockError::WouldBlock) => Err(AlreadyRunning),
        Err(TryLockError::Error(e)) => {
            tracing::warn!(
                "can't lock {} ({e}); not checking for another copy",
                path.display()
            );
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overrides_change_only_what_was_given() {
        let file = tmate2_core::config::Config::default().radio;
        assert_eq!(Overrides::default().apply(&file), file);
        assert_eq!(Overrides::default().describe(), None);
        let port = Overrides {
            port: Some(40002),
            ..Default::default()
        };
        let r = port.apply(&file);
        assert_eq!((r.host.as_str(), r.port), (file.host.as_str(), 40002));
        assert_eq!(port.describe().as_deref(), Some("--port 40002"));
        let both = Overrides {
            host: Some("10.0.0.2".into()),
            port: Some(40001),
        };
        let r = both.apply(&file);
        assert_eq!((r.host.as_str(), r.port), ("10.0.0.2", 40001));
        assert_eq!(both.describe().as_deref(), Some("--host 10.0.0.2 --port 40001"));
    }

    #[test]
    fn a_second_copy_is_turned_away_until_the_first_ends() {
        let path = std::env::temp_dir().join(format!("tmate2-lock-test-{}", std::process::id()));
        let first = one_copy(&path).expect("first copy runs");
        assert!(first.is_some());
        assert_eq!(one_copy(&path).unwrap_err(), AlreadyRunning);
        drop(first);
        assert!(one_copy(&path).expect("runs once the first has ended").is_some());
        let _ = std::fs::remove_file(&path);
    }
}
