//! Une sonde par catégorie, dans l'ordre d'affichage du rapport.

pub mod bpf;
pub mod btf;
pub mod cgroups;
pub mod clib;
pub mod fhs;
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
pub mod tracing;
pub mod xdg;

use crate::Section;

pub type Probe = fn() -> Section;

pub const ALL: &[(&str, Probe)] = &[
    ("identity", identity::probe),
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
    ("fhs", fhs::probe),
    ("shell", shell::probe),
    ("init", init::probe),
    ("packages", packages::probe),
    ("cgroups", cgroups::probe),
    ("xdg", xdg::probe),
];
