//! Package managers present.

use std::env;
use std::path::{Path, PathBuf};

use crate::{Section, Value};

const MANAGERS: &[&str] = &[
    "apt",
    "dpkg",
    "dnf",
    "yum",
    "rpm",
    "pacman",
    "zypper",
    "apk",
    "xbps-install",
    "emerge",
    "nix",
];

const EXTRA_DIRS: &[&str] = &[
    "/usr/local/sbin",
    "/usr/local/bin",
    "/usr/sbin",
    "/usr/bin",
    "/sbin",
    "/bin",
    "/nix/var/nix/profiles/default/bin",
    "/run/current-system/sw/bin",
];

pub fn probe() -> Section {
    let mut s = Section::new("packages");
    let mut dirs: Vec<PathBuf> = env::var_os("PATH")
        .map(|p| env::split_paths(&p).collect())
        .unwrap_or_default();
    dirs.extend(EXTRA_DIRS.iter().map(PathBuf::from));
    let found: Vec<&str> = MANAGERS
        .iter()
        .copied()
        .filter(|m| dirs.iter().any(|d| is_file(&d.join(m))))
        .collect();
    s.push(
        "managers",
        Value::ok(if found.is_empty() {
            "none".to_string()
        } else {
            found.join(", ")
        }),
    );
    s
}

fn is_file(path: &Path) -> bool {
    path.metadata().map(|m| m.is_file()).unwrap_or(false)
}
