//! uid, capabilities effectives, seccomp, NoNewPrivs.

use std::fs;
use std::path::Path;

use crate::parse::status::{self, Status};
use crate::{util, Section, Value};

pub const CAP_SYS_MODULE: u32 = 16;
pub const CAP_SYS_PTRACE: u32 = 19;
pub const CAP_SYS_ADMIN: u32 = 21;
pub const CAP_PERFMON: u32 = 38;
pub const CAP_BPF: u32 = 39;

const CAPS: &[(&str, u32)] = &[
    ("CAP_BPF", CAP_BPF),
    ("CAP_PERFMON", CAP_PERFMON),
    ("CAP_SYS_ADMIN", CAP_SYS_ADMIN),
    ("CAP_SYS_PTRACE", CAP_SYS_PTRACE),
    ("CAP_SYS_MODULE", CAP_SYS_MODULE),
];

pub fn has_cap(mask: u64, cap: u32) -> bool {
    (mask >> cap) & 1 == 1
}

pub(crate) fn self_status() -> Result<Status, Value> {
    let path = Path::new("/proc/self/status");
    fs::read_to_string(path)
        .map(|t| status::parse(&t))
        .map_err(|e| util::io_value(&e, path))
}

pub(crate) fn cap_eff() -> Option<u64> {
    self_status().ok()?.cap_eff()
}

/// Vrai dans le user namespace initial. Ailleurs, les capabilities ne valent
/// que dans le namespace : CAP_BPF affiché n'autorise pas `bpf()`.
pub(crate) fn initial_user_ns() -> Option<bool> {
    let map = fs::read_to_string("/proc/self/uid_map").ok()?;
    Some(crate::parse::status::is_initial_uid_map(&map))
}

pub fn probe() -> Section {
    let mut s = Section::new("privileges");
    let status = match self_status() {
        Ok(st) => st,
        Err(err) => {
            for key in [
                "uid",
                "euid",
                "cap_eff",
                "user_namespace",
                "seccomp",
                "no_new_privs",
            ] {
                s.push_privileged(key, err.clone());
            }
            return s;
        }
    };
    let missing = |field: &str| Value::unavailable(format!("champ {field} absent"));
    match status.uids() {
        Some(ids) => {
            s.push_privileged("uid", Value::ok(ids[0].to_string()));
            s.push_privileged("euid", Value::ok(ids[1].to_string()));
        }
        None => {
            s.push_privileged("uid", missing("Uid"));
            s.push_privileged("euid", missing("Uid"));
        }
    }
    match status.cap_eff() {
        Some(mask) => {
            s.push_privileged("cap_eff", Value::ok(format!("{mask:#018x}")));
            for (name, bit) in CAPS {
                s.push_privileged(
                    *name,
                    Value::ok(if has_cap(mask, *bit) { "oui" } else { "non" }),
                );
            }
        }
        None => s.push_privileged("cap_eff", missing("CapEff")),
    }
    s.push_privileged(
        "user_namespace",
        match initial_user_ns() {
            Some(true) => Value::ok("initial"),
            Some(false) => Value::ok("non initial (capabilities limitées au namespace)"),
            None => Value::unavailable("/proc/self/uid_map illisible"),
        },
    );
    s.push_privileged(
        "seccomp",
        match status.seccomp() {
            Some(0) => Value::ok("désactivé"),
            Some(1) => Value::ok("strict"),
            Some(2) => Value::ok("filtre"),
            Some(n) => Value::ok(n.to_string()),
            None => missing("Seccomp"),
        },
    );
    s.push_privileged(
        "no_new_privs",
        status
            .no_new_privs()
            .map(|b| Value::ok(if b { "oui" } else { "non" }))
            .unwrap_or_else(|| missing("NoNewPrivs")),
    );
    s
}
