//! Sysctls qui changent le comportement de perf, BPF et ptrace.

use crate::{util, Section};

pub const KEYS: &[&str] = &[
    "kernel.perf_event_paranoid",
    "kernel.perf_event_max_sample_rate",
    "kernel.unprivileged_bpf_disabled",
    "net.core.bpf_jit_enable",
    "net.core.bpf_jit_harden",
    "kernel.kptr_restrict",
    "kernel.dmesg_restrict",
    "kernel.yama.ptrace_scope",
];

pub fn probe() -> Section {
    let mut s = Section::new("sysctl");
    for key in KEYS {
        s.push(
            *key,
            util::read_value(format!("/proc/sys/{}", key.replace('.', "/"))),
        );
    }
    s
}
