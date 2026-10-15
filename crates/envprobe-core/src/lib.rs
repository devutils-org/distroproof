//! Sondes d'environnement Linux.
//!
//! Chaque catégorie est un module de [`probes`] exposant `probe() -> Section`.
//! Une sonde ne plante jamais : une information absente ou refusée devient une
//! [`Value::Unavailable`] ou une [`Value::Denied`] avec sa raison.

pub mod diff;
pub mod parse;
pub mod probes;
mod sys;
mod util;

use std::fmt;
use std::panic;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Version du format du rapport JSON. À incrémenter à chaque changement
/// incompatible (clé renommée, statut ajouté…).
pub const SCHEMA_VERSION: u32 = 1;

/// Résultat d'une sonde élémentaire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "status", content = "value", rename_all = "snake_case")]
pub enum Value {
    /// Valeur obtenue.
    Ok(String),
    /// Information absente ou impossible à obtenir (fichier absent, syscall inexistant…) ; contient la raison.
    Unavailable(String),
    /// Accès refusé (EPERM, EACCES, seccomp…) ; contient l'errno et le contexte.
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
            Value::Unavailable(reason) => write!(f, "indisponible ({reason})"),
            Value::Denied(reason) => write!(f, "refusé ({reason})"),
        }
    }
}

/// Une information du rapport.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Entry {
    pub key: String,
    #[serde(flatten)]
    pub value: Value,
    /// Le résultat change selon les privilèges du processus (root, capabilities).
    pub privileged: bool,
}

/// Une catégorie du rapport, par exemple « kernel » ou « bpf ».
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

/// Rapport complet, tel qu'écrit par `envprobe --json`.
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

/// Noms des sections, dans l'ordre du rapport.
pub fn section_names() -> impl Iterator<Item = &'static str> {
    probes::ALL.iter().map(|(name, _)| *name)
}

/// Exécute les sondes demandées (toutes si `only` est vide). Une sonde qui
/// panique est remplacée par une section contenant l'erreur, les autres continuent.
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
                    .unwrap_or_else(|| "panique".to_string());
                let mut section = Section::new(*name);
                section.push(
                    "error",
                    Value::unavailable(format!("sonde en échec : {msg}")),
                );
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

/// Exécute toutes les sondes.
pub fn probe_all() -> Vec<Section> {
    probe(&[]).sections
}

/// Schéma JSON du rapport.
pub fn json_schema() -> serde_json::Value {
    serde_json::to_value(schemars::schema_for!(Report)).expect("schéma sérialisable")
}
