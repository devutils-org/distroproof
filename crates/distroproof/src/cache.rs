//! Last-level cache domains: VMs that measure performance each get one of
//! their own, so a neighbour's working set does not evict theirs (`up
//! --pin-cache`).

use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};

/// The CPUs sharing each L3 cache, from `<root>/cpu*/cache/index*/` (root:
/// `/sys/devices/system/cpu`). Sorted by first CPU; empty when sysfs does not
/// describe the caches.
pub fn l3_domains(root: &Path) -> Vec<Vec<usize>> {
    let mut domains: Vec<Vec<usize>> = Vec::new();
    let Ok(cpus) = fs::read_dir(root) else {
        return domains;
    };
    for cpu in cpus.flatten() {
        let name = cpu.file_name();
        let is_cpu = name
            .to_str()
            .and_then(|n| n.strip_prefix("cpu"))
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
        if !is_cpu {
            continue;
        }
        let Ok(indexes) = fs::read_dir(cpu.path().join("cache")) else {
            continue;
        };
        for index in indexes.flatten() {
            let level = fs::read_to_string(index.path().join("level")).unwrap_or_default();
            if level.trim() != "3" {
                continue;
            }
            let Ok(list) = fs::read_to_string(index.path().join("shared_cpu_list")) else {
                continue;
            };
            let Some(shared) = parse_list(list.trim()) else {
                continue;
            };
            if !shared.is_empty() && !domains.contains(&shared) {
                domains.push(shared);
            }
        }
    }
    domains.sort();
    domains
}

/// A kernel CPU list (`0-2,12-14`) as CPU numbers, sorted.
pub fn parse_list(list: &str) -> Option<Vec<usize>> {
    let mut cpus = Vec::new();
    for part in list.split(',').filter(|p| !p.is_empty()) {
        match part.split_once('-') {
            Some((a, b)) => {
                let (a, b) = (a.parse::<usize>().ok()?, b.parse::<usize>().ok()?);
                if a > b {
                    return None;
                }
                cpus.extend(a..=b);
            }
            None => cpus.push(part.parse().ok()?),
        }
    }
    cpus.sort_unstable();
    cpus.dedup();
    Some(cpus)
}

/// CPU numbers back in the kernel's list form.
pub fn format_list(cpus: &[usize]) -> String {
    let mut parts = Vec::new();
    let mut i = 0;
    while i < cpus.len() {
        let start = cpus[i];
        while i + 1 < cpus.len() && cpus[i + 1] == cpus[i] + 1 {
            i += 1;
        }
        parts.push(if cpus[i] == start {
            start.to_string()
        } else {
            format!("{start}-{}", cpus[i])
        });
        i += 1;
    }
    parts.join(",")
}

/// The free domains. A VM takes one before QEMU starts and gives it back when
/// it ends; with at most as many jobs as domains, one is always free.
#[derive(Clone)]
pub struct Pool(Arc<Mutex<Vec<Vec<usize>>>>);

impl Pool {
    pub fn new(domains: Vec<Vec<usize>>) -> Pool {
        Pool(Arc::new(Mutex::new(domains)))
    }

    pub fn take(&self) -> Option<Lease> {
        let mut free = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if free.is_empty() {
            return None;
        }
        Some(Lease {
            cpus: free.remove(0),
            pool: self.clone(),
        })
    }
}

/// A domain held by one VM, returned to the pool on drop.
pub struct Lease {
    pub cpus: Vec<usize>,
    pool: Pool,
}

impl Drop for Lease {
    fn drop(&mut self) {
        let mut free = self.pool.0.lock().unwrap_or_else(|e| e.into_inner());
        free.push(std::mem::take(&mut self.cpus));
        free.sort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_round_trip() {
        assert_eq!(parse_list("0-2,12-14"), Some(vec![0, 1, 2, 12, 13, 14]));
        assert_eq!(parse_list("5"), Some(vec![5]));
        assert_eq!(parse_list("3-1"), None);
        assert_eq!(parse_list("x"), None);
        assert_eq!(format_list(&[0, 1, 2, 12, 13, 14]), "0-2,12-14");
        assert_eq!(format_list(&[4, 7]), "4,7");
    }

    #[test]
    fn domains_from_a_sysfs_tree() {
        let root = std::env::temp_dir().join(format!("dp-cache-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        // A Ryzen 3900X: four L3 domains of three cores and their SMT siblings.
        for cpu in 0..24 {
            let base = (cpu % 12) / 3 * 3;
            for (index, level, shared) in [
                (0, "1", cpu.to_string()),
                (
                    3,
                    "3",
                    format!("{}-{},{}-{}", base, base + 2, base + 12, base + 14),
                ),
            ] {
                let dir = root.join(format!("cpu{cpu}/cache/index{index}"));
                fs::create_dir_all(&dir).unwrap();
                fs::write(dir.join("level"), format!("{level}\n")).unwrap();
                fs::write(dir.join("shared_cpu_list"), format!("{shared}\n")).unwrap();
            }
        }
        fs::create_dir_all(root.join("cpufreq")).unwrap();
        let domains = l3_domains(&root);
        fs::remove_dir_all(&root).unwrap();
        assert_eq!(domains.len(), 4);
        assert_eq!(domains[0], vec![0, 1, 2, 12, 13, 14]);
        assert_eq!(domains[3], vec![9, 10, 11, 21, 22, 23]);
    }

    #[test]
    fn a_lease_returns_its_domain() {
        let pool = Pool::new(vec![vec![0], vec![1]]);
        let a = pool.take().unwrap();
        let b = pool.take().unwrap();
        assert!(pool.take().is_none());
        assert_eq!((a.cpus.clone(), b.cpus.clone()), (vec![0], vec![1]));
        drop(a);
        assert_eq!(pool.take().unwrap().cpus, vec![0]);
    }
}
