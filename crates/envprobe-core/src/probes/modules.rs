//! Modules noyau chargés et possibilité d'en charger.

use std::fs;
use std::path::Path;

use crate::probes::privileges::{self, CAP_SYS_MODULE};
use crate::{util, Section, Value};

pub fn probe() -> Section {
    let mut s = Section::new("modules");
    match fs::read_to_string("/proc/modules") {
        Ok(text) => {
            let names: Vec<&str> = text
                .lines()
                .filter_map(|l| l.split_whitespace().next())
                .collect();
            s.push("loaded_count", Value::ok(names.len().to_string()));
            s.push(
                "loaded",
                Value::ok(if names.is_empty() {
                    "(aucun)".to_string()
                } else {
                    names.join(", ")
                }),
            );
        }
        Err(e) => {
            let v = util::io_value(&e, Path::new("/proc/modules"));
            s.push("loaded_count", v.clone());
            s.push("loaded", v);
        }
    }
    s.push_privileged("loadable", loadable());
    s
}

fn loadable() -> Value {
    if !Path::new("/proc/modules").exists() {
        return Value::ok("non (noyau sans support des modules)");
    }
    if util::read_trimmed("/proc/sys/kernel/modules_disabled").as_deref() == Ok("1") {
        return Value::ok("non (kernel.modules_disabled=1)");
    }
    match privileges::cap_eff() {
        Some(caps) if privileges::has_cap(caps, CAP_SYS_MODULE) => {
            Value::ok("oui (CAP_SYS_MODULE)")
        }
        Some(_) => Value::ok("non (CAP_SYS_MODULE absente)"),
        None => Value::unavailable("capabilities illisibles"),
    }
}
