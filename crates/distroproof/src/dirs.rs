//! XDG directories used by distroproof.

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

/// Working directories of running VMs (overlays, throwaway disks).
pub fn runs() -> PathBuf {
    cache().join("runs")
}

/// Short directory for unix sockets (108 bytes at most per path).
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
