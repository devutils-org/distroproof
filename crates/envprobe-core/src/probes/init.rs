//! PID 1.

use std::fs;
use std::path::Path;

use crate::{util, Section, Value};

pub fn probe() -> Section {
    let mut s = Section::new("init");
    let comm = util::read_value("/proc/1/comm");
    s.push(
        "kind",
        comm.as_ok().map(classify).unwrap_or_else(|| comm.clone()),
    );
    s.push("comm", comm);
    s.push_privileged(
        "exe",
        match fs::read_link("/proc/1/exe") {
            Ok(p) => Value::ok(p.display().to_string()),
            Err(e) => util::io_value(&e, Path::new("/proc/1/exe")),
        },
    );
    s
}

fn classify(comm: &str) -> Value {
    let kind = match comm {
        "systemd" => "systemd",
        "openrc-init" => "openrc",
        "runit" | "runit-init" => "runit",
        "s6-svscan" => "s6",
        "dinit" => "dinit",
        "tini" | "docker-init" => "tini (conteneur)",
        "dumb-init" => "dumb-init (conteneur)",
        "init" => "init (sysvinit, busybox ou openrc)",
        _ => return Value::ok(format!("autre ({comm})")),
    };
    Value::ok(kind)
}
