//! `perf_event_open` logiciel et matériel, PMU disponibles.

use std::fs;
use std::path::Path;

use crate::sys;
use crate::util::{self, errno_name};
use crate::{Section, Value};

pub fn probe() -> Section {
    let mut s = Section::new("perf");
    let events = [
        (
            "sw_cpu_clock.user",
            sys::PERF_TYPE_SOFTWARE,
            sys::PERF_COUNT_SW_CPU_CLOCK,
            true,
        ),
        (
            "sw_cpu_clock.kernel",
            sys::PERF_TYPE_SOFTWARE,
            sys::PERF_COUNT_SW_CPU_CLOCK,
            false,
        ),
        (
            "hw_cpu_cycles.user",
            sys::PERF_TYPE_HARDWARE,
            sys::PERF_COUNT_HW_CPU_CYCLES,
            true,
        ),
        (
            "hw_cpu_cycles.kernel",
            sys::PERF_TYPE_HARDWARE,
            sys::PERF_COUNT_HW_CPU_CYCLES,
            false,
        ),
    ];
    for (key, kind, config, user_only) in events {
        let value = match sys::perf_event_open(kind, config, user_only) {
            Ok(_fd) => Value::ok("ouvert"),
            Err(libc::ENOSYS) => {
                Value::unavailable("ENOSYS : perf_event_open absent (CONFIG_PERF_EVENTS)")
            }
            Err(e @ (libc::ENOENT | libc::EOPNOTSUPP | libc::ENODEV)) => {
                Value::ok(format!("non supporté ({})", errno_name(e)))
            }
            Err(e) => util::errno_value(e),
        };
        s.push_privileged(key, value);
    }
    let dir = Path::new("/sys/bus/event_source/devices");
    s.push(
        "pmus",
        match fs::read_dir(dir) {
            Ok(entries) => {
                let mut names: Vec<String> = entries
                    .flatten()
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect();
                names.sort();
                Value::ok(names.join(", "))
            }
            Err(e) => util::io_value(&e, dir),
        },
    );
    s
}
