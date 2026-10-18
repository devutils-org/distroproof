//! Kernel version, command line and configuration.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use crate::parse::kconfig;
use crate::{sys, util, Section, Value};

pub const CONFIG_KEYS: &[&str] = &[
    "CONFIG_BPF",
    "CONFIG_BPF_SYSCALL",
    "CONFIG_BPF_JIT",
    "CONFIG_BPF_EVENTS",
    "CONFIG_BPF_LSM",
    "CONFIG_KPROBES",
    "CONFIG_UPROBES",
    "CONFIG_FTRACE",
    "CONFIG_DEBUG_INFO_BTF",
    "CONFIG_PERF_EVENTS",
    "CONFIG_MODULES",
];

pub fn probe() -> Section {
    let mut s = Section::new("kernel");
    let uname = sys::uname();
    match &uname {
        Ok(u) => {
            s.push("release", Value::ok(u.release.as_str()));
            s.push("version", Value::ok(u.version.as_str()));
        }
        Err(e) => {
            let v = Value::unavailable(format!("uname: {e}"));
            s.push("release", v.clone());
            s.push("version", v);
        }
    }
    s.push("cmdline", util::read_value("/proc/cmdline"));
    let release = uname.as_ref().ok().map(|u| u.release.as_str());
    match load_config(release) {
        Ok((source, config)) => {
            s.push("config_source", Value::ok(source));
            for key in CONFIG_KEYS {
                let value = config
                    .get(*key)
                    .map(|v| Value::ok(v.as_str()))
                    .unwrap_or_else(|| Value::ok("not in the config"));
                s.push(*key, value);
            }
        }
        Err(err) => {
            s.push("config_source", err.clone());
            for key in CONFIG_KEYS {
                s.push(*key, err.clone());
            }
        }
    }
    s
}

/// Configuration of the running kernel: `/proc/config.gz`, then the distro locations.
pub(crate) fn load_config(
    release: Option<&str>,
) -> Result<(String, BTreeMap<String, String>), Value> {
    let mut last = None;
    match fs::read("/proc/config.gz") {
        Ok(data) => match kconfig::parse_gz(&data) {
            Ok(map) => return Ok(("/proc/config.gz".to_string(), map)),
            Err(e) => {
                last = Some(Value::unavailable(format!(
                    "/proc/config.gz unreadable: {e}"
                )))
            }
        },
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            last = Some(util::io_value(&e, Path::new("/proc/config.gz")))
        }
        Err(_) => {}
    }
    if let Some(release) = release {
        for path in [
            format!("/boot/config-{release}"),
            format!("/lib/modules/{release}/config"),
        ] {
            match fs::read_to_string(&path) {
                Ok(text) => return Ok((path, kconfig::parse(&text))),
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                    last = Some(util::io_value(&e, Path::new(&path)))
                }
                Err(_) => {}
            }
        }
    }
    Err(last.unwrap_or_else(|| {
        Value::unavailable(
            "neither /proc/config.gz nor /boot/config-<release> nor /lib/modules/<release>/config",
        )
    }))
}

/// `LINUX_VERSION_CODE` derived from the release, required by kprobes on kernels < 5.0.
pub(crate) fn version_code(release: &str) -> u32 {
    let mut parts = release
        .split(|c: char| !c.is_ascii_digit())
        .map(|p| p.parse::<u32>().unwrap_or(0));
    let major = parts.next().unwrap_or(0);
    let minor = parts.next().unwrap_or(0);
    let patch = parts.next().unwrap_or(0).min(255);
    (major << 16) | (minor << 8) | patch
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_codes() {
        assert_eq!(
            version_code("4.18.0-553.el8_10.x86_64"),
            (4 << 16) | (18 << 8)
        );
        assert_eq!(version_code("6.1.112-1"), (6 << 16) | (1 << 8) | 112);
        assert_eq!(version_code("5.4.300"), (5 << 16) | (4 << 8) | 255);
    }
}
