//! System libc: glibc or musl, and its version.
//!
//! The probe itself is statically linked against musl, so
//! `gnu_get_libc_version()` is never available: we read the ELF interpreter
//! of `/bin/sh` instead.

use std::fs;
use std::path::Path;

use crate::parse::{elf, libc_version};
use crate::{util, Section, Value};

pub fn probe() -> Section {
    let mut s = Section::new("libc");
    let sh = Path::new("/bin/sh");
    let interp = match fs::read(sh) {
        Ok(data) => elf::interpreter(&data),
        Err(e) => {
            let v = util::io_value(&e, sh);
            for key in ["interpreter", "kind", "version"] {
                s.push(key, v.clone());
            }
            return s;
        }
    };
    let Some(interp) = interp else {
        s.push("interpreter", Value::ok("none (static /bin/sh)"));
        s.push(
            "kind",
            Value::unavailable("static /bin/sh, libc cannot be determined"),
        );
        s.push("version", Value::unavailable("static /bin/sh"));
        return s;
    };
    s.push("interpreter", Value::ok(interp.as_str()));
    if interp.contains("ld-musl") {
        s.push("kind", Value::ok("musl"));
        s.push(
            "version",
            match fs::read_to_string("/lib/apk/db/installed") {
                Ok(db) => libc_version::musl_version_from_apk(&db)
                    .map(Value::ok)
                    .unwrap_or_else(|| {
                        Value::unavailable("musl package missing from the apk database")
                    }),
                Err(_) => Value::unavailable("no apk database to read the musl version from"),
            },
        );
    } else if interp.contains("ld-linux") || interp.contains("ld64.so") {
        s.push("kind", Value::ok("glibc"));
        s.push("version", glibc_version(&interp));
    } else {
        s.push(
            "kind",
            Value::unavailable(format!("unknown interpreter: {interp}")),
        );
        s.push("version", Value::unavailable("unknown libc"));
    }
    s
}

fn glibc_version(interp: &str) -> Value {
    let triplet = util::multiarch_triplet();
    let mut candidates = Vec::new();
    if let Some(dir) = Path::new(interp).parent() {
        candidates.push(dir.join("libc.so.6"));
    }
    for dir in [
        format!("/lib/{triplet}"),
        format!("/usr/lib/{triplet}"),
        "/lib64".to_string(),
        "/usr/lib64".to_string(),
        "/lib".to_string(),
        "/usr/lib".to_string(),
    ] {
        candidates.push(Path::new(&dir).join("libc.so.6"));
    }
    for path in &candidates {
        if let Ok(data) = fs::read(path) {
            if let Some(v) = libc_version::glibc_version(&data) {
                return Value::ok(format!("{v} ({})", path.display()));
            }
        }
    }
    Value::unavailable("libc.so.6 not found or without a version string")
}
