//! The TCI-X capability manifest (TCI-X §4.2): what the server says it
//! supports, with each command's access and value type.
//!
//! A TCI-X server lists every command it supports, base TCI ones included,
//! so a command that isn't in the manifest isn't supported. `cap` lines can
//! arrive again after `cap_end`: the newest wins, and `cap:<name>,none;`
//! removes one.

use std::collections::BTreeMap;

/// Whether a command can be read, written or both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    Read,
    Write,
    ReadWrite,
}

impl Access {
    pub fn writable(self) -> bool {
        matches!(self, Access::Write | Access::ReadWrite)
    }
}

/// A capability's value type and its parameters.
#[derive(Debug, Clone, PartialEq)]
pub enum CapKind {
    Bool,
    Range {
        min: f64,
        max: f64,
        step: f64,
        unit: String,
    },
    Enum(Vec<String>),
    Action,
    Meter {
        unit: String,
        min: f64,
        max: f64,
        red_from: Option<f64>,
    },
    Info(Vec<String>),
    /// A type this client doesn't know yet (kept so the command still counts
    /// as supported).
    Other(String, Vec<String>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Cap {
    pub access: Access,
    pub kind: CapKind,
}

/// What one `cap` line asks for.
#[derive(Debug, Clone, PartialEq)]
pub enum CapLine {
    Set(String, Cap),
    Remove(String),
}

/// Parse the arguments of a `cap:` line (`<command>,<access>,<type>[,<params>…]`
/// or `<command>,none`). Names are lower-cased; enum values keep their case.
pub fn parse_cap(args: &[String]) -> Option<CapLine> {
    let name = args.first()?.trim().to_ascii_lowercase();
    if name.is_empty() {
        return None;
    }
    let access = args.get(1)?.trim().to_ascii_lowercase();
    if access == "none" {
        return Some(CapLine::Remove(name));
    }
    let access = match access.as_str() {
        "r" => Access::Read,
        "w" => Access::Write,
        "rw" | "wr" => Access::ReadWrite,
        _ => return None,
    };
    let ty = args
        .get(2)
        .map(|t| t.trim().to_ascii_lowercase())
        .unwrap_or_default();
    let p: Vec<String> = args.iter().skip(3).map(|s| s.trim().to_string()).collect();
    let num = |i: usize| p.get(i).and_then(|s| s.parse::<f64>().ok());
    let kind = match ty.as_str() {
        "bool" => CapKind::Bool,
        "action" => CapKind::Action,
        "enum" => CapKind::Enum(p),
        "info" => CapKind::Info(p),
        "range" => match (num(0), num(1)) {
            (Some(min), Some(max)) => CapKind::Range {
                min,
                max,
                step: num(2).unwrap_or(1.0),
                unit: p.get(3).cloned().unwrap_or_default(),
            },
            _ => CapKind::Other(ty, p),
        },
        "meter" => match (num(1), num(2)) {
            (Some(min), Some(max)) => CapKind::Meter {
                unit: p.first().cloned().unwrap_or_default(),
                min,
                max,
                red_from: num(3),
            },
            _ => CapKind::Other(ty, p),
        },
        _ => CapKind::Other(ty, p),
    };
    Some(CapLine::Set(name, Cap { access, kind }))
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Manifest {
    caps: BTreeMap<String, Cap>,
}

impl Manifest {
    /// Apply one `cap` line's arguments. Returns false if it couldn't be parsed.
    pub fn apply(&mut self, args: &[String]) -> bool {
        match parse_cap(args) {
            Some(CapLine::Set(name, cap)) => {
                self.caps.insert(name, cap);
                true
            }
            Some(CapLine::Remove(name)) => {
                self.caps.remove(&name);
                true
            }
            None => false,
        }
    }

    pub fn get(&self, name: &str) -> Option<&Cap> {
        self.caps.get(&name.to_ascii_lowercase())
    }

    pub fn has(&self, name: &str) -> bool {
        self.get(name).is_some()
    }

    /// The server accepts this command from clients.
    pub fn writable(&self, name: &str) -> bool {
        self.get(name).is_some_and(|c| c.access.writable())
    }

    /// The values of an enum capability, in the server's order.
    pub fn enum_values(&self, name: &str) -> Option<&[String]> {
        match &self.get(name)?.kind {
            CapKind::Enum(v) => Some(v),
            _ => None,
        }
    }

    pub fn len(&self) -> usize {
        self.caps.len()
    }

    pub fn is_empty(&self) -> bool {
        self.caps.is_empty()
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.caps.keys().map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Vec<String> {
        s.split(',').map(str::to_string).collect()
    }

    #[test]
    fn parses_each_type() {
        let mut m = Manifest::default();
        for line in [
            "trx,rw,bool",
            "drive,rw,range,0,100,1,pct",
            "filter,rw,enum,fil1,fil2,fil3",
            "vfo_swap,w,action",
            "meter.alc,r,meter,pct,0,100,100",
            "rated_power,r,info,hf,100,6m,100",
            "band,w,enum,160m,80m",
        ] {
            assert!(m.apply(&args(line)), "{line}");
        }
        assert_eq!(m.len(), 7);
        assert_eq!(m.get("trx").unwrap().kind, CapKind::Bool);
        assert_eq!(
            m.get("drive").unwrap().kind,
            CapKind::Range {
                min: 0.0,
                max: 100.0,
                step: 1.0,
                unit: "pct".into()
            }
        );
        assert_eq!(m.enum_values("filter").unwrap(), ["fil1", "fil2", "fil3"]);
        assert!(m.writable("band") && m.writable("vfo_swap"));
        assert!(!m.writable("meter.alc") && m.has("meter.alc"));
        assert_eq!(
            m.get("meter.alc").unwrap().kind,
            CapKind::Meter {
                unit: "pct".into(),
                min: 0.0,
                max: 100.0,
                red_from: Some(100.0)
            }
        );
        assert!(!m.has("lock"));
    }

    #[test]
    fn newest_line_wins_and_none_removes() {
        let mut m = Manifest::default();
        m.apply(&args("rf_gain,rw,range,0,100,1,pct"));
        m.apply(&args("rf_gain,r,range,0,100,1,pct"));
        assert!(m.has("rf_gain") && !m.writable("rf_gain"));
        m.apply(&args("RF_GAIN,none"));
        assert!(!m.has("rf_gain"));
        assert!(!m.apply(&args("broken")));
    }
}
