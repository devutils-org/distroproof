//! `/proc/<pid>/status`.

use std::collections::BTreeMap;

pub struct Status {
    fields: BTreeMap<String, String>,
}

pub fn parse(text: &str) -> Status {
    let fields = text
        .lines()
        .filter_map(|line| line.split_once(':'))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect();
    Status { fields }
}

impl Status {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.fields.get(key).map(String::as_str)
    }

    /// Real, effective, saved and fs IDs (`Uid:` line).
    pub fn uids(&self) -> Option<[u32; 4]> {
        ids(self.get("Uid")?)
    }

    pub fn gids(&self) -> Option<[u32; 4]> {
        ids(self.get("Gid")?)
    }

    pub fn cap_eff(&self) -> Option<u64> {
        u64::from_str_radix(self.get("CapEff")?, 16).ok()
    }

    /// 0: disabled, 1: strict, 2: filter.
    pub fn seccomp(&self) -> Option<u32> {
        self.get("Seccomp")?.parse().ok()
    }

    pub fn no_new_privs(&self) -> Option<bool> {
        Some(self.get("NoNewPrivs")?.parse::<u32>().ok()? != 0)
    }
}

fn ids(value: &str) -> Option<[u32; 4]> {
    let mut out = [0u32; 4];
    let mut parts = value.split_whitespace();
    for slot in &mut out {
        *slot = parts.next()?.parse().ok()?;
    }
    Some(out)
}

/// `/proc/<pid>/uid_map` of the initial user namespace: `0 0 4294967295`.
pub fn is_initial_uid_map(map: &str) -> bool {
    let lines: Vec<Vec<&str>> = map
        .lines()
        .map(|l| l.split_whitespace().collect())
        .collect();
    lines == [vec!["0", "0", "4294967295"]]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uid_maps() {
        assert!(is_initial_uid_map("         0          0 4294967295\n"));
        assert!(!is_initial_uid_map("         0        501          1\n"));
        assert!(!is_initial_uid_map(""));
    }
}
