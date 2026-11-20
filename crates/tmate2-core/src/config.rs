//! Human-readable TOML configuration.

use std::fmt;
use std::str::FromStr;

use serde::Deserialize;

/// The commented default configuration; also the single source of defaults.
pub const DEFAULT_TOML: &str = include_str!("default_config.toml");

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub radio: RadioConfig,
    pub display: DisplayConfig,
    pub tuning: TuningConfig,
    pub e1: KnobConfig,
    pub e2: KnobConfig,
    pub buttons: ButtonsConfig,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RadioConfig {
    pub backend: Backend,
    pub host: String,
    pub port: u16,
    pub tcix: TcixMode,
    pub trx: usize,
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

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DisplayConfig {
    pub contrast: u8,
    pub backlight: [u8; 3],
    pub backlight_tx: [u8; 3],
    pub backlight_disconnected: [u8; 3],
    pub readout: Readout,
    pub flash_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Readout {
    Dbm,
    SUnits,
    Off,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TuningConfig {
    pub steps_hz: Vec<u64>,
    pub step_hz: u64,
    pub acceleration: f32,
    pub counts_per_detent: i32,
    pub filter_step_hz: i32,
    pub rit_step_hz: i32,
    pub mode_cycle: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KnobConfig {
    pub functions: Vec<KnobFunction>,
}

#[derive(Debug, Clone, Deserialize)]
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
    300
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

#[derive(Debug, Clone, Default, Deserialize)]
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
    High,
    Low,
    Shift,
    Rit,
    Xit,
}

impl KnobFunction {
    pub const ALL: [KnobFunction; 12] = [
        KnobFunction::Vol,
        KnobFunction::Sql,
        KnobFunction::Rfg,
        KnobFunction::Drv,
        KnobFunction::Nr,
        KnobFunction::Nb,
        KnobFunction::An,
        KnobFunction::High,
        KnobFunction::Low,
        KnobFunction::Shift,
        KnobFunction::Rit,
        KnobFunction::Xit,
    ];

    pub fn name(self) -> &'static str {
        match self {
            KnobFunction::Vol => "vol",
            KnobFunction::Sql => "sql",
            KnobFunction::Rfg => "rfg",
            KnobFunction::Drv => "drv",
            KnobFunction::Nr => "nr",
            KnobFunction::Nb => "nb",
            KnobFunction::An => "an",
            KnobFunction::High => "high",
            KnobFunction::Low => "low",
            KnobFunction::Shift => "shift",
            KnobFunction::Rit => "rit",
            KnobFunction::Xit => "xit",
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
    VfoSwap,
    Split,
    Rit,
    Xit,
    RitClear,
    Lock,
    Mute,
    Nr,
    Nb,
    Anf,
    Notch,
    Tune,
    Mox,
    Ptt,
    StepNext,
    StepPrev,
    E1Next,
    E2Next,
    E1Toggle,
    E2Toggle,
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
    ("vfo_swap", Action::VfoSwap),
    ("split", Action::Split),
    ("rit", Action::Rit),
    ("xit", Action::Xit),
    ("rit_clear", Action::RitClear),
    ("lock", Action::Lock),
    ("mute", Action::Mute),
    ("nr", Action::Nr),
    ("nb", Action::Nb),
    ("anf", Action::Anf),
    ("notch", Action::Notch),
    ("tune", Action::Tune),
    ("mox", Action::Mox),
    ("ptt", Action::Ptt),
    ("step_next", Action::StepNext),
    ("step_prev", Action::StepPrev),
    ("e1_next", Action::E1Next),
    ("e2_next", Action::E2Next),
    ("e1_toggle", Action::E1Toggle),
    ("e2_toggle", Action::E2Toggle),
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
                        .chain(["mode:USB", "raw:..."]),
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
        )
    }
}

impl fmt::Display for Action {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Action::Mode(m) => write!(f, "mode:{m}"),
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
        if t.counts_per_detent < 1 {
            return bad("tuning.counts_per_detent must be at least 1".into());
        }
        if self.e1.functions.is_empty() || self.e2.functions.is_empty() {
            return bad("e1.functions and e2.functions must each list at least one function".into());
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
    }

    #[test]
    fn step_must_be_in_list() {
        let text = DEFAULT_TOML.replace("step_hz = 10\n", "step_hz = 7\n");
        assert!(Config::parse(&text).is_err());
    }
}
