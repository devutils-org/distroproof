//! The build tools on PATH and their versions: what compiled, or would
//! compile, the program a report sits beside. Each tool found is asked its
//! version once, with a time limit; nothing else is run.

use std::env;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::{Section, Value};

/// The tool, and the argument that prints its version.
const TOOLS: &[(&str, &str)] = &[
    ("gcc", "--version"),
    ("clang", "--version"),
    ("rustc", "--version"),
    ("cargo", "--version"),
    ("go", "version"),
    ("python3", "--version"),
    ("make", "--version"),
    ("cmake", "--version"),
    ("ld", "--version"),
];

/// Longest a tool may take to say its version.
const LIMIT: Duration = Duration::from_secs(3);

pub fn probe() -> Section {
    let mut s = Section::new("toolchain");
    let path = env::var_os("PATH").unwrap_or_default();
    let dirs: Vec<PathBuf> = env::split_paths(&path).collect();
    for (tool, arg) in TOOLS {
        let value = match find(tool, &dirs) {
            None => Value::ok("absent"),
            Some(bin) => version(&bin, arg),
        };
        s.push(*tool, value);
    }
    s
}

fn find(tool: &str, dirs: &[PathBuf]) -> Option<PathBuf> {
    dirs.iter().map(|d| d.join(tool)).find(|p| p.is_file())
}

fn version(bin: &Path, arg: &str) -> Value {
    let mut child = match Command::new(bin)
        .arg(arg)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => return Value::unavailable(format!("{}: {e}", bin.display())),
    };
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if start.elapsed() < LIMIT => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Value::unavailable(format!(
                    "{} did not answer in {} s",
                    bin.display(),
                    LIMIT.as_secs()
                ));
            }
        }
    }
    // some tools (old python) print their version on stderr
    let mut text = String::new();
    if let Some(mut out) = child.stdout.take() {
        let _ = out.read_to_string(&mut text);
    }
    if text.trim().is_empty() {
        if let Some(mut err) = child.stderr.take() {
            let _ = err.read_to_string(&mut text);
        }
    }
    match first_line(&text) {
        Some(line) => Value::ok(line),
        None => Value::unavailable(format!("{} printed no version", bin.display())),
    }
}

/// The first non-empty line, trimmed: `gcc (GCC) 15.2.1 20250813`.
pub fn first_line(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_lines() {
        assert_eq!(
            first_line("\ngcc (GCC) 15.2.1 20250813\nCopyright\n").as_deref(),
            Some("gcc (GCC) 15.2.1 20250813")
        );
        assert_eq!(first_line("  \n"), None);
    }

    #[test]
    fn a_missing_tool_is_absent() {
        assert_eq!(
            find("surely-no-such-tool", &[PathBuf::from("/nonexistent")]),
            None
        );
    }
}
