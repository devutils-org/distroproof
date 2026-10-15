//! LSM, SELinux, AppArmor, lockdown, Secure Boot.

use std::fs;
use std::path::Path;

use crate::parse::bracketed;
use crate::{util, Section, Value};

const SECURE_BOOT_VAR: &str =
    "/sys/firmware/efi/efivars/SecureBoot-8be4df61-93ca-11d2-aa0d-00e098032b8c";

pub fn probe() -> Section {
    let mut s = Section::new("security");
    s.push("lsm", util::read_value("/sys/kernel/security/lsm"));
    s.push("selinux", selinux());
    s.push("apparmor", apparmor());
    s.push_privileged("apparmor_profiles", apparmor_profiles());
    s.push("lockdown", lockdown());
    s.push("secure_boot", secure_boot());
    s
}

fn selinux() -> Value {
    if !Path::new("/sys/fs/selinux").exists() {
        return Value::ok("absent");
    }
    match util::read_trimmed("/sys/fs/selinux/enforce") {
        Ok(v) if v == "1" => Value::ok("enforcing"),
        Ok(v) if v == "0" => Value::ok("permissive"),
        Ok(v) => Value::ok(v),
        Err(v) => v,
    }
}

fn apparmor() -> Value {
    match util::read_trimmed("/sys/module/apparmor/parameters/enabled") {
        Ok(v) if v == "Y" => Value::ok("activé"),
        Ok(_) => Value::ok("désactivé"),
        Err(Value::Unavailable(_)) => Value::ok("absent"),
        Err(v) => v,
    }
}

fn apparmor_profiles() -> Value {
    let path = Path::new("/sys/kernel/security/apparmor/profiles");
    match fs::read_to_string(path) {
        Ok(text) => {
            let total = text.lines().count();
            let enforce = text.lines().filter(|l| l.ends_with("(enforce)")).count();
            Value::ok(format!("{total} profils, {enforce} en enforce"))
        }
        Err(e) => util::io_value(&e, path),
    }
}

fn lockdown() -> Value {
    match util::read_trimmed("/sys/kernel/security/lockdown") {
        Ok(text) => bracketed(&text).map_or_else(|| Value::ok(text.as_str()), Value::ok),
        Err(v) => v,
    }
}

fn secure_boot() -> Value {
    if !Path::new("/sys/firmware/efi").exists() {
        return Value::ok("non (démarrage non EFI)");
    }
    match fs::read(SECURE_BOOT_VAR) {
        // 4 octets d'attributs, puis la valeur.
        Ok(data) if data.len() >= 5 => Value::ok(if data[4] == 1 {
            "activé"
        } else {
            "désactivé"
        }),
        Ok(_) => Value::unavailable("variable SecureBoot tronquée"),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Value::ok("désactivé (variable absente)")
        }
        Err(e) => util::io_value(&e, Path::new(SECURE_BOOT_VAR)),
    }
}
