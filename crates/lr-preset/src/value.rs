//! A preset's raw settings, as both readers produce them.

use std::collections::BTreeMap;

/// One setting's value, as a Lightroom file writes it.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Num(f64),
    Text(String),
    Bool(bool),
    List(Vec<Value>),
}

impl Value {
    /// The number, if this is one.
    pub fn num(&self) -> Option<f64> {
        match self {
            Value::Num(n) => Some(*n),
            _ => None,
        }
    }

    /// The text, if this is text.
    pub fn text(&self) -> Option<&str> {
        match self {
            Value::Text(t) => Some(t),
            _ => None,
        }
    }

    /// True for `true` and for the `"True"` an `.xmp` writes.
    pub fn as_bool(&self) -> bool {
        match self {
            Value::Bool(b) => *b,
            Value::Text(t) => t.eq_ignore_ascii_case("true"),
            Value::Num(n) => *n != 0.0,
            Value::List(_) => false,
        }
    }

    /// The numbers of a list — a tone curve's flattened `x, y` pairs.
    pub fn numbers(&self) -> Vec<f64> {
        match self {
            Value::List(v) => v.iter().filter_map(Value::num).collect(),
            _ => Vec::new(),
        }
    }
}

/// Every setting a preset carries, by its Lightroom name (`Exposure2012`, `ToneCurvePV2012`).
pub type Settings = BTreeMap<String, Value>;

/// A preset as read, before mapping: its name and group when the file says them, and its settings.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Raw {
    pub name: Option<String>,
    pub group: Option<String>,
    pub settings: Settings,
}

/// Why a file was not read as a preset.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum Error {
    /// A file that reads fine but is something else — a profile, a photo's sidecar.
    #[error("not a develop preset: {0}")]
    NotAPreset(&'static str),
    #[error("malformed: {0}")]
    Malformed(String),
}
