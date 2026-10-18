//! Pure parsers, with no system access, tested on fixtures.

pub mod elf;
pub mod kconfig;
pub mod ldconfig;
pub mod libc_version;
pub mod limits;
pub mod mounts;
pub mod os_release;
pub mod status;

/// Extracts the active value from a list like `none [integrity] confidentiality`.
pub fn bracketed(s: &str) -> Option<&str> {
    let start = s.find('[')? + 1;
    let end = start + s[start..].find(']')?;
    Some(&s[start..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bracketed_value() {
        assert_eq!(
            bracketed("none [integrity] confidentiality"),
            Some("integrity")
        );
        assert_eq!(bracketed("[none] integrity"), Some("none"));
        assert_eq!(bracketed("none integrity"), None);
    }
}
