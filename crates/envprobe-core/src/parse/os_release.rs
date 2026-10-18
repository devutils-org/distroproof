//! `/etc/os-release` (format described in os-release(5)).

use std::collections::BTreeMap;

pub fn parse(text: &str) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        map.insert(key.trim().to_string(), unquote(value.trim()));
    }
    map
}

fn unquote(value: &str) -> String {
    let quote = match value.chars().next() {
        Some(q @ ('"' | '\'')) if value.len() >= 2 && value.ends_with(q) => q,
        _ => return value.to_string(),
    };
    let inner = &value[1..value.len() - 1];
    if quote == '\'' {
        return inner.to_string();
    }
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(next) = chars.next() {
                out.push(next);
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_and_escapes() {
        let map = parse("A=plain\nB=\"double \\\"q\\\"\"\nC='single'\n# comment\n\nD=\"\"\n");
        assert_eq!(map["A"], "plain");
        assert_eq!(map["B"], "double \"q\"");
        assert_eq!(map["C"], "single");
        assert_eq!(map["D"], "");
    }
}
