//! cgroups version.

use crate::probes::tracing::read_mounts;
use crate::{util, Section, Value};

pub fn probe() -> Section {
    let mut s = Section::new("cgroups");
    match read_mounts() {
        Ok(mounts) => {
            let v2_root = mounts
                .iter()
                .any(|m| m.fstype == "cgroup2" && m.target == "/sys/fs/cgroup");
            let v2_any = mounts.iter().any(|m| m.fstype == "cgroup2");
            let v1 = mounts.iter().any(|m| m.fstype == "cgroup");
            let mode = if v1 && v2_any {
                Value::ok("hybrid")
            } else if v2_root {
                Value::ok("v2")
            } else if v2_any {
                Value::ok("v2 (outside /sys/fs/cgroup)")
            } else if v1 {
                Value::ok("v1")
            } else {
                Value::unavailable("no cgroup mounted")
            };
            s.push("mode", mode);
        }
        Err(err) => s.push("mode", err),
    }
    s.push(
        "controllers",
        util::read_value("/sys/fs/cgroup/cgroup.controllers"),
    );
    s.push("self", util::read_value("/proc/self/cgroup"));
    s
}
