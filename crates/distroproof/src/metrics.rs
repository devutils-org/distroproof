//! CPU and memory of a QEMU process, read from `/proc/<pid>`.

use std::fs;
use std::time::Instant;

/// Cumulative CPU time (utime + stime) in ticks, fields 14 and 15 of `stat`.
pub fn parse_cpu_ticks(stat: &str) -> Option<u64> {
    // The process name (field 2) can contain spaces: start again from the
    // last closing parenthesis.
    let rest = &stat[stat.rfind(')')? + 2..];
    let fields: Vec<&str> = rest.split_whitespace().collect();
    // After ")", field 3 (state) is at index 0: utime = 14 → index 11.
    let utime: u64 = fields.get(11)?.parse().ok()?;
    let stime: u64 = fields.get(12)?.parse().ok()?;
    Some(utime + stime)
}

/// `VmRSS` from `/proc/<pid>/status`, in bytes.
pub fn parse_rss(status: &str) -> Option<u64> {
    let line = status.lines().find(|l| l.starts_with("VmRSS:"))?;
    let kib: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kib * 1024)
}

/// Sampler: the CPU percentage is computed between two calls.
pub struct Sampler {
    pid: u32,
    ticks_per_sec: u64,
    last: Option<(Instant, u64)>,
}

impl Sampler {
    pub fn new(pid: u32) -> Self {
        Sampler {
            pid,
            ticks_per_sec: crate::sys::clock_ticks(),
            last: None,
        }
    }

    /// (CPU as a % of one core, RSS in bytes).
    pub fn sample(&mut self) -> Option<(f64, u64)> {
        let stat = fs::read_to_string(format!("/proc/{}/stat", self.pid)).ok()?;
        let status = fs::read_to_string(format!("/proc/{}/status", self.pid)).ok()?;
        let ticks = parse_cpu_ticks(&stat)?;
        let rss = parse_rss(&status).unwrap_or(0);
        let now = Instant::now();
        let cpu = match self.last {
            Some((t, prev)) => {
                let secs = now.duration_since(t).as_secs_f64();
                if secs > 0.0 {
                    (ticks.saturating_sub(prev) as f64 / self.ticks_per_sec as f64) / secs * 100.0
                } else {
                    0.0
                }
            }
            None => 0.0,
        };
        self.last = Some((now, ticks));
        Some((cpu, rss))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_stat_with_spaces_in_name() {
        let stat = "1234 (qemu-system x86) S 1 1234 1234 0 -1 4194560 1000 0 0 0 250 50 0 0 20 0 5 0 100 0 0";
        assert_eq!(parse_cpu_ticks(stat), Some(300));
    }

    #[test]
    fn parses_rss() {
        assert_eq!(
            parse_rss("Name:\tqemu\nVmRSS:\t  2048 kB\n"),
            Some(2048 * 1024)
        );
    }
}
