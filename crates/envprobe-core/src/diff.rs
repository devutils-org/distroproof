//! Différences entre deux rapports.

use crate::{Report, Section, Value};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    /// Clé présente seulement dans le second rapport.
    Added {
        key: String,
        value: Value,
    },
    /// Clé présente seulement dans le premier rapport.
    Removed {
        key: String,
        value: Value,
    },
    Changed {
        key: String,
        a: Value,
        b: Value,
    },
}

impl Change {
    pub fn key(&self) -> &str {
        match self {
            Change::Added { key, .. }
            | Change::Removed { key, .. }
            | Change::Changed { key, .. } => key,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SectionDiff {
    OnlyInA(String),
    OnlyInB(String),
    Changed { name: String, changes: Vec<Change> },
}

impl SectionDiff {
    pub fn name(&self) -> &str {
        match self {
            SectionDiff::OnlyInA(name) | SectionDiff::OnlyInB(name) => name,
            SectionDiff::Changed { name, .. } => name,
        }
    }
}

/// Sections qui diffèrent, dans l'ordre du premier rapport puis du second.
/// Le marqueur `privileged` n'est pas comparé : seule la valeur compte.
pub fn diff(a: &Report, b: &Report) -> Vec<SectionDiff> {
    let mut out = Vec::new();
    for sa in &a.sections {
        match b.section(&sa.name) {
            None => out.push(SectionDiff::OnlyInA(sa.name.clone())),
            Some(sb) => {
                let changes = diff_section(sa, sb);
                if !changes.is_empty() {
                    out.push(SectionDiff::Changed {
                        name: sa.name.clone(),
                        changes,
                    });
                }
            }
        }
    }
    for sb in &b.sections {
        if a.section(&sb.name).is_none() {
            out.push(SectionDiff::OnlyInB(sb.name.clone()));
        }
    }
    out
}

fn diff_section(a: &Section, b: &Section) -> Vec<Change> {
    let mut changes = Vec::new();
    for ea in &a.entries {
        match b.entries.iter().find(|eb| eb.key == ea.key) {
            None => changes.push(Change::Removed {
                key: ea.key.clone(),
                value: ea.value.clone(),
            }),
            Some(eb) if eb.value != ea.value => changes.push(Change::Changed {
                key: ea.key.clone(),
                a: ea.value.clone(),
                b: eb.value.clone(),
            }),
            Some(_) => {}
        }
    }
    for eb in &b.entries {
        if !a.entries.iter().any(|ea| ea.key == eb.key) {
            changes.push(Change::Added {
                key: eb.key.clone(),
                value: eb.value.clone(),
            });
        }
    }
    changes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(sections: Vec<Section>) -> Report {
        Report {
            schema_version: crate::SCHEMA_VERSION,
            envprobe_version: "test".into(),
            sections,
        }
    }

    fn section(name: &str, entries: &[(&str, Value)]) -> Section {
        let mut s = Section::new(name);
        for (k, v) in entries {
            s.push(*k, v.clone());
        }
        s
    }

    #[test]
    fn identical_reports_have_no_diff() {
        let r = report(vec![section("kernel", &[("release", Value::ok("6.1"))])]);
        assert!(diff(&r, &r).is_empty());
    }

    #[test]
    fn detects_every_kind_of_change() {
        let a = report(vec![
            section(
                "libc",
                &[("kind", Value::ok("glibc")), ("old", Value::ok("x"))],
            ),
            section("only_a", &[]),
        ]);
        let mut privileged = section(
            "libc",
            &[("kind", Value::ok("musl")), ("new", Value::denied("EPERM"))],
        );
        privileged.entries[0].privileged = true;
        let b = report(vec![privileged, section("only_b", &[])]);
        let d = diff(&a, &b);
        assert_eq!(
            d,
            vec![
                SectionDiff::Changed {
                    name: "libc".into(),
                    changes: vec![
                        Change::Changed {
                            key: "kind".into(),
                            a: Value::ok("glibc"),
                            b: Value::ok("musl")
                        },
                        Change::Removed {
                            key: "old".into(),
                            value: Value::ok("x")
                        },
                        Change::Added {
                            key: "new".into(),
                            value: Value::denied("EPERM")
                        },
                    ],
                },
                SectionDiff::OnlyInA("only_a".into()),
                SectionDiff::OnlyInB("only_b".into()),
            ]
        );
    }

    #[test]
    fn privileged_flag_alone_is_not_a_change() {
        let a = report(vec![section("x", &[("k", Value::ok("v"))])]);
        let mut b = a.clone();
        b.sections[0].entries[0].privileged = true;
        assert!(diff(&a, &b).is_empty());
    }
}
