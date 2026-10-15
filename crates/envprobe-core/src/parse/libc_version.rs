//! Version de la libc sans l'exécuter.

/// Cherche « release version X.Y » dans le binaire `libc.so.6` de glibc.
pub fn glibc_version(data: &[u8]) -> Option<String> {
    let marker = b"release version ";
    let pos = data.windows(marker.len()).position(|w| w == marker)? + marker.len();
    let version: String = data[pos..]
        .iter()
        .take_while(|b| b.is_ascii_digit() || **b == b'.')
        .map(|&b| b as char)
        .collect();
    let version = version.trim_end_matches('.');
    (!version.is_empty()).then(|| version.to_string())
}

/// Version du paquet `musl` dans la base apk (`/lib/apk/db/installed`).
pub fn musl_version_from_apk(db: &str) -> Option<String> {
    db.split("\n\n").find_map(|block| {
        let mut lines = block.lines();
        lines.clone().any(|l| l == "P:musl").then_some(())?;
        lines.find_map(|l| l.strip_prefix("V:")).map(str::to_string)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glibc() {
        let blob = b"\0\0GNU C Library (Debian GLIBC 2.36-9+deb12u7) stable release version 2.36.\nCopyright";
        assert_eq!(glibc_version(blob).as_deref(), Some("2.36"));
        assert_eq!(glibc_version(b"nothing here"), None);
    }

    #[test]
    fn musl_apk() {
        let db = "C:Q1abc\nP:busybox\nV:1.36.1-r29\n\nC:Q1def\nP:musl\nV:1.2.5-r0\nA:x86_64\n\n";
        assert_eq!(musl_version_from_apk(db).as_deref(), Some("1.2.5-r0"));
        assert_eq!(musl_version_from_apk("P:busybox\nV:1\n"), None);
    }
}
