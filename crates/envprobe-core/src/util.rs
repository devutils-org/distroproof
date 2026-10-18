//! Small helpers shared by the probes.

use std::fs;
use std::io;
use std::path::Path;

use crate::Value;

/// Turns an I/O error into a report value.
pub(crate) fn io_value(err: &io::Error, path: &Path) -> Value {
    match err.kind() {
        io::ErrorKind::NotFound => Value::unavailable(format!("{} missing", path.display())),
        io::ErrorKind::PermissionDenied => Value::denied(format!(
            "{}: {}",
            errno_name(err.raw_os_error().unwrap_or(libc::EACCES)),
            path.display()
        )),
        _ => Value::unavailable(format!("{}: {err}", path.display())),
    }
}

/// Reads a text file and trims surrounding whitespace.
pub(crate) fn read_trimmed(path: impl AsRef<Path>) -> Result<String, Value> {
    let path = path.as_ref();
    fs::read_to_string(path)
        .map(|s| s.trim().to_string())
        .map_err(|e| io_value(&e, path))
}

/// Like [`read_trimmed`], but returns a [`Value`] directly.
pub(crate) fn read_value(path: impl AsRef<Path>) -> Value {
    read_trimmed(path).map_or_else(|v| v, Value::Ok)
}

/// Result of a failed syscall: EPERM and EACCES are denials, anything else is unavailability.
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

/// Debian multiarch triplet matching the probe's architecture.
pub(crate) fn multiarch_triplet() -> String {
    format!("{}-linux-gnu", std::env::consts::ARCH)
}
