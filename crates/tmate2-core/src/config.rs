//! Human-readable TOML configuration.

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use serde::Deserialize;

/// The commented default configuration; also the single source of defaults.
pub const DEFAULT_TOML: &str = include_str!("default_config.toml");

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub radio: RadioConfig,
    pub display: DisplayConfig,
    pub tuning: TuningConfig,
    pub e1: KnobConfig,
    pub e2: KnobConfig,
    pub buttons: ButtonsConfig,
    /// The desktop window (not used with --headless).
    #[serde(default)]
    pub window: WindowConfig,
    /// Default receive filter per mode name (e.g. "USB"), [low, high] in Hz.
    #[serde(default = "default_filters")]
    pub filters: BTreeMap<String, [i32; 2]>,
}

fn default_filters() -> BTreeMap<String, [i32; 2]> {
    #[derive(Deserialize)]
    struct F {
        filters: BTreeMap<String, [i32; 2]>,
    }
    let section = &DEFAULT_TOML[DEFAULT_TOML
        .find("[filters]")
        .expect("default config has [filters]")..];
    let section = section.split("\n[").next().unwrap_or(section);
    toml::from_str::<F>(section)
        .expect("default [filters] is valid")
        .filters
}

impl Config {
    /// Default filter for a mode: from the file's `[filters]`, else the
    /// built-in default (so a mode added later works with older files).
    pub fn default_filter(&self, mode: &str) -> Option<(i32, i32)> {
        static BUILTIN: std::sync::LazyLock<BTreeMap<String, [i32; 2]>> =
            std::sync::LazyLock::new(default_filters);
        let mode = mode.to_ascii_uppercase();
        self.filters
            .get(&mode)
            .or_else(|| BUILTIN.get(&mode))
            .map(|[lo, hi]| (*lo, *hi))
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RadioConfig {
    pub backend: Backend,
    pub host: String,
    pub port: u16,
    pub tcix: TcixMode,
    /// How dial and knob values are paced to the server.
    #[serde(default)]
    pub pacing: Pacing,
    /// How often what the LCD shows is read from the radio, in ms (0 = never).
    #[serde(default = "default_poll_ms")]
    pub poll_ms: u64,
    /// The numbers behind `pacing`: the tested values unless the file has a
    /// `[radio.pacing_tuning]` table to override them. Not in the window.
    #[serde(default)]
    pub pacing_tuning: PacingTuning,
    /// Friendly names for TCI servers, by `host:port` (`[radio.names]`),
    /// shown in the window instead of the address.
    #[serde(default)]
    pub names: BTreeMap<String, String>,
    pub trx: usize,
    /// TCI-X login (§4.3), for a server that asks for one.
    #[serde(default)]
    pub user: Option<String>,
    #[serde(default)]
    pub password: Option<String>,
}

/// The numbers behind `[radio] pacing`. The defaults are the values tested
/// on Thetis and piHPSDR, and are deliberately not in `default_config.toml`:
/// a user with a slow link (a radio reached over the internet through
/// kadrad-hub, say) can add a `[radio.pacing_tuning]` table with just the
/// keys to change (see the README). Missing keys keep these values.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PacingTuning {
    /// `auto`: pace a target once its echoes average this long (ms)…
    pub on_above_ms: u64,
    /// …and send freely again once they are back under this.
    pub off_below_ms: u64,
    /// While paced, the shortest gap between values (ms).
    pub min_interval_ms: u64,
    /// While paced, how long to wait for an echo before sending anyway (ms).
    pub echo_wait_ms: u64,
    /// How much each new echo counts in the average, 0 to 1 (1 = only the
    /// newest echo).
    pub smoothing: f32,
}

impl Default for PacingTuning {
    fn default() -> Self {
        PacingTuning {
            on_above_ms: 80,
            off_below_ms: 30,
            min_interval_ms: 40,
            echo_wait_ms: 300,
            smoothing: 0.3,
        }
    }
}

fn default_poll_ms() -> u64 {
    1000
}

/// The shortest poll period, in ms: reads faster than this only fill the log.
pub const POLL_MIN_MS: u64 = 100;

/// Environment variable that supplies the TCI-X password instead of the
/// configuration file.
pub const PASSWORD_ENV: &str = "TMATE2_TCIX_PASSWORD";

/// A TCI server as a person would say it: `localhost:50001` for this
/// computer, otherwise the host as written (a name stays a name, an address
/// an address), with an IPv6 address in brackets. `[radio.names]` keys are
/// matched in this form (ignoring case), so `127.0.0.1:50001` and
/// `localhost:50001` agree.
pub fn server_label(host: &str, port: u16) -> String {
    let host = host.trim();
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    match bare.parse::<std::net::IpAddr>() {
        _ if host.eq_ignore_ascii_case("localhost") => format!("localhost:{port}"),
        Ok(ip) if ip.is_loopback() => format!("localhost:{port}"),
        Ok(std::net::IpAddr::V6(v6)) => format!("[{v6}]:{port}"),
        _ => format!("{host}:{port}"),
    }
}

/// A `[radio.names]` key, `host:port`, in [`server_label`] form, lower case
/// for matching.
fn name_key_label(key: &str) -> Option<String> {
    let (host, port) = key.trim().rsplit_once(':')?;
    let port: u16 = port.parse().ok()?;
    (!host.is_empty()).then(|| server_label(host, port).to_ascii_lowercase())
}

impl RadioConfig {
    /// The key in `[radio.names]` for the server this connects to, as
    /// written in the file, if it has one.
    pub fn name_key(&self) -> Option<&String> {
        let here = server_label(&self.host, self.port).to_ascii_lowercase();
        self.names
            .keys()
            .find(|k| name_key_label(k).as_deref() == Some(here.as_str()))
    }

    /// The friendly name of the server this connects to, if it has one.
    pub fn server_name(&self) -> Option<&str> {
        self.name_key()
            .and_then(|k| self.names.get(k))
            .map(|n| n.trim())
            .filter(|n| !n.is_empty())
    }

    /// Give the server this connects to a name, or none (an empty `name`):
    /// the entry it already has is changed, else one is added as
    /// `host:port`.
    pub fn set_server_name(&mut self, name: &str) {
        let key = self
            .name_key()
            .cloned()
            .unwrap_or_else(|| format!("{}:{}", self.host.trim(), self.port));
        if name.trim().is_empty() {
            self.names.remove(&key);
        } else {
            self.names.insert(key, name.to_string());
        }
    }

    /// The login to use, if both a user and a password are set (the
    /// password from the environment first, then the file).
    pub fn credentials(&self) -> Option<(String, String)> {
        let user = self.user.as_deref().map(str::trim).filter(|u| !u.is_empty())?;
        let password = std::env::var(PASSWORD_ENV)
            .ok()
            .or_else(|| self.password.clone())
            .filter(|p| !p.is_empty())?;
        Some((user.to_string(), password))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Backend {
    Tci,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TcixMode {
    Off,
    Auto,
    On,
}

/// Pacing of dial and knob values (see `pacing` in `default_config.toml`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Pacing {
    /// Send every value at once (newest per target when several queue up).
    Off,
    /// Send at once while the server keeps up; one value in flight per
    /// target once its echoes fall behind.
    #[default]
    Auto,
    /// Always one value in flight per target, the next on the echo.
    On,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DisplayConfig {
    /// Percent, 100 = darkest.
    pub contrast: u8,
    pub backlight: [u8; 3],
    pub backlight_tx: [u8; 3],
    pub backlight_disconnected: [u8; 3],
    pub readout: Readout,
    pub flash_ms: u64,
    #[serde(default)]
    pub volume_scale: VolumeScale,
    /// What the bargraph shows while transmitting.
    #[serde(default)]
    pub tx_meter: TxMeter,
    /// What the small readout shows while transmitting.
    #[serde(default)]
    pub tx_readout: TxReadout,
    /// Power that fills the bargraph, in W.
    #[serde(default = "default_tx_power_max_w")]
    pub tx_power_max_w: u32,
}

fn default_tx_power_max_w() -> u32 {
    100
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TxMeter {
    /// Peak forward power, 0 to `tx_power_max_w`.
    #[default]
    Power,
    /// SWR: 1.8:1 at the S9 bar, 3:1 fills it.
    Swr,
    Off,
}

/// The desktop window (`[window]`). Its size and the mimic's height are
/// kept per machine by the window itself, not here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WindowConfig {
    pub start: WindowMode,
}

/// How the window opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowMode {
    /// The connection status and the LCD only.
    #[default]
    Mini,
    /// With all the settings.
    Full,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TxReadout {
    /// Peak power in W, with the W unit lit.
    #[default]
    Watts,
    /// SWR to one decimal, e.g. 1_2 for 1.2:1.
    Swr,
    Off,
}

/// Backlight presets (name, colour): the GUI's swatches and `backlight_next`.
pub const BACKLIGHT_PRESETS: [(&str, [u8; 3]); 8] = [
    ("White", [255, 255, 255]),
    ("Warm white", [255, 180, 90]),
    ("Amber", [255, 120, 0]),
    ("Red", [255, 0, 0]),
    ("Green", [0, 255, 0]),
    ("Cyan", [0, 255, 255]),
    ("Blue", [0, 0, 255]),
    ("Rich purple", [140, 0, 255]),
];

/// AGC modes TCI knows, for `agc:<mode>`.
pub const AGC_MODES: [&str; 6] = ["off", "long", "slow", "normal", "fast", "custom"];
/// Modes stepped through by `agc_next`.
pub const AGC_CYCLE: [&str; 4] = ["long", "slow", "normal", "fast"];

/// How the volume knobs (VOL, rx_vol, mon_vol) show and step the volume.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VolumeScale {
    /// 0-100, like Thetis's Master AF (1 per click).
    #[default]
    Percent,
    /// TCI's own -60..0 dB (1 dB per click).
    Db,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Readout {
    Dbm,
    SUnits,
    Off,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TuningConfig {
    pub steps_hz: Vec<u64>,
    pub step_hz: u64,
    pub acceleration: f32,
    pub counts_per_detent: i32,
    pub filter_step_hz: i32,
    pub rit_step_hz: i32,
    pub mode_cycle: Vec<String>,
    /// Step multiplier while `fast` is on.
    #[serde(default = "default_fast_multiplier")]
    pub fast_multiplier: u64,
}

fn default_fast_multiplier() -> u64 {
    10
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KnobConfig {
    pub functions: Vec<KnobFunction>,
    /// Function selected when the connector starts (default: the first).
    #[serde(default)]
    pub start: Option<KnobFunction>,
}

impl KnobConfig {
    /// Index in `functions` of the starting function.
    pub fn start_index(&self) -> usize {
        self.start
            .and_then(|s| self.functions.iter().position(|f| *f == s))
            .unwrap_or(0)
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ButtonsConfig {
    pub hold_ms: u64,
    #[serde(default = "default_push_lockout_ms")]
    pub push_lockout_ms: u64,
    #[serde(default = "default_hold_repeat_ms")]
    pub hold_repeat_ms: u64,
    pub f1: ButtonAction,
    pub f2: ButtonAction,
    pub f3: ButtonAction,
    pub f4: ButtonAction,
    pub f5: ButtonAction,
    pub f6: ButtonAction,
    pub main_push: ButtonAction,
    pub e1_push: ButtonAction,
    pub e2_push: ButtonAction,
}

fn default_push_lockout_ms() -> u64 {
    300
}

fn default_hold_repeat_ms() -> u64 {
    500
}

impl ButtonsConfig {
    pub fn get_mut(&mut self, b: tmate2_hid::Button) -> &mut ButtonAction {
        use tmate2_hid::Button::*;
        match b {
            F1 => &mut self.f1,
            F2 => &mut self.f2,
            F3 => &mut self.f3,
            F4 => &mut self.f4,
            F5 => &mut self.f5,
            F6 => &mut self.f6,
            MainPush => &mut self.main_push,
            E1Push => &mut self.e1_push,
            E2Push => &mut self.e2_push,
        }
    }

    pub fn get(&self, b: tmate2_hid::Button) -> &ButtonAction {
        use tmate2_hid::Button::*;
        match b {
            F1 => &self.f1,
            F2 => &self.f2,
            F3 => &self.f3,
            F4 => &self.f4,
            F5 => &self.f5,
            F6 => &self.f6,
            MainPush => &self.main_push,
            E1Push => &self.e1_push,
            E2Push => &self.e2_push,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ButtonAction {
    #[serde(default)]
    pub tap: Option<Action>,
    #[serde(default)]
    pub hold: Option<Action>,
}

/// What a knob (E1 / E2) currently adjusts; names match the LCD labels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(try_from = "String")]
pub enum KnobFunction {
    Vol,
    Sql,
    Rfg,
    Drv,
    Nr,
    Nb,
    An,
    /// Filter width, symmetric about the passband centre. The LCD has no
    /// WIDTH label, so HIGH and LOW are lit together.
    Width,
    High,
    Low,
    Shift,
    Rit,
    Xit,
    // Functions with no LCD label of their own: only the value flashed on
    // the main display shows what the knob is doing.
    /// TX monitor volume.
    MonVol,
    /// CW keyer speed.
    CwSpeed,
    /// Drive used while tuning.
    TuneDrive,
    /// The controlled receiver's own volume (Thetis RX1 / RX2 AF).
    RxVol,
}

impl KnobFunction {
    pub const ALL: [KnobFunction; 17] = [
        KnobFunction::Vol,
        KnobFunction::Sql,
        KnobFunction::Rfg,
        KnobFunction::Drv,
        KnobFunction::Nr,
        KnobFunction::Nb,
        KnobFunction::An,
        KnobFunction::Width,
        KnobFunction::High,
        KnobFunction::Low,
        KnobFunction::Shift,
        KnobFunction::Rit,
        KnobFunction::Xit,
        KnobFunction::MonVol,
        KnobFunction::CwSpeed,
        KnobFunction::TuneDrive,
        KnobFunction::RxVol,
    ];

    /// Functions without a label on the LCD's bottom row.
    pub const UNLABELLED: [KnobFunction; 4] = [
        KnobFunction::MonVol,
        KnobFunction::CwSpeed,
        KnobFunction::TuneDrive,
        KnobFunction::RxVol,
    ];

    pub fn has_lcd_label(self) -> bool {
        !Self::UNLABELLED.contains(&self)
    }

    /// For a function without an LCD label: the name the small 3-digit
    /// readout holds while a knob rests on it, and whether the minus segment
    /// to its left is lit with it. MON is "-non" ("m" can't be drawn on
    /// seven segments).
    pub fn readout_name(self) -> Option<(&'static str, bool)> {
        match self {
            KnobFunction::RxVol => Some(("AF", false)),
            KnobFunction::MonVol => Some(("non", true)),
            KnobFunction::CwSpeed => Some(("SPd", false)),
            KnobFunction::TuneDrive => Some(("tdr", false)),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            KnobFunction::Vol => "vol",
            KnobFunction::Sql => "sql",
            KnobFunction::Rfg => "rfg",
            KnobFunction::Drv => "drv",
            KnobFunction::Nr => "nr",
            KnobFunction::Nb => "nb",
            KnobFunction::An => "an",
            KnobFunction::Width => "width",
            KnobFunction::High => "high",
            KnobFunction::Low => "low",
            KnobFunction::Shift => "shift",
            KnobFunction::Rit => "rit",
            KnobFunction::Xit => "xit",
            KnobFunction::MonVol => "mon_vol",
            KnobFunction::CwSpeed => "cw_speed",
            KnobFunction::TuneDrive => "tune_drive",
            KnobFunction::RxVol => "rx_vol",
        }
    }
}

impl TryFrom<String> for KnobFunction {
    type Error = String;
    fn try_from(s: String) -> Result<Self, String> {
        let t = s.trim().to_ascii_lowercase();
        KnobFunction::ALL
            .into_iter()
            .find(|f| f.name() == t)
            .ok_or_else(|| unknown("knob function", &s, KnobFunction::ALL.iter().map(|f| f.name())))
    }
}

/// A button action.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
pub enum Action {
    BandUp,
    BandDown,
    ModeNext,
    ModePrev,
    Mode(String),
    VfoToggle,
    VfoAToB,
    VfoBToA,
    VfoSwap,
    /// Main knob uses the step times `tuning.fast_multiplier` (on/off).
    Fast,
    /// Second receiver on/off.
    Rx2,
    /// Sub-receiver (second channel) of the controlled receiver on/off.
    SubRx,
    /// Knobs and display follow the other receiver (RX1 <-> RX2).
    KnobRx2,
    FilterDefault,
    FilterWider,
    FilterNarrower,
    Split,
    Rit,
    Xit,
    /// Clear the RIT offset.
    RitClear,
    /// Clear the XIT offset.
    XitClear,
    /// Clear both offsets.
    RitXitClear,
    Lock,
    Mute,
    Rx1Mute,
    Rx2Mute,
    Nr,
    Nb,
    Anf,
    Notch,
    Squelch,
    AgcNext,
    /// Set an AGC mode (one of [`AGC_MODES`]).
    Agc(String),
    Apf,
    Binaural,
    Monitor,
    Tune,
    /// Tune carrier only while the button is held.
    TuneHold,
    Mox,
    Ptt,
    Power,
    BacklightNext,
    StepNext,
    StepPrev,
    E1Next,
    E2Next,
    E1Toggle,
    E2Toggle,
    /// TCI-X control (§4.4): ask for it, or give it up if we hold it.
    Control,
    /// TCI-X `preamp` / `attenuator`: the next of the radio's settings.
    Preamp,
    Att,
    Raw(String),
    None,
}

const SIMPLE_ACTIONS: &[(&str, Action)] = &[
    ("band_up", Action::BandUp),
    ("band_down", Action::BandDown),
    ("mode_next", Action::ModeNext),
    ("mode_prev", Action::ModePrev),
    ("vfo_toggle", Action::VfoToggle),
    ("vfo_a_to_b", Action::VfoAToB),
    ("vfo_b_to_a", Action::VfoBToA),
    ("vfo_swap", Action::VfoSwap),
    ("fast", Action::Fast),
    ("rx2", Action::Rx2),
    ("sub_rx", Action::SubRx),
    ("knob_rx2", Action::KnobRx2),
    ("filter_default", Action::FilterDefault),
    ("filter_wider", Action::FilterWider),
    ("filter_narrower", Action::FilterNarrower),
    ("split", Action::Split),
    ("rit", Action::Rit),
    ("xit", Action::Xit),
    ("rit_clear", Action::RitClear),
    ("xit_clear", Action::XitClear),
    ("rit_xit_clear", Action::RitXitClear),
    ("lock", Action::Lock),
    ("mute", Action::Mute),
    ("rx1_mute", Action::Rx1Mute),
    ("rx2_mute", Action::Rx2Mute),
    ("nr", Action::Nr),
    ("nb", Action::Nb),
    ("anf", Action::Anf),
    ("notch", Action::Notch),
    ("squelch", Action::Squelch),
    ("agc_next", Action::AgcNext),
    ("apf", Action::Apf),
    ("binaural", Action::Binaural),
    ("monitor", Action::Monitor),
    ("tune", Action::Tune),
    ("tune_hold", Action::TuneHold),
    ("mox", Action::Mox),
    ("ptt", Action::Ptt),
    ("power", Action::Power),
    ("backlight_next", Action::BacklightNext),
    ("step_next", Action::StepNext),
    ("step_prev", Action::StepPrev),
    ("e1_next", Action::E1Next),
    ("e2_next", Action::E2Next),
    ("e1_toggle", Action::E1Toggle),
    ("e2_toggle", Action::E2Toggle),
    ("control", Action::Control),
    ("preamp", Action::Preamp),
    ("att", Action::Att),
    ("none", Action::None),
];

impl FromStr for Action {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        let t = s.trim();
        if let Some(raw) = t.strip_prefix("raw:") {
            return Ok(Action::Raw(raw.to_string()));
        }
        if let Some(mode) = t.strip_prefix("mode:") {
            return Ok(Action::Mode(mode.trim().to_ascii_uppercase()));
        }
        if let Some(agc) = t.strip_prefix("agc:") {
            let agc = agc.trim().to_ascii_lowercase();
            return if AGC_MODES.contains(&agc.as_str()) {
                Ok(Action::Agc(agc))
            } else {
                Err(unknown("AGC mode", &agc, AGC_MODES.into_iter()))
            };
        }
        let lower = t.to_ascii_lowercase();
        SIMPLE_ACTIONS
            .iter()
            .find(|(n, _)| *n == lower)
            .map(|(_, a)| a.clone())
            .ok_or_else(|| {
                unknown(
                    "action",
                    t,
                    SIMPLE_ACTIONS
                        .iter()
                        .map(|(n, _)| *n)
                        .chain(["mode:USB", "agc:fast", "raw:..."]),
                )
            })
    }
}

impl TryFrom<String> for Action {
    type Error = String;
    fn try_from(s: String) -> Result<Self, String> {
        s.parse()
    }
}

impl Action {
    /// Every action that has a plain name, for pickers in the GUI.
    pub fn simple() -> impl Iterator<Item = &'static Action> {
        SIMPLE_ACTIONS.iter().map(|(_, a)| a)
    }

    /// Stepping / cycling actions that auto-repeat while a button is held.
    /// On/off toggles never repeat.
    pub fn repeats(&self) -> bool {
        matches!(
            self,
            Action::StepNext
                | Action::StepPrev
                | Action::E1Next
                | Action::E2Next
                | Action::ModeNext
                | Action::ModePrev
                | Action::BandUp
                | Action::BandDown
                | Action::FilterWider
                | Action::FilterNarrower
                | Action::AgcNext
                | Action::BacklightNext
        )
    }
}

impl fmt::Display for Action {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Action::Mode(m) => write!(f, "mode:{m}"),
            Action::Agc(m) => write!(f, "agc:{m}"),
            Action::Raw(r) => write!(f, "raw:{r}"),
            a => {
                let name = SIMPLE_ACTIONS
                    .iter()
                    .find(|(_, x)| x == a)
                    .map(|(n, _)| *n)
                    .unwrap_or("?");
                f.write_str(name)
            }
        }
    }
}

/// "unknown action 'bandup' - did you mean 'band_up'?"
fn unknown<'a>(what: &str, got: &str, valid: impl Iterator<Item = &'a str>) -> String {
    let valid: Vec<&str> = valid.collect();
    let best = valid
        .iter()
        .map(|v| {
            (
                strsim::jaro_winkler(&got.to_ascii_lowercase(), &v.to_ascii_lowercase()),
                *v,
            )
        })
        .max_by(|a, b| a.0.total_cmp(&b.0));
    match best {
        Some((score, v)) if score > 0.8 => format!("unknown {what} '{got}' - did you mean '{v}'?"),
        _ => format!("unknown {what} '{got}'; valid: {}", valid.join(", ")),
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("{0}")]
    Parse(String),
    #[error("{0}")]
    Invalid(String),
}

impl Config {
    pub fn parse(text: &str) -> Result<Config, ConfigError> {
        let mut c: Config = toml::from_str(text).map_err(|e| ConfigError::Parse(e.to_string()))?;
        // "none" is the same as leaving the gesture out: in particular a
        // button whose hold is "none" responds as soon as it is pressed.
        for b in tmate2_hid::Button::ALL {
            let a = c.buttons.get_mut(b);
            a.tap = a.tap.take().filter(|x| *x != Action::None);
            a.hold = a.hold.take().filter(|x| *x != Action::None);
        }
        c.validate()?;
        Ok(c)
    }

    fn validate(&self) -> Result<(), ConfigError> {
        let bad = |m: String| Err(ConfigError::Invalid(m));
        let t = &self.tuning;
        if t.steps_hz.is_empty() || t.steps_hz.contains(&0) {
            return bad("tuning.steps_hz must be a non-empty list of positive values".into());
        }
        if !t.steps_hz.contains(&t.step_hz) {
            return bad(format!(
                "tuning.step_hz = {} is not one of tuning.steps_hz",
                t.step_hz
            ));
        }
        let p = &self.radio.pacing_tuning;
        if p.off_below_ms >= p.on_above_ms {
            return bad("radio.pacing_tuning: off_below_ms must be less than on_above_ms".into());
        }
        if p.min_interval_ms < 1 || p.echo_wait_ms < p.min_interval_ms {
            return bad(
                "radio.pacing_tuning: min_interval_ms must be at least 1, and echo_wait_ms at least min_interval_ms"
                    .into(),
            );
        }
        if !(p.smoothing > 0.0 && p.smoothing <= 1.0) {
            return bad("radio.pacing_tuning: smoothing must be above 0 and at most 1".into());
        }
        if let Some(k) = self.radio.names.keys().find(|k| name_key_label(k).is_none()) {
            return bad(format!(
                "radio.names: \"{k}\" should be host:port, e.g. \"127.0.0.1:50001\""
            ));
        }
        let poll = self.radio.poll_ms;
        if poll != 0 && poll < POLL_MIN_MS {
            return bad(format!("radio.poll_ms must be 0 (off) or at least {POLL_MIN_MS}"));
        }
        if self.display.tx_power_max_w < 1 {
            return bad("display.tx_power_max_w must be at least 1".into());
        }
        if t.fast_multiplier < 1 {
            return bad("tuning.fast_multiplier must be at least 1".into());
        }
        if t.counts_per_detent < 1 {
            return bad("tuning.counts_per_detent must be at least 1".into());
        }
        if self.e1.functions.is_empty() || self.e2.functions.is_empty() {
            return bad("e1.functions and e2.functions must each list at least one function".into());
        }
        for (mode, [lo, hi]) in &self.filters {
            if hi - lo < 50 {
                return bad(format!("filters.{mode}: high must be at least 50 Hz above low"));
            }
        }
        for (name, k) in [("e1", &self.e1), ("e2", &self.e2)] {
            if let Some(s) = k.start.filter(|s| !k.functions.contains(s)) {
                return bad(format!(
                    "{name}.start = \"{}\" is not in {name}.functions",
                    s.name()
                ));
            }
        }
        Ok(())
    }
}

impl Default for Config {
    fn default() -> Self {
        Config::parse(DEFAULT_TOML).expect("built-in default config is valid")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_parses() {
        let c = Config::default();
        assert_eq!(c.radio.port, 50001);
        assert_eq!(c.radio.tcix, TcixMode::Auto);
        assert_eq!(c.buttons.f1.tap, Some(Action::Mox));
        assert_eq!(c.buttons.f1.hold, None, "hold = \"none\" means no hold");
        assert_eq!(c.buttons.main_push.tap, Some(Action::VfoToggle));
        assert_eq!(c.e1.functions[0], KnobFunction::Vol);
        assert_eq!(c.radio.poll_ms, 1000);
        assert_eq!(c.window.start, WindowMode::Mini);
        // The tested pacing numbers are built in, not in the file.
        assert!(!DEFAULT_TOML.contains("[radio.pacing_tuning]"));
        assert_eq!(c.radio.pacing_tuning, PacingTuning::default());
    }

    #[test]
    fn servers_have_friendly_names() {
        let with = |names: &str| Config::parse(&format!("{DEFAULT_TOML}\n[radio.names]\n{names}\n"));
        let c = with("\"localhost:50001\" = \"Thetis\"\n\"G2-Pi.local:40001\" = \"piHPSDR\"").unwrap();
        // 127.0.0.1 in [radio] finds the localhost entry.
        assert_eq!(c.radio.host, "127.0.0.1");
        assert_eq!(c.radio.server_name(), Some("Thetis"));
        let mut r = c.radio.clone();
        (r.host, r.port) = ("g2-pi.local".into(), 40001);
        assert_eq!(r.server_name(), Some("piHPSDR"));
        r.port = 40002;
        assert_eq!(r.server_name(), None);
        // Naming changes the entry the server has, or adds one.
        let mut r = c.radio.clone();
        r.set_server_name("Thetis (shack PC)");
        assert_eq!(
            r.names.get("localhost:50001").map(String::as_str),
            Some("Thetis (shack PC)")
        );
        r.set_server_name("");
        assert!(!r.names.contains_key("localhost:50001"));
        r.port = 40001;
        r.set_server_name("piHPSDR");
        assert_eq!(
            r.names.get("127.0.0.1:40001").map(String::as_str),
            Some("piHPSDR")
        );
        // A key that isn't host:port is refused.
        assert!(with("\"Thetis\" = \"Thetis\"").is_err());
    }

    #[test]
    fn server_labels() {
        assert_eq!(server_label("127.0.0.1", 50001), "localhost:50001");
        assert_eq!(server_label("::1", 40001), "localhost:40001");
        assert_eq!(server_label("LocalHost", 50001), "localhost:50001");
        assert_eq!(server_label("192.168.1.20", 40001), "192.168.1.20:40001");
        assert_eq!(server_label("G2-Pi.local", 40001), "G2-Pi.local:40001");
        assert_eq!(server_label("fe80::1", 40001), "[fe80::1]:40001");
    }

    #[test]
    fn pacing_tuning_can_be_added_as_an_override() {
        let with = |table: &str| Config::parse(&format!("{DEFAULT_TOML}\n[radio.pacing_tuning]\n{table}\n"));
        // Only the keys given change; the rest keep the tested values.
        let t = with("on_above_ms = 150\necho_wait_ms = 600")
            .unwrap()
            .radio
            .pacing_tuning;
        assert_eq!((t.on_above_ms, t.echo_wait_ms), (150, 600));
        assert_eq!(t.off_below_ms, PacingTuning::default().off_below_ms);
        // Values that can't work, and misspelt keys, are refused.
        assert!(with("off_below_ms = 90").is_err());
        assert!(with("smoothing = 0").is_err());
        assert!(with("echo_wait_ms = 10").is_err());
        assert!(with("on_above = 150").is_err());
    }

    #[test]
    fn poll_ms_is_off_or_at_least_the_minimum() {
        let with = |v: &str| DEFAULT_TOML.replace("poll_ms = 1000", &format!("poll_ms = {v}"));
        assert_eq!(Config::parse(&with("0")).unwrap().radio.poll_ms, 0);
        assert_eq!(Config::parse(&with("250")).unwrap().radio.poll_ms, 250);
        assert!(Config::parse(&with("50")).is_err());
        // An older file without the key polls every second.
        let old = DEFAULT_TOML.replace("poll_ms = 1000\n", "");
        assert_eq!(Config::parse(&old).unwrap().radio.poll_ms, 1000);
    }

    #[test]
    fn misspelt_action_suggests_and_names_the_line() {
        let text = DEFAULT_TOML.replace(r#"tap = "band_down""#, r#"tap = "banddown""#);
        let err = Config::parse(&text).unwrap_err().to_string();
        assert!(err.contains("did you mean 'band_down'"), "{err}");
        assert!(err.contains("line"), "{err}");
    }

    #[test]
    fn mode_and_raw_actions() {
        assert_eq!("mode:usb".parse::<Action>(), Ok(Action::Mode("USB".into())));
        assert_eq!(
            "raw:trx:0,true;".parse::<Action>(),
            Ok(Action::Raw("trx:0,true;".into()))
        );
        assert_eq!(Action::VfoAToB.to_string(), "vfo_a_to_b");
        assert_eq!("agc:Fast".parse::<Action>(), Ok(Action::Agc("fast".into())));
        assert_eq!(Action::Agc("slow".into()).to_string(), "agc:slow");
        assert!("agc:quick".parse::<Action>().is_err());
    }

    #[test]
    fn every_simple_action_and_knob_function_round_trips() {
        for a in Action::simple() {
            assert_eq!(a.to_string().parse::<Action>().as_ref(), Ok(a));
        }
        for f in KnobFunction::ALL {
            assert_eq!(KnobFunction::try_from(f.name().to_string()), Ok(f));
        }
    }

    #[test]
    fn filters_default_when_missing_from_file() {
        // Remove the whole [filters] block, as in a config written before it existed.
        let last = "NFM = [-5500, 5500]\n";
        let start = DEFAULT_TOML.find("\n# Default receive filter").unwrap();
        let end = DEFAULT_TOML.find(last).unwrap() + last.len();
        let text = format!("{}{}", &DEFAULT_TOML[..start], &DEFAULT_TOML[end..]);
        assert!(!text.contains("\n[filters]"));
        let c = Config::parse(&text).unwrap();
        assert_eq!(c.default_filter("usb"), Some((100, 2500)));
        assert_eq!(Config::default().default_filter("CWL"), Some((-850, -350)));
    }

    #[test]
    fn filters_missing_a_mode_fall_back_to_the_builtin() {
        // A file written before CW was added; its own values still win.
        let text = DEFAULT_TOML
            .replace("CW = [-250, 250]\n", "")
            .replace("USB = [100, 2500]\n", "USB = [200, 2700]\n");
        let c = Config::parse(&text).unwrap();
        assert!(!c.filters.contains_key("CW"));
        assert_eq!(c.default_filter("cw"), Some((-250, 250)));
        assert_eq!(c.default_filter("USB"), Some((200, 2700)));
        assert_eq!(c.default_filter("SPEC"), None);
    }

    #[test]
    fn step_must_be_in_list() {
        let text = DEFAULT_TOML.replace("step_hz = 10\n", "step_hz = 7\n");
        assert!(Config::parse(&text).is_err());
    }
}
