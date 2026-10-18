//! Linux environment probes.
//!
//! Each category is a [`probes`] module exposing `probe() -> Section`.
//! A probe never crashes: missing or denied information becomes a
//! [`Value::Unavailable`] or a [`Value::Denied`] with its reason.

pub mod diff;
pub mod parse;
pub mod probes;
mod sys;
mod util;

use std::fmt;
use std::panic;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Version of the JSON report format. Bump it on every incompatible change
/// (renamed key, added status…).
pub const SCHEMA_VERSION: u32 = 1;

/// Result of an elementary probe.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "status", content = "value", rename_all = "snake_case")]
pub enum Value {
    /// Value obtained.
    Ok(String),
    /// Information missing or impossible to obtain (missing file, nonexistent syscall…); holds the reason.
    Unavailable(String),
    /// Access denied (EPERM, EACCES, seccomp…); holds the errno and the context.
    Denied(String),
}

impl Value {
    pub fn ok(value: impl Into<String>) -> Self {
        Value::Ok(value.into())
    }

    pub fn unavailable(reason: impl Into<String>) -> Self {
        Value::Unavailable(reason.into())
    }

    pub fn denied(reason: impl Into<String>) -> Self {
        Value::Denied(reason.into())
    }

    pub fn as_ok(&self) -> Option<&str> {
        match self {
            Value::Ok(v) => Some(v),
            _ => None,
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Ok(v) => f.write_str(v),
            Value::Unavailable(reason) => write!(f, "unavailable ({reason})"),
            Value::Denied(reason) => write!(f, "denied ({reason})"),
        }
    }
}

/// One piece of information in the report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Entry {
    pub key: String,
    #[serde(flatten)]
    pub value: Value,
    /// The result depends on the process's privileges (root, capabilities).
    pub privileged: bool,
}

/// A report category, for example "kernel" or "bpf".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Section {
    pub name: String,
    pub entries: Vec<Entry>,
}

impl Section {
    pub fn new(name: impl Into<String>) -> Self {
        Section {
            name: name.into(),
            entries: Vec::new(),
        }
    }

    pub fn push(&mut self, key: impl Into<String>, value: Value) {
        self.entries.push(Entry {
            key: key.into(),
            value,
            privileged: false,
        });
    }

    pub fn push_privileged(&mut self, key: impl Into<String>, value: Value) {
        self.entries.push(Entry {
            key: key.into(),
            value,
            privileged: true,
        });
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        self.entries.iter().find(|e| e.key == key).map(|e| &e.value)
    }
}

/// Full report, as written by `envprobe --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Report {
    pub schema_version: u32,
    pub envprobe_version: String,
    pub sections: Vec<Section>,
}

impl Report {
    pub fn section(&self, name: &str) -> Option<&Section> {
        self.sections.iter().find(|s| s.name == name)
    }
}

/// Section names, in report order.
pub fn section_names() -> impl Iterator<Item = &'static str> {
    probes::ALL.iter().map(|(name, _)| *name)
}

/// Runs the requested probes (all of them if `only` is empty). A probe that
/// panics is replaced by a section holding the error; the others go on.
pub fn probe(only: &[&str]) -> Report {
    let sections = probes::ALL
        .iter()
        .filter(|(name, _)| only.is_empty() || only.contains(name))
        .map(|(name, probe)| {
            panic::catch_unwind(probe).unwrap_or_else(|payload| {
                let msg = payload
                    .downcast_ref::<&str>()
                    .map(|s| s.to_string())
                    .or_else(|| payload.downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "panic".to_string());
                let mut section = Section::new(*name);
                section.push("error", Value::unavailable(format!("probe failed: {msg}")));
                section
            })
        })
        .collect();
    Report {
        schema_version: SCHEMA_VERSION,
        envprobe_version: env!("CARGO_PKG_VERSION").to_string(),
        sections,
    }
}

/// Runs every probe.
pub fn probe_all() -> Vec<Section> {
    probe(&[]).sections
}

/// JSON schema of the report.
pub fn json_schema() -> serde_json::Value {
    serde_json::to_value(schemars::schema_for!(Report)).expect("serializable schema")
}
