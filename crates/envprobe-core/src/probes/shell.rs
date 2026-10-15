//! Cible de /bin/sh et implémentation des outils de base.

use std::fs;
use std::path::Path;

use crate::util::{self, contains_bytes};
use crate::{Section, Value};

pub fn probe() -> Section {
    let mut s = Section::new("shell");
    s.push("sh", resolve("/bin/sh"));
    s.push("coreutils", coreutils());
    s
}

fn resolve(path: &str) -> Value {
    match fs::canonicalize(path) {
        Ok(real) => Value::ok(real.display().to_string()),
        Err(e) => util::io_value(&e, Path::new(path)),
    }
}

fn coreutils() -> Value {
    let ls = Path::new("/bin/ls");
    let real = match fs::canonicalize(ls) {
        Ok(p) => p,
        Err(e) => return util::io_value(&e, ls),
    };
    let name = real
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    match name.as_str() {
        "busybox" => return Value::ok(format!("busybox ({})", real.display())),
        "toybox" => return Value::ok(format!("toybox ({})", real.display())),
        _ => {}
    }
    let data = match fs::read(&real) {
        Ok(d) => d,
        Err(e) => return util::io_value(&e, &real),
    };
    let kind = if contains_bytes(&data, b"GNU coreutils") {
        "GNU coreutils"
    } else if contains_bytes(&data, b"uutils") {
        "uutils"
    } else if contains_bytes(&data, b"BusyBox") {
        "busybox"
    } else {
        "inconnu"
    };
    Value::ok(format!("{kind} ({})", real.display()))
}
