//! `/proc/mounts`.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mount {
    pub source: String,
    pub target: String,
    pub fstype: String,
    pub options: String,
}

pub fn parse(text: &str) -> Vec<Mount> {
    text.lines()
        .filter_map(|line| {
            let mut f = line.split_whitespace();
            Some(Mount {
                source: unescape(f.next()?),
                target: unescape(f.next()?),
                fstype: f.next()?.to_string(),
                options: f.next()?.to_string(),
            })
        })
        .collect()
}

/// The kernel encodes spaces, tabs and `\` in octal (`\040`).
fn unescape(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 3 < bytes.len() {
            let digits = &bytes[i + 1..i + 4];
            if digits.iter().all(|d| (b'0'..=b'7').contains(d)) {
                let v = digits
                    .iter()
                    .fold(0u32, |acc, d| acc * 8 + u32::from(d - b'0'));
                out.push(v as u8);
                i += 4;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_unescape() {
        let m = parse("tracefs /sys/kernel/tracing tracefs rw,nosuid 0 0\n/dev/sda1 /mnt/my\\040disk ext4 rw 0 0\ngarbage\n");
        assert_eq!(m.len(), 2);
        assert_eq!(m[0].fstype, "tracefs");
        assert_eq!(m[1].target, "/mnt/my disk");
    }
}
