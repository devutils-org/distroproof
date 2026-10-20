//! One probe per category, in report display order.

pub mod bpf;
pub mod btf;
pub mod cgroups;
pub mod clib;
pub mod fhs;
pub mod hardware;
pub mod identity;
pub mod init;
pub mod kernel;
pub mod libs;
pub mod modules;
pub mod packages;
pub mod perf;
pub mod privileges;
pub mod security;
pub mod shell;
pub mod sysctl;
pub mod toolchain;
pub mod tracing;
pub mod xdg;

use crate::Section;

pub type Probe = fn() -> Section;

pub const ALL: &[(&str, Probe)] = &[
    ("identity", identity::probe),
    ("hardware", hardware::probe),
    ("kernel", kernel::probe),
    ("modules", modules::probe),
    ("sysctl", sysctl::probe),
    ("security", security::probe),
    ("privileges", privileges::probe),
    ("btf", btf::probe),
    ("bpf", bpf::probe),
    ("perf", perf::probe),
    ("tracing", tracing::probe),
    ("libc", clib::probe),
    ("libs", libs::probe),
    ("toolchain", toolchain::probe),
    ("fhs", fhs::probe),
    ("shell", shell::probe),
    ("init", init::probe),
    ("packages", packages::probe),
    ("cgroups", cgroups::probe),
    ("xdg", xdg::probe),
];
