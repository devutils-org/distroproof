//! Distro, architecture, virtualization, container.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use crate::parse::os_release;
use crate::{sys, util, Section, Value};

const OS_RELEASE: &[&str] = &["/etc/os-release", "/usr/lib/os-release"];

pub fn probe() -> Section {
    let mut s = Section::new("identity");
    let fields = [
        ("distro", "PRETTY_NAME"),
        ("id", "ID"),
        ("version_id", "VERSION_ID"),
        ("id_like", "ID_LIKE"),
    ];
    match read_os_release() {
        Ok((path, map)) => {
            s.push("os_release", Value::ok(path));
            for (key, field) in fields {
                let value = map
                    .get(field)
                    .map(|v| Value::ok(v.as_str()))
                    .unwrap_or_else(|| Value::unavailable(format!("{field} not set")));
                s.push(key, value);
            }
        }
        Err(err) => {
            s.push("os_release", err.clone());
            for (key, _) in fields {
                s.push(key, err.clone());
            }
        }
    }
    s.push(
        "arch",
        match sys::uname() {
            Ok(u) => Value::ok(u.machine),
            Err(e) => Value::unavailable(format!("uname: {e}")),
        },
    );
    s.push("virtualization", detect_virt());
    s.push("container", detect_container());
    s
}

fn read_os_release() -> Result<(&'static str, BTreeMap<String, String>), Value> {
    let mut last = Value::unavailable("os-release missing");
    for path in OS_RELEASE {
        match fs::read_to_string(path) {
            Ok(text) => return Ok((path, os_release::parse(&text))),
            Err(e) => last = util::io_value(&e, Path::new(path)),
        }
    }
    Err(last)
}

fn detect_virt() -> Value {
    let dmi = ["sys_vendor", "product_name", "bios_vendor", "board_vendor"]
        .iter()
        .filter_map(|f| fs::read_to_string(format!("/sys/class/dmi/id/{f}")).ok())
        .map(|v| v.trim().to_lowercase())
        .collect::<Vec<_>>()
        .join(" ");
    const DMI: &[(&str, &str)] = &[
        ("qemu", "qemu"),
        ("kvm", "kvm"),
        ("vmware", "vmware"),
        ("virtualbox", "virtualbox"),
        ("innotek", "virtualbox"),
        ("xen", "xen"),
        ("amazon ec2", "amazon ec2"),
        ("google compute engine", "google compute engine"),
        ("parallels", "parallels"),
        ("bochs", "bochs"),
        ("apple virtualization", "apple virtualization"),
        ("microsoft corporation virtual machine", "hyper-v"),
    ];
    if let Some((_, name)) = DMI.iter().find(|(pat, _)| dmi.contains(pat)) {
        return Value::ok(format!("{name} (DMI)"));
    }
    if let Ok(kind) = util::read_trimmed("/sys/hypervisor/type") {
        if !kind.is_empty() {
            return Value::ok(format!("{kind} (/sys/hypervisor)"));
        }
    }
    if let Ok(compat) = fs::read("/proc/device-tree/compatible") {
        let compat = String::from_utf8_lossy(&compat).replace('\0', " ");
        let compat = compat.trim();
        if compat.contains("virt") {
            return Value::ok(format!("{compat} (device tree)"));
        }
    }
    if let Ok(cpuinfo) = fs::read_to_string("/proc/cpuinfo") {
        let flagged = cpuinfo
            .lines()
            .filter(|l| l.starts_with("flags"))
            .any(|l| l.split_whitespace().any(|f| f == "hypervisor"));
        if flagged {
            return Value::ok("unknown hypervisor (cpuinfo hypervisor flag)");
        }
    }
    Value::ok("none detected")
}

fn detect_container() -> Value {
    if Path::new("/.dockerenv").exists() {
        return Value::ok("docker (/.dockerenv)");
    }
    if Path::new("/run/.containerenv").exists() {
        return Value::ok("podman (/run/.containerenv)");
    }
    if let Ok(kind) = util::read_trimmed("/run/systemd/container") {
        if !kind.is_empty() {
            return Value::ok(format!("{kind} (/run/systemd/container)"));
        }
    }
    if let Ok(cgroup) = fs::read_to_string("/proc/1/cgroup") {
        const PATTERNS: &[(&str, &str)] = &[
            ("docker", "docker"),
            ("kubepods", "kubernetes"),
            ("libpod", "podman"),
            ("containerd", "containerd"),
            ("lxc", "lxc"),
        ];
        if let Some((_, name)) = PATTERNS.iter().find(|(pat, _)| cgroup.contains(pat)) {
            return Value::ok(format!("{name} (/proc/1/cgroup)"));
        }
    }
    if let Ok(kind) = std::env::var("container") {
        return Value::ok(format!("{kind} (container variable)"));
    }
    Value::ok("none detected")
}
