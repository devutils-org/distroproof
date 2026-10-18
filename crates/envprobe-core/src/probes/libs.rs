//! Presence and version of common libraries.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::parse::ldconfig;
use crate::{util, Section, Value};

/// (key, soname prefix)
const LIBS: &[(&str, &str)] = &[
    ("libbpf", "libbpf.so"),
    ("libelf", "libelf.so"),
    ("zlib", "libz.so"),
    ("libssl", "libssl.so"),
    ("libcrypto", "libcrypto.so"),
];

const LDCONFIG: &[&str] = &["/sbin/ldconfig", "/usr/sbin/ldconfig", "/usr/bin/ldconfig"];

pub fn probe() -> Section {
    let mut s = Section::new("libs");
    let (source, entries) = match ldconfig_entries() {
        Some(entries) => ("ldconfig -p", entries),
        None => ("directory scan", scan_dirs()),
    };
    s.push("source", Value::ok(source));
    for (key, prefix) in LIBS {
        let found = entries
            .iter()
            .find(|(soname, _)| soname.starts_with(&format!("{prefix}.")));
        let value = match found {
            Some((_, path)) => {
                let real = fs::canonicalize(path).unwrap_or_else(|_| PathBuf::from(path));
                Value::ok(real.display().to_string())
            }
            None => Value::ok("absent"),
        };
        s.push(*key, value);
    }
    s
}

/// Output of `ldconfig -p` (read-only cache lookup), if the tool exists and answers.
fn ldconfig_entries() -> Option<Vec<(String, String)>> {
    let bin = LDCONFIG.iter().find(|p| Path::new(p).exists())?;
    let out = Command::new(bin).arg("-p").output().ok()?;
    if !out.status.success() {
        return None;
    }
    let entries = ldconfig::parse(&String::from_utf8_lossy(&out.stdout));
    (!entries.is_empty()).then_some(entries)
}

fn scan_dirs() -> Vec<(String, String)> {
    let triplet = util::multiarch_triplet();
    let dirs = [
        format!("/lib/{triplet}"),
        format!("/usr/lib/{triplet}"),
        "/lib64".to_string(),
        "/usr/lib64".to_string(),
        "/lib".to_string(),
        "/usr/lib".to_string(),
        "/usr/local/lib".to_string(),
    ];
    let mut entries = Vec::new();
    for dir in dirs {
        let Ok(read) = fs::read_dir(&dir) else {
            continue;
        };
        let mut names: Vec<String> = read
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains(".so."))
            .collect();
        names.sort();
        for name in names {
            let path = Path::new(&dir).join(&name).display().to_string();
            entries.push((name, path));
        }
    }
    entries
}
