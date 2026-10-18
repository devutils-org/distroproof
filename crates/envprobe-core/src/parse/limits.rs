//! `/proc/<pid>/limits`: one resource a line, soft and hard limit, unit.

/// The soft and hard limit of `resource` (`Max locked memory`), as the file
/// prints them: a number or `unlimited`.
pub fn limit(text: &str, resource: &str) -> Option<(String, String)> {
    let line = text.lines().find(|line| line.starts_with(resource))?;
    let mut fields = line[resource.len()..].split_whitespace();
    Some((fields.next()?.to_string(), fields.next()?.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEBIAN_11_USER: &str =
        "Limit                     Soft Limit           Hard Limit           Units
Max cpu time              unlimited            unlimited            seconds
Max locked memory         65536                65536                bytes
Max address space         unlimited            unlimited            bytes
";

    #[test]
    fn the_locked_memory_limit_is_read_soft_then_hard() {
        assert_eq!(
            limit(DEBIAN_11_USER, "Max locked memory"),
            Some(("65536".into(), "65536".into()))
        );
        assert_eq!(
            limit(DEBIAN_11_USER, "Max cpu time"),
            Some(("unlimited".into(), "unlimited".into()))
        );
        assert_eq!(limit(DEBIAN_11_USER, "Max realtime timeout"), None);
    }
}
