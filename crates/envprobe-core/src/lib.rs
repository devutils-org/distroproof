//! Sondes d'environnement Linux.
//!
//! Chaque catégorie est un module de [`probes`] exposant `probe() -> Section`.
//! Une sonde ne plante jamais : une information absente ou refusée devient une
//! [`Value::Unavailable`] ou une [`Value::Denied`] avec sa raison.

pub mod parse;
pub mod probes;
mod sys;
mod util;

use std::fmt;
use std::panic;

/// Résultat d'une sonde élémentaire.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    /// Valeur obtenue.
    Ok(String),
    /// Information absente ou impossible à obtenir (fichier absent, syscall inexistant…).
    Unavailable(String),
    /// Accès refusé (EPERM, EACCES, seccomp…).
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub key: String,
    pub value: Value,
    /// Le résultat change selon les privilèges du processus (root, capabilities).
    pub privileged: bool,
}

/// Une catégorie du rapport, par exemple « kernel » ou « bpf ».
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    pub name: &'static str,
    pub entries: Vec<Entry>,
}

impl Section {
    pub fn new(name: &'static str) -> Self {
        Section {
            name,
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

/// Exécute toutes les sondes. Une sonde qui panique est remplacée par une
/// section contenant l'erreur, les autres continuent.
pub fn probe_all() -> Vec<Section> {
    probes::ALL
        .iter()
        .map(|(name, probe)| {
            panic::catch_unwind(probe).unwrap_or_else(|payload| {
                let msg = payload
                    .downcast_ref::<&str>()
                    .map(|s| s.to_string())
                    .or_else(|| payload.downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "panique".to_string());
                let mut section = Section::new(name);
                section.push(
                    "error",
                    Value::unavailable(format!("sonde en échec : {msg}")),
                );
                section
            })
        })
        .collect()
}
