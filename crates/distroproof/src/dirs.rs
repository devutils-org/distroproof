//! Répertoires XDG utilisés par distroproof.

use std::env;
use std::path::PathBuf;

fn xdg(var: &str, fallback: &str) -> PathBuf {
    match env::var_os(var) {
        Some(v) if !v.is_empty() => PathBuf::from(v),
        _ => {
            let home = env::var_os("HOME").map_or_else(|| PathBuf::from("/tmp"), PathBuf::from);
            home.join(fallback)
        }
    }
}

/// `$XDG_CACHE_HOME/distroproof`.
pub fn cache() -> PathBuf {
    xdg("XDG_CACHE_HOME", ".cache").join("distroproof")
}

pub fn images() -> PathBuf {
    cache().join("images")
}

/// Répertoires de travail des VM en cours (overlays, disques jetables).
pub fn runs() -> PathBuf {
    cache().join("runs")
}

/// Répertoire court pour les sockets unix (108 octets maximum par chemin).
pub fn sockets() -> PathBuf {
    match env::var_os("XDG_RUNTIME_DIR") {
        Some(v) if !v.is_empty() => PathBuf::from(v).join("distroproof"),
        _ => env::temp_dir().join(format!("distroproof-{}", crate::sys::uid())),
    }
}

/// `$XDG_CONFIG_HOME/distroproof`.
pub fn config() -> PathBuf {
    xdg("XDG_CONFIG_HOME", ".config").join("distroproof")
}
