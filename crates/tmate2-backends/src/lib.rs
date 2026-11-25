//! Radio backends. Each turns [`tmate2_core::Command`]s into its protocol and
//! keeps the shared [`tmate2_core::RadioState`] up to date.

pub mod login;
pub mod pacing;
pub mod tci;
pub mod tci_client;

use std::sync::{Arc, Mutex};

use tmate2_core::RadioState;

/// Radio state shared between the backend task and the device loop.
pub type SharedState = Arc<Mutex<RadioState>>;
