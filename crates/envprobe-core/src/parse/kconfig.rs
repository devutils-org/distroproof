//! Configuration du noyau (`/proc/config.gz`, `/boot/config-*`).

use std::collections::BTreeMap;
use std::io::{self, Read};

use flate2::read::GzDecoder;

/// Associe chaque option à sa valeur ; `# CONFIG_X is not set` donne `n`.
pub fn parse(text: &str) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("# ") {
            if let Some(name) = rest.strip_suffix(" is not set") {
                if name.starts_with("CONFIG_") {
                    map.insert(name.to_string(), "n".to_string());
                }
            }
        } else if line.starts_with("CONFIG_") {
            if let Some((key, value)) = line.split_once('=') {
                map.insert(key.to_string(), value.trim_matches('"').to_string());
            }
        }
    }
    map
}

pub fn parse_gz(data: &[u8]) -> io::Result<BTreeMap<String, String>> {
    let mut text = String::new();
    GzDecoder::new(data).read_to_string(&mut text)?;
    Ok(parse(&text))
}
