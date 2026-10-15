//! Arborescence : merged-usr, /lib64, multiarch Debian.

use std::fs;
use std::path::Path;

use crate::{util, Section, Value};

pub fn probe() -> Section {
    let mut s = Section::new("fhs");
    let mut merged = 0;
    for dir in ["/bin", "/sbin", "/lib"] {
        let value = match fs::read_link(dir) {
            Ok(target) => {
                let target = target.display().to_string();
                if target.trim_start_matches('/').starts_with("usr/") {
                    merged += 1;
                }
                Value::ok(format!("→ {target}"))
            }
            Err(_) if Path::new(dir).is_dir() => Value::ok("répertoire"),
            Err(e) => util::io_value(&e, Path::new(dir)),
        };
        s.push(dir, value);
    }
    s.push(
        "merged_usr",
        Value::ok(match merged {
            3 => "oui",
            0 => "non",
            _ => "partiel",
        }),
    );
    s.push(
        "/lib64",
        match fs::read_link("/lib64") {
            Ok(target) => Value::ok(format!("→ {}", target.display())),
            Err(_) if Path::new("/lib64").is_dir() => Value::ok("répertoire"),
            Err(_) => Value::ok("absent"),
        },
    );
    let multiarch = format!("/usr/lib/{}", util::multiarch_triplet());
    s.push(
        "multiarch",
        Value::ok(if Path::new(&multiarch).is_dir() {
            format!("oui ({multiarch})")
        } else {
            "non".to_string()
        }),
    );
    let lib_dirs: Vec<String> = [
        "/lib".to_string(),
        "/lib64".to_string(),
        "/usr/lib".to_string(),
        "/usr/lib64".to_string(),
        multiarch,
        "/usr/local/lib".to_string(),
    ]
    .into_iter()
    .filter(|d| Path::new(d).is_dir())
    .collect();
    s.push("lib_dirs", Value::ok(lib_dirs.join(", ")));
    s
}
