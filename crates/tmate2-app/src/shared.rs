//! State shared between the device loop, the radio backend and the GUI.

use std::collections::VecDeque;
use std::fmt;
use std::sync::atomic::AtomicBool;
use std::sync::{mpsc, Arc, Mutex};
use std::time::Instant;

use tmate2_backends::SharedState;
use tmate2_core::Config;
use tmate2_hid::{InputEvent, OutputReport};
use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::{Context, Layer};

const LOG_LINES: usize = 500;

pub struct Shared {
    pub radio: SharedState,
    /// The output report last sent to the TMATE-2 (for the LCD mirror).
    pub lcd: Mutex<OutputReport>,
    pub device_connected: AtomicBool,
    /// Most recent input from the TMATE-2 (for "press a button to find it").
    pub last_input: Mutex<Option<(InputEvent, Instant)>>,
    #[cfg_attr(not(feature = "gui"), allow(dead_code))]
    pub log: Arc<Mutex<VecDeque<String>>>,
    pub stop: AtomicBool,
    /// Light every LCD segment until this time (display test / photos).
    pub all_segments_until: Mutex<Option<Instant>>,
    /// New settings for the device loop.
    #[cfg_attr(not(feature = "gui"), allow(dead_code))]
    pub config_tx: Mutex<mpsc::Sender<Config>>,
}

/// A tracing layer that keeps recent log lines for the GUI's Log tab.
pub struct LogBuffer {
    pub lines: Arc<Mutex<VecDeque<String>>>,
    pub start: Instant,
}

struct MessageVisitor(String);

impl Visit for MessageVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        if field.name() == "message" {
            self.0 = format!("{value:?}");
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.0 = value.to_string();
        }
    }
}

impl<S: Subscriber> Layer<S> for LogBuffer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let mut v = MessageVisitor(String::new());
        event.record(&mut v);
        let line = format!(
            "{:>8.1}s {:<5} {}",
            self.start.elapsed().as_secs_f32(),
            event.metadata().level(),
            v.0
        );
        let mut lines = self.lines.lock().unwrap();
        if lines.len() >= LOG_LINES {
            lines.pop_front();
        }
        lines.push_back(line);
    }
}
