//! TUI theme: `$XDG_CONFIG_HOME/distroproof/theme.toml`.
//!
//! ```toml
//! [colors]
//! accent = "#7aa2f7"
//! success = "#9ece6a"
//! ```
//!
//! Colors are true color if the terminal advertises it (`COLORTERM`),
//! otherwise mapped to the closest color of the 256-color palette.

use std::collections::HashMap;

use ratatui::style::Color;
use serde::Deserialize;

#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub fg: Color,
    pub muted: Color,
    pub accent: Color,
    pub border: Color,
    pub border_focus: Color,
    pub selection: Color,
    pub success: Color,
    pub failure: Color,
    pub running: Color,
    pub warning: Color,
    pub cpu: Color,
    pub mem: Color,
    pub search: Color,
}

const DEFAULT: &[(&str, (u8, u8, u8))] = &[
    ("fg", (0xc0, 0xca, 0xf5)),
    ("muted", (0x56, 0x5f, 0x89)),
    ("accent", (0x7a, 0xa2, 0xf7)),
    ("border", (0x3b, 0x42, 0x61)),
    ("border_focus", (0x7d, 0xcf, 0xff)),
    ("selection", (0x28, 0x34, 0x57)),
    ("success", (0x9e, 0xce, 0x6a)),
    ("failure", (0xf7, 0x76, 0x8e)),
    ("running", (0x7d, 0xcf, 0xff)),
    ("warning", (0xe0, 0xaf, 0x68)),
    ("cpu", (0xbb, 0x9a, 0xf7)),
    ("mem", (0x73, 0xda, 0xca)),
    ("search", (0xff, 0x9e, 0x64)),
];

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct ThemeFile {
    #[serde(default)]
    colors: HashMap<String, String>,
}

fn parse_hex(s: &str) -> Option<(u8, u8, u8)> {
    let h = s.trim().strip_prefix('#')?;
    if h.len() != 6 {
        return None;
    }
    let v = u32::from_str_radix(h, 16).ok()?;
    Some(((v >> 16) as u8, (v >> 8) as u8, v as u8))
}

/// Closest xterm-256 index for an RGB color (6×6×6 cube or grays).
pub fn to_256(r: u8, g: u8, b: u8) -> u8 {
    let level = |c: u8| -> u8 {
        match c {
            0..=47 => 0,
            48..=114 => 1,
            _ => (c - 35) / 40,
        }
    };
    let steps = [0u8, 95, 135, 175, 215, 255];
    let (lr, lg, lb) = (level(r), level(g), level(b));
    let cube = 16 + 36 * lr + 6 * lg + lb;
    let cube_rgb = (steps[lr as usize], steps[lg as usize], steps[lb as usize]);
    let avg = (u16::from(r) + u16::from(g) + u16::from(b)) / 3;
    let gray_idx = if avg > 238 {
        23
    } else {
        avg.saturating_sub(3) / 10
    } as u8;
    let gray = 8 + 10 * gray_idx;
    let dist = |(a, b2, c): (u8, u8, u8)| {
        let d = |x: u8, y: u8| (i32::from(x) - i32::from(y)).pow(2);
        d(a, r) + d(b2, g) + d(c, b)
    };
    if dist((gray, gray, gray)) < dist(cube_rgb) {
        232 + gray_idx
    } else {
        cube
    }
}

fn truecolor() -> bool {
    std::env::var("COLORTERM")
        .map(|v| v.contains("truecolor") || v.contains("24bit"))
        .unwrap_or(false)
}

impl Theme {
    /// Default theme, overridden by the user's file if it exists. Also returns
    /// a warning if the file is invalid.
    pub fn load() -> (Theme, Option<String>) {
        let path = crate::dirs::config().join("theme.toml");
        let mut warning = None;
        let file: ThemeFile = match std::fs::read_to_string(&path) {
            Ok(text) => toml::from_str(&text).unwrap_or_else(|e| {
                warning = Some(format!("{}: {}", path.display(), e.message()));
                ThemeFile::default()
            }),
            Err(_) => ThemeFile::default(),
        };
        let mut colors: HashMap<&str, (u8, u8, u8)> = DEFAULT.iter().copied().collect();
        for (k, v) in &file.colors {
            match (colors.contains_key(k.as_str()), parse_hex(v)) {
                (true, Some(rgb)) => {
                    let key = DEFAULT.iter().find(|(n, _)| n == k).map(|(n, _)| *n);
                    if let Some(key) = key {
                        colors.insert(key, rgb);
                    }
                }
                (false, _) => warning = Some(format!("theme.toml: unknown color \"{k}\"")),
                (true, None) => {
                    warning = Some(format!("theme.toml: \"{v}\" is not in #rrggbb format"))
                }
            }
        }
        let tc = truecolor();
        let c = |name: &str| {
            let (r, g, b) = colors[name];
            if tc {
                Color::Rgb(r, g, b)
            } else {
                Color::Indexed(to_256(r, g, b))
            }
        };
        (
            Theme {
                fg: c("fg"),
                muted: c("muted"),
                accent: c("accent"),
                border: c("border"),
                border_focus: c("border_focus"),
                selection: c("selection"),
                success: c("success"),
                failure: c("failure"),
                running: c("running"),
                warning: c("warning"),
                cpu: c("cpu"),
                mem: c("mem"),
                search: c("search"),
            },
            warning,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_to_256_palette() {
        assert_eq!(to_256(0, 0, 0), 16);
        assert_eq!(to_256(255, 255, 255), 231);
        assert_eq!(to_256(255, 0, 0), 196);
        assert_eq!(to_256(128, 128, 128), 244);
        assert_eq!(parse_hex("#7aa2f7"), Some((0x7a, 0xa2, 0xf7)));
        assert_eq!(parse_hex("7aa2f7"), None);
    }
}
