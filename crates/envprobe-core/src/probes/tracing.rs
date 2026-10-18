//! tracefs, debugfs, kallsyms readability.

use std::fs;
use std::io::{BufRead, BufReader};
use std::path::Path;

use crate::parse::mounts::{self, Mount};
use crate::{util, Section, Value};

pub(crate) fn read_mounts() -> Result<Vec<Mount>, Value> {
    let path = Path::new("/proc/mounts");
    fs::read_to_string(path)
        .map(|t| mounts::parse(&t))
        .map_err(|e| util::io_value(&e, path))
}

pub fn probe() -> Section {
    let mut s = Section::new("tracing");
    match read_mounts() {
        Ok(mounts) => {
            for fstype in ["tracefs", "debugfs"] {
                let targets: Vec<&str> = mounts
                    .iter()
                    .filter(|m| m.fstype == fstype)
                    .map(|m| m.target.as_str())
                    .collect();
                let value = if targets.is_empty() {
                    Value::ok("not mounted")
                } else {
                    Value::ok(targets.join(", "))
                };
                s.push(fstype, value);
            }
        }
        Err(err) => {
            s.push("tracefs", err.clone());
            s.push("debugfs", err);
        }
    }
    for (key, dir) in [
        ("tracing_dir", "/sys/kernel/tracing"),
        ("debug_tracing_dir", "/sys/kernel/debug/tracing"),
    ] {
        let value = match fs::read_dir(dir) {
            Ok(_) => Value::ok("readable"),
            Err(e) => util::io_value(&e, Path::new(dir)),
        };
        s.push_privileged(key, value);
    }
    s.push_privileged("kallsyms", kallsyms());
    s
}

fn kallsyms() -> Value {
    let path = Path::new("/proc/kallsyms");
    let file = match fs::File::open(path) {
        Ok(f) => f,
        Err(e) => return util::io_value(&e, path),
    };
    let mut line = String::new();
    if let Err(e) = BufReader::new(file).read_line(&mut line) {
        return util::io_value(&e, path);
    }
    let addr = line.split_whitespace().next().unwrap_or("");
    if addr.is_empty() {
        Value::ok("empty")
    } else if addr.bytes().all(|b| b == b'0') {
        Value::ok("readable, addresses hidden (kptr_restrict)")
    } else {
        Value::ok("readable, addresses visible")
    }
}
