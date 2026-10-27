//! An ssh target, end to end, on this machine: `DISTROPROOF_SSH` names a
//! stand-in that drops ssh's options and host and runs the command here, as
//! root of a user namespace. What a real host would see is the same payload,
//! agent and cleanup.

use std::path::{Path, PathBuf};
use std::process::Command;

const BINARY: &str = env!("CARGO_BIN_EXE_distroproof");

const FAKE_SSH: &str = r#"#!/bin/sh
while [ $# -gt 0 ]; do
    case $1 in
        --) shift; shift; break ;;
        -o|-p|-i) shift 2 ;;
        *) shift ;;
    esac
done
exec unshare -r sh -c "$*"
"#;

/// The static envprobe the workspace builds, which every target is sent.
fn envprobe() -> Option<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let path = root.join(format!(
        "target/{}-unknown-linux-musl/release/envprobe",
        std::env::consts::ARCH
    ));
    path.is_file().then_some(path)
}

/// A fresh directory with the stand-in, the matrix and a file for the payload.
fn project(name: &str, matrix: &str) -> Option<(PathBuf, PathBuf)> {
    if !Command::new("unshare")
        .args(["-r", "true"])
        .status()
        .is_ok_and(|status| status.success())
    {
        eprintln!("skipped: no unprivileged user namespace for the stand-in's root");
        return None;
    }
    let Some(envprobe) = envprobe() else {
        eprintln!(
            "skipped: no static envprobe; build it with \
             `cargo zigbuild --release --target x86_64-unknown-linux-musl -p envprobe`"
        );
        return None;
    };
    let dir = std::env::temp_dir().join(format!("dp-ssh-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("remote-tmp")).unwrap();
    let ssh = dir.join("fake-ssh");
    std::fs::write(&ssh, FAKE_SSH).unwrap();
    std::fs::set_permissions(&ssh, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    std::fs::write(dir.join("distroproof.toml"), matrix).unwrap();
    std::fs::write(dir.join("data.txt"), "sent with the payload\n").unwrap();
    Some((dir, envprobe))
}

fn up(dir: &Path, envprobe: &Path) -> serde_json::Value {
    let out = Command::new(BINARY)
        .current_dir(dir)
        .env("DISTROPROOF_SSH", dir.join("fake-ssh"))
        // What the stand-in's "host" uses for its work directory: this
        // test's own, so that what the run leaves behind can be seen.
        .env("TMPDIR", dir.join("remote-tmp"))
        .args(["up", "--no-tui", "--json", "report.json", "--envprobe"])
        .arg(envprobe)
        .output()
        .expect("distroproof runs");
    let text = std::fs::read_to_string(dir.join("report.json")).unwrap_or_else(|e| {
        panic!(
            "no report ({e}):\n{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    });
    serde_json::from_str(&text).unwrap()
}

fn left_behind(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir.join("remote-tmp"))
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect()
}

/// The payload arrives, the probe and the command run, the collected file
/// and the report come back, and the host keeps nothing.
#[test]
fn an_ssh_target_runs_the_job_and_leaves_nothing_behind() {
    let Some((dir, envprobe)) = project(
        "ok",
        "[[target]]\nname = \"here\"\nssh = \"tester@example.invalid\"\n\n\
         [run]\ncommand = \"echo on $DP_TARGET $DP_IMAGE; cat data.txt; echo kept > out.txt\"\n\
         collect = [\"out.txt\"]\n",
    ) else {
        return;
    };
    let report = up(&dir, &envprobe);
    let target = &report["targets"][0];
    assert_eq!(target["outcome"], "success", "{target:#}");
    assert_eq!(target["image"], "ssh");
    let stdout = target["command"]["stdout"].as_str().unwrap();
    assert!(stdout.contains("on here ssh"), "{stdout}");
    assert!(stdout.contains("sent with the payload"), "{stdout}");
    assert!(
        target["envprobe"].is_object(),
        "the probe's report came back"
    );
    assert_eq!(
        std::fs::read_to_string(dir.join(".distroproof/collect/here/out.txt")).unwrap(),
        "kept\n"
    );
    assert!(left_behind(&dir).is_empty(), "{:?}", left_behind(&dir));
    std::fs::remove_dir_all(&dir).unwrap();
}

/// A command that outlasts its timeout is stopped with everything it
/// started, and its directory removed.
#[test]
fn a_timed_out_ssh_target_stops_what_it_started() {
    let Some((dir, envprobe)) = project(
        "timeout",
        "[[target]]\nname = \"slow\"\nssh = \"tester@example.invalid\"\ntimeout = \"2s\"\n\n\
         [run]\nprobe = false\ncommand = \"sleep 4171\"\n",
    ) else {
        return;
    };
    let report = up(&dir, &envprobe);
    let target = &report["targets"][0];
    assert_eq!(target["outcome"], "timed_out", "{target:#}");
    let still = Command::new("pgrep")
        .args(["-f", "^sleep 4171$"])
        .output()
        .unwrap();
    assert!(
        still.stdout.is_empty(),
        "the command's own child survived: {}",
        String::from_utf8_lossy(&still.stdout)
    );
    assert!(left_behind(&dir).is_empty(), "{:?}", left_behind(&dir));
    std::fs::remove_dir_all(&dir).unwrap();
}
