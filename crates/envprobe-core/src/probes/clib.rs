//! libc du système : glibc ou musl, et sa version.
//!
//! La sonde est elle-même liée statiquement à musl, `gnu_get_libc_version()`
//! n'est donc jamais disponible : on lit l'interpréteur ELF de `/bin/sh`.

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
        s.push("interpreter", Value::ok("aucun (/bin/sh statique)"));
        s.push(
            "kind",
            Value::unavailable("/bin/sh statique, libc non déterminable"),
        );
        s.push("version", Value::unavailable("/bin/sh statique"));
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
                    .unwrap_or_else(|| Value::unavailable("paquet musl absent de la base apk")),
                Err(_) => Value::unavailable("pas de base apk pour lire la version de musl"),
            },
        );
    } else if interp.contains("ld-linux") || interp.contains("ld64.so") {
        s.push("kind", Value::ok("glibc"));
        s.push("version", glibc_version(&interp));
    } else {
        s.push(
            "kind",
            Value::unavailable(format!("interpréteur inconnu : {interp}")),
        );
        s.push("version", Value::unavailable("libc inconnue"));
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
    Value::unavailable("libc.so.6 introuvable ou sans chaîne de version")
}
