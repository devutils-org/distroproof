//! Kernel and module BTF.

use std::fs;
use std::path::Path;

use crate::{util, Section, Value};

pub fn probe() -> Section {
    let mut s = Section::new("btf");
    let vmlinux = Path::new("/sys/kernel/btf/vmlinux");
    s.push(
        "vmlinux",
        match fs::metadata(vmlinux) {
            Ok(m) => Value::ok(format!("present, {} bytes", m.len())),
            Err(e) => util::io_value(&e, vmlinux),
        },
    );
    s.push(
        "modules",
        match fs::read_dir("/sys/kernel/btf") {
            Ok(entries) => {
                let count = entries
                    .flatten()
                    .filter(|e| e.file_name() != "vmlinux")
                    .count();
                Value::ok(count.to_string())
            }
            Err(e) => util::io_value(&e, Path::new("/sys/kernel/btf")),
        },
    );
    s
}
