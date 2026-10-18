//! Regression tests on real reports: published schema, deserialization, diff.
//!
//! Regenerate the schema: `UPDATE_SCHEMA=1 cargo test -p envprobe-core --test reports`.

use std::path::Path;

use envprobe_core::diff::{diff, SectionDiff};
use envprobe_core::{Report, Value};

const SCHEMA_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../schema/envprobe-report.schema.json"
);

const FIXTURES: &[(&str, &str)] = &[
    (
        "archarm-orbstack-user",
        include_str!("fixtures/reports/archarm-orbstack-user.json"),
    ),
    (
        "archarm-orbstack-root",
        include_str!("fixtures/reports/archarm-orbstack-root.json"),
    ),
    (
        "alpine-3.20-rootfs",
        include_str!("fixtures/reports/alpine-3.20-rootfs.json"),
    ),
    (
        "debian-12-rootfs",
        include_str!("fixtures/reports/debian-12-rootfs.json"),
    ),
];

fn load(name: &str) -> Report {
    let text = FIXTURES.iter().find(|(n, _)| *n == name).unwrap().1;
    serde_json::from_str(text).unwrap_or_else(|e| panic!("{name}: {e}"))
}

fn value<'a>(report: &'a Report, section: &str, key: &str) -> &'a Value {
    report
        .section(section)
        .and_then(|s| s.get(key))
        .unwrap_or_else(|| panic!("{section}.{key} missing"))
}

#[test]
fn published_schema_is_up_to_date() {
    let generated = serde_json::to_string_pretty(&envprobe_core::json_schema()).unwrap() + "\n";
    if std::env::var_os("UPDATE_SCHEMA").is_some() {
        std::fs::write(SCHEMA_PATH, &generated).unwrap();
    }
    let published =
        std::fs::read_to_string(Path::new(SCHEMA_PATH)).expect("published schema missing");
    assert_eq!(
        published, generated,
        "stale schema: rerun with UPDATE_SCHEMA=1"
    );
}

#[test]
fn fixtures_and_live_report_validate_against_schema() {
    let schema: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(SCHEMA_PATH).unwrap()).unwrap();
    let validator = jsonschema::validator_for(&schema).expect("valid schema");
    let live = serde_json::to_value(envprobe_core::probe(&[])).unwrap();
    let mut docs: Vec<(&str, serde_json::Value)> = FIXTURES
        .iter()
        .map(|(n, t)| (*n, serde_json::from_str(t).unwrap()))
        .collect();
    docs.push(("live", live));
    for (name, doc) in &docs {
        let errors: Vec<String> = validator.iter_errors(doc).map(|e| e.to_string()).collect();
        assert!(errors.is_empty(), "{name}: {errors:?}");
    }
    let broken = serde_json::json!({"schema_version": 1, "envprobe_version": "x", "sections": [
        {"name": "k", "entries": [{"key": "a", "status": "maybe", "value": "v", "privileged": false}]}
    ]});
    assert!(!validator.is_valid(&broken));
}

#[test]
fn fixtures_roundtrip() {
    for (name, text) in FIXTURES {
        let report: Report = serde_json::from_str(text).unwrap();
        let again: Report = serde_json::from_str(&serde_json::to_string(&report).unwrap()).unwrap();
        assert_eq!(report, again, "{name}");
        assert_eq!(report.schema_version, envprobe_core::SCHEMA_VERSION);
    }
}

#[test]
fn host_vs_alpine_differs_in_userspace_not_kernel() {
    let host = load("archarm-orbstack-user");
    let alpine = load("alpine-3.20-rootfs");
    let d = diff(&host, &alpine);
    let changed = |name: &str, key: &str| {
        d.iter().any(|s| match s {
            SectionDiff::Changed { name: n, changes } => {
                n == name && changes.iter().any(|c| c.key() == key)
            }
            _ => false,
        })
    };
    assert!(changed("libc", "kind"));
    assert!(changed("shell", "sh"));
    assert!(changed("packages", "managers"));
    assert!(
        d.iter().all(|s| s.name() != "kernel"),
        "same kernel, no difference expected"
    );
    assert!(d.iter().all(|s| s.name() != "btf"));
    assert_eq!(value(&alpine, "libc", "kind"), &Value::ok("musl"));
    assert_eq!(value(&alpine, "packages", "managers"), &Value::ok("apk"));
}

#[test]
fn debian_rootfs_report() {
    let debian = load("debian-12-rootfs");
    assert_eq!(value(&debian, "identity", "id"), &Value::ok("debian"));
    assert_eq!(value(&debian, "libc", "kind"), &Value::ok("glibc"));
    assert!(value(&debian, "libc", "version")
        .as_ok()
        .unwrap()
        .starts_with("2.36"));
    assert!(value(&debian, "fhs", "multiarch")
        .as_ok()
        .unwrap()
        .starts_with("yes"));
    assert_eq!(value(&debian, "shell", "sh"), &Value::ok("/usr/bin/dash"));
}

#[test]
fn user_vs_root_differs_on_privileges_only() {
    let user = load("archarm-orbstack-user");
    let root = load("archarm-orbstack-root");
    assert_eq!(value(&user, "bpf", "syscall"), &Value::denied("EPERM"));
    assert_eq!(value(&root, "bpf", "syscall"), &Value::ok("available"));
    assert_eq!(value(&root, "privileges", "CAP_BPF"), &Value::ok("yes"));
    for s in diff(&user, &root) {
        assert!(
            [
                "modules",
                "privileges",
                "bpf",
                "perf",
                "tracing",
                "init",
                "xdg"
            ]
            .contains(&s.name()),
            "unexpected section: {}",
            s.name()
        );
    }
}

#[test]
fn user_namespace_is_reported_in_rootfs() {
    let alpine = load("alpine-3.20-rootfs");
    assert!(value(&alpine, "privileges", "user_namespace")
        .as_ok()
        .unwrap()
        .starts_with("non-initial"));
    assert_eq!(value(&alpine, "bpf", "syscall"), &Value::denied("EPERM"));
}
