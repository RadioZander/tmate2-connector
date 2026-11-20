//! Backend-independent core of the TMATE-2 connector:
//!
//! - [`state`]: what we know about the radio
//! - [`command`]: what we ask the radio to do (backends translate these)
//! - [`config`]: the human-readable TOML configuration
//! - [`engine`]: turns knob / button events into commands
//! - [`render`]: draws the radio state on the TMATE-2 LCD

pub mod bands;
pub mod command;
pub mod config;
pub mod engine;
pub mod render;
pub mod state;

pub use command::Command;
pub use config::Config;
pub use engine::Engine;
pub use state::RadioState;
