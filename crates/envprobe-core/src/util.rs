//! Petites aides partagées par les sondes.

use std::fs;
use std::io;
use std::path::Path;

use crate::Value;

/// Convertit une erreur d'E/S en valeur de rapport.
pub(crate) fn io_value(err: &io::Error, path: &Path) -> Value {
    match err.kind() {
        io::ErrorKind::NotFound => Value::unavailable(format!("{} absent", path.display())),
        io::ErrorKind::PermissionDenied => Value::denied(format!(
            "{} : {}",
            errno_name(err.raw_os_error().unwrap_or(libc::EACCES)),
            path.display()
        )),
        _ => Value::unavailable(format!("{} : {err}", path.display())),
    }
}

/// Lit un fichier texte et retire les blancs en bordure.
pub(crate) fn read_trimmed(path: impl AsRef<Path>) -> Result<String, Value> {
    let path = path.as_ref();
    fs::read_to_string(path)
        .map(|s| s.trim().to_string())
        .map_err(|e| io_value(&e, path))
}

/// Comme [`read_trimmed`], mais renvoie directement une [`Value`].
pub(crate) fn read_value(path: impl AsRef<Path>) -> Value {
    read_trimmed(path).map_or_else(|v| v, Value::Ok)
}

/// Résultat d'un syscall en échec : EPERM et EACCES sont des refus, le reste une indisponibilité.
pub(crate) fn errno_value(errno: i32) -> Value {
    match errno {
        libc::EPERM | libc::EACCES => Value::denied(errno_name(errno)),
        _ => Value::unavailable(errno_name(errno)),
    }
}

pub fn errno_name(errno: i32) -> String {
    let name = match errno {
        libc::EPERM => "EPERM",
        libc::ENOENT => "ENOENT",
        libc::EINTR => "EINTR",
        libc::EBADF => "EBADF",
        libc::EAGAIN => "EAGAIN",
        libc::ENOMEM => "ENOMEM",
        libc::EACCES => "EACCES",
        libc::EFAULT => "EFAULT",
        libc::EBUSY => "EBUSY",
        libc::ENODEV => "ENODEV",
        libc::EINVAL => "EINVAL",
        libc::E2BIG => "E2BIG",
        libc::ENOSPC => "ENOSPC",
        libc::ENOSYS => "ENOSYS",
        libc::EOPNOTSUPP => "EOPNOTSUPP",
        524 => "ENOTSUPP",
        _ => return format!("errno {errno}"),
    };
    name.to_string()
}

pub(crate) fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && haystack.windows(needle.len()).any(|w| w == needle)
}

/// Triplet multiarch Debian correspondant à l'architecture de la sonde.
pub(crate) fn multiarch_triplet() -> String {
    format!("{}-linux-gnu", std::env::consts::ARCH)
}
