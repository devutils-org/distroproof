//! The machine underneath: processor, memory, NUMA, graphics cards and disks,
//! read from /proc and /sys. Two reports from different machines differ here
//! first, which is what a measurement compared across them needs to know.

use std::collections::BTreeSet;
use std::fs;

use crate::{Section, Value};

pub fn probe() -> Section {
    let mut s = Section::new("hardware");
    let cpuinfo = fs::read_to_string("/proc/cpuinfo").unwrap_or_default();
    s.push("cpu_model", cpu_model(&cpuinfo));
    s.push("cpus", cpus());
    s.push("cores", cores());
    s.push("cpu_max_mhz", max_mhz());
    let meminfo = fs::read_to_string("/proc/meminfo").unwrap_or_default();
    s.push("memory", meminfo_size(&meminfo, "MemTotal:"));
    s.push("swap", meminfo_size(&meminfo, "SwapTotal:"));
    s.push("numa_nodes", numa_nodes());
    s.push("gpus", gpus());
    s.push("disks", disks());
    s
}

/// `model name` on x86, `Hardware` or the implementer/part pair on Arm.
pub fn cpu_model(cpuinfo: &str) -> Value {
    let field = |name: &str| {
        cpuinfo.lines().find_map(|l| {
            let (k, v) = l.split_once(':')?;
            (k.trim() == name).then(|| v.trim().to_string())
        })
    };
    if let Some(model) = field("model name").or_else(|| field("Hardware")) {
        return Value::ok(model);
    }
    match (field("CPU implementer"), field("CPU part")) {
        (Some(i), Some(p)) => Value::ok(format!("implementer {i}, part {p}")),
        _ => Value::unavailable("/proc/cpuinfo names no model"),
    }
}

/// Online logical CPUs, from `/sys/devices/system/cpu/online` (`0-23`).
fn cpus() -> Value {
    match fs::read_to_string("/sys/devices/system/cpu/online") {
        Ok(list) => Value::ok(count_cpu_list(list.trim()).to_string()),
        Err(_) => Value::unavailable("/sys/devices/system/cpu/online not readable"),
    }
}

/// `0-3,8,10-11` → 7.
pub fn count_cpu_list(list: &str) -> usize {
    list.split(',')
        .filter(|r| !r.is_empty())
        .map(|r| match r.split_once('-') {
            Some((a, b)) => match (a.parse::<usize>(), b.parse::<usize>()) {
                (Ok(a), Ok(b)) if b >= a => b - a + 1,
                _ => 0,
            },
            None => usize::from(r.parse::<usize>().is_ok()),
        })
        .sum()
}

/// Physical cores: distinct (package, core) pairs across the CPUs.
fn cores() -> Value {
    let Ok(dir) = fs::read_dir("/sys/devices/system/cpu") else {
        return Value::unavailable("/sys/devices/system/cpu not readable");
    };
    let mut pairs = BTreeSet::new();
    for entry in dir.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with("cpu") || !name[3..].chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let topo = entry.path().join("topology");
        let read = |f: &str| {
            fs::read_to_string(topo.join(f))
                .ok()
                .map(|v| v.trim().to_string())
        };
        if let (Some(p), Some(c)) = (read("physical_package_id"), read("core_id")) {
            pairs.insert((p, c));
        }
    }
    if pairs.is_empty() {
        Value::unavailable("no CPU topology in /sys")
    } else {
        Value::ok(pairs.len().to_string())
    }
}

fn max_mhz() -> Value {
    match fs::read_to_string("/sys/devices/system/cpu/cpu0/cpufreq/cpuinfo_max_freq") {
        Ok(khz) => match khz.trim().parse::<u64>() {
            Ok(khz) => Value::ok((khz / 1000).to_string()),
            Err(_) => Value::unavailable("cpuinfo_max_freq is not a number"),
        },
        Err(_) => Value::unavailable("no cpufreq for cpu0 (a VM, or no frequency driver)"),
    }
}

/// `MemTotal:  16303540 kB` → `15.5 GiB`.
pub fn meminfo_size(meminfo: &str, key: &str) -> Value {
    let kb = meminfo.lines().find_map(|l| {
        l.strip_prefix(key)?
            .split_whitespace()
            .next()?
            .parse::<u64>()
            .ok()
    });
    match kb {
        Some(kb) => Value::ok(human(kb * 1024)),
        None => Value::unavailable(format!(
            "/proc/meminfo has no {}",
            key.trim_end_matches(':')
        )),
    }
}

fn numa_nodes() -> Value {
    match fs::read_dir("/sys/devices/system/node") {
        Ok(dir) => {
            let n = dir
                .flatten()
                .filter(|e| {
                    let name = e.file_name();
                    let name = name.to_string_lossy();
                    name.starts_with("node") && name[4..].chars().all(|c| c.is_ascii_digit())
                })
                .count();
            Value::ok(n.max(1).to_string())
        }
        Err(_) => Value::ok("1"),
    }
}

/// Each DRM card: its driver and PCI vendor:device (`nvidia 10de:1e07`).
fn gpus() -> Value {
    let Ok(dir) = fs::read_dir("/sys/class/drm") else {
        return Value::ok("none");
    };
    let mut cards: Vec<String> = dir
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            // card0, card1: not their connectors (card0-HDMI-A-1)
            if !name.starts_with("card") || name.contains('-') {
                return None;
            }
            let device = e.path().join("device");
            let id = |f: &str| {
                fs::read_to_string(device.join(f))
                    .ok()
                    .map(|v| v.trim().trim_start_matches("0x").to_string())
            };
            let driver = fs::read_link(device.join("driver"))
                .ok()
                .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
                .unwrap_or_else(|| "no driver".into());
            Some(match (id("vendor"), id("device")) {
                (Some(v), Some(d)) => format!("{driver} {v}:{d}"),
                _ => driver,
            })
        })
        .collect();
    cards.sort();
    cards.dedup();
    if cards.is_empty() {
        Value::ok("none")
    } else {
        Value::ok(cards.join(", "))
    }
}

/// Whole disks, with their kind and size: `nvme0n1 ssd 1.8 TiB`. Loop, RAM,
/// zram and device-mapper devices are not disks.
fn disks() -> Value {
    let Ok(dir) = fs::read_dir("/sys/block") else {
        return Value::unavailable("/sys/block not readable");
    };
    let mut disks: Vec<String> = dir
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if ["loop", "ram", "zram", "dm-", "md", "sr", "fd"]
                .iter()
                .any(|p| name.starts_with(p))
            {
                return None;
            }
            let read = |f: &str| {
                fs::read_to_string(e.path().join(f))
                    .ok()
                    .map(|v| v.trim().to_string())
            };
            let sectors: u64 = read("size")?.parse().ok()?;
            if sectors == 0 {
                return None;
            }
            let kind = match read("queue/rotational").as_deref() {
                Some("1") => "hdd",
                Some("0") => "ssd",
                _ => "disk",
            };
            Some(format!("{name} {kind} {}", human(sectors * 512)))
        })
        .collect();
    disks.sort();
    if disks.is_empty() {
        Value::ok("none")
    } else {
        Value::ok(disks.join(", "))
    }
}

/// Binary units, one decimal: `15.5 GiB`.
pub fn human(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_lists() {
        assert_eq!(count_cpu_list("0-23"), 24);
        assert_eq!(count_cpu_list("0-3,8,10-11"), 7);
        assert_eq!(count_cpu_list("0"), 1);
        assert_eq!(count_cpu_list(""), 0);
    }

    #[test]
    fn models_on_x86_and_arm() {
        let x86 = "processor\t: 0\nmodel name\t: AMD Ryzen 9 3900X 12-Core Processor\n";
        assert_eq!(
            cpu_model(x86),
            Value::ok("AMD Ryzen 9 3900X 12-Core Processor")
        );
        let arm = "processor\t: 0\nCPU implementer\t: 0x41\nCPU part\t: 0xd0c\n";
        assert_eq!(cpu_model(arm), Value::ok("implementer 0x41, part 0xd0c"));
        assert!(matches!(cpu_model(""), Value::Unavailable(_)));
    }

    #[test]
    fn sizes() {
        let meminfo = "MemTotal:       16303540 kB\nSwapTotal:             0 kB\n";
        assert_eq!(meminfo_size(meminfo, "MemTotal:"), Value::ok("15.5 GiB"));
        assert_eq!(meminfo_size(meminfo, "SwapTotal:"), Value::ok("0 B"));
        assert!(matches!(
            meminfo_size("", "MemTotal:"),
            Value::Unavailable(_)
        ));
        assert_eq!(human(2_000_398_934_016), "1.8 TiB");
    }
}
