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
    match first_text_address(BufReader::new(file)) {
        Err(e) => util::io_value(&e, path),
        Ok(None) => Value::ok("empty"),
        Ok(Some(addr)) if addr.bytes().all(|b| b == b'0') => {
            Value::ok("readable, addresses hidden (kptr_restrict)")
        }
        Ok(Some(_)) => Value::ok("readable, addresses visible"),
    }
}

/// The address of the first text symbol (`t`, `T`, `w`, `W`), as written.
///
/// Not the first line's: x86-64 opens the table with its per-cpu symbols,
/// absolute and at offsets from zero (`0000000000000000 A fixed_percpu_data`),
/// whether or not `kptr_restrict` hides the rest. Read as the answer, a root
/// process on Debian 10 and Ubuntu 20.04 was reported as seeing no address.
/// No function is at address zero.
fn first_text_address(reader: impl BufRead) -> std::io::Result<Option<String>> {
    for line in reader.lines() {
        let line = line?;
        let mut fields = line.split_whitespace();
        let (Some(addr), Some(kind)) = (fields.next(), fields.next()) else {
            continue;
        };
        if matches!(kind, "t" | "T" | "w" | "W") {
            return Ok(Some(addr.to_owned()));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod kallsyms_tests {
    use super::first_text_address;

    #[test]
    fn the_table_is_judged_by_its_first_function_not_its_first_line() {
        let shown = "0000000000000000 A fixed_percpu_data\n\
                     ffffffffb3c00000 T startup_64\n";
        let hidden = "0000000000000000 A fixed_percpu_data\n\
                      0000000000000000 T startup_64\n";
        assert_eq!(
            first_text_address(shown.as_bytes()).unwrap().as_deref(),
            Some("ffffffffb3c00000")
        );
        assert_eq!(
            first_text_address(hidden.as_bytes()).unwrap().as_deref(),
            Some("0000000000000000")
        );
        assert_eq!(first_text_address("".as_bytes()).unwrap(), None);
    }
}
