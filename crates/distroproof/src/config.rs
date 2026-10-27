//! Declarative `distroproof.toml` file: the project's test matrix.
//!
//! Every error (syntax, unknown key, nonexistent image, invalid duration…)
//! is reported with its line and column, before any VM is started.

use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;
use toml::Spanned;

use crate::images;
use crate::model::{SshHost, VmSpec};

pub const DEFAULT_FILE: &str = "distroproof.toml";

/// Default exclusions for the working directory copied into the guest.
pub const DEFAULT_EXCLUDE: &[&str] = &[".git", "target", ".distroproof"];

/// Capabilities that can be granted to the test user.
const CAPABILITIES: &[&str] = &[
    "audit_control",
    "audit_read",
    "audit_write",
    "block_suspend",
    "bpf",
    "checkpoint_restore",
    "chown",
    "dac_override",
    "dac_read_search",
    "fowner",
    "fsetid",
    "ipc_lock",
    "ipc_owner",
    "kill",
    "lease",
    "linux_immutable",
    "mac_admin",
    "mac_override",
    "mknod",
    "net_admin",
    "net_bind_service",
    "net_broadcast",
    "net_raw",
    "perfmon",
    "setfcap",
    "setgid",
    "setpcap",
    "setuid",
    "sys_admin",
    "sys_boot",
    "sys_chroot",
    "sys_module",
    "sys_nice",
    "sys_pacct",
    "sys_ptrace",
    "sys_rawio",
    "sys_resource",
    "sys_time",
    "sys_tty_config",
    "syslog",
    "wake_alarm",
];

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawDefaults {
    memory: Option<Spanned<String>>,
    cpus: Option<Spanned<i64>>,
    timeout: Option<Spanned<String>>,
    as_user: Option<Spanned<String>>,
    caps: Option<Spanned<Vec<String>>>,
    pmu: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTarget {
    name: Spanned<String>,
    image: Option<Spanned<String>>,
    /// A machine already running, reached over SSH (`user@host`, or a host
    /// alias of `~/.ssh/config`), instead of an image booted in a VM.
    ssh: Option<Spanned<String>>,
    port: Option<Spanned<i64>>,
    identity: Option<Spanned<String>>,
    memory: Option<Spanned<String>>,
    cpus: Option<Spanned<i64>>,
    timeout: Option<Spanned<String>>,
    #[serde(default)]
    sysctl: BTreeMap<Spanned<String>, Spanned<toml::Value>>,
    kernel_cmdline: Option<Spanned<String>>,
    as_user: Option<Spanned<String>>,
    caps: Option<Spanned<Vec<String>>>,
    pmu: Option<bool>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawRun {
    setup: Option<String>,
    command: Option<String>,
    as_user: Option<Spanned<String>>,
    workdir: Option<Spanned<String>>,
    exclude: Option<Vec<String>>,
    #[serde(default)]
    include: Vec<String>,
    probe: Option<bool>,
    #[serde(default)]
    env: BTreeMap<Spanned<String>, String>,
    #[serde(default)]
    file_caps: BTreeMap<String, Spanned<Vec<String>>>,
    #[serde(default)]
    collect: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    #[serde(default)]
    defaults: RawDefaults,
    #[serde(default)]
    target: Vec<RawTarget>,
    run: Option<Spanned<RawRun>>,
}

/// Error located in the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigError {
    pub path: PathBuf,
    pub line: usize,
    pub column: usize,
    pub message: String,
    /// Offending line and underline.
    snippet: Option<(String, usize, usize)>,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}:{}:{}: {}",
            self.path.display(),
            self.line,
            self.column,
            self.message
        )?;
        if let Some((text, start, len)) = &self.snippet {
            let num = self.line.to_string();
            let pad = " ".repeat(num.len());
            write!(
                f,
                "\n{pad} |\n{num} | {text}\n{pad} | {}{}",
                " ".repeat(*start),
                "^".repeat((*len).max(1))
            )?;
        }
        Ok(())
    }
}

struct Ctx<'a> {
    path: &'a Path,
    text: &'a str,
}

impl Ctx<'_> {
    fn error(&self, span: Range<usize>, message: impl Into<String>) -> ConfigError {
        let start = span.start.min(self.text.len());
        let before = &self.text[..start];
        let line = before.matches('\n').count() + 1;
        let line_start = before.rfind('\n').map_or(0, |i| i + 1);
        let line_end = self.text[start..]
            .find('\n')
            .map_or(self.text.len(), |i| start + i);
        let line_text = &self.text[line_start..line_end];
        let column = self.text[line_start..start].chars().count() + 1;
        let len = self.text[start..span.end.clamp(start, line_end)]
            .chars()
            .count();
        ConfigError {
            path: self.path.to_path_buf(),
            line,
            column,
            message: message.into(),
            snippet: Some((line_text.to_string(), column - 1, len)),
        }
    }
}

/// Validated matrix.
#[derive(Debug, Clone)]
pub struct Config {
    pub targets: Vec<VmSpec>,
}

pub fn load(path: &Path) -> Result<Config, ConfigError> {
    let text = std::fs::read_to_string(path).map_err(|e| ConfigError {
        path: path.to_path_buf(),
        line: 0,
        column: 0,
        message: format!("cannot read: {e}"),
        snippet: None,
    })?;
    parse(path, &text)
}

pub fn parse(path: &Path, text: &str) -> Result<Config, ConfigError> {
    let ctx = Ctx { path, text };
    let raw: RawConfig = toml::from_str(text).map_err(|e| {
        let span = e.span().unwrap_or(0..0);
        ctx.error(span, e.message().trim().to_string())
    })?;
    let base = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));

    let d = &raw.defaults;
    let def_memory = match &d.memory {
        Some(m) => parse_size_mib(m.get_ref()).map_err(|e| ctx.error(m.span(), e))?,
        None => 1024,
    };
    let def_cpus = match &d.cpus {
        Some(c) => check_cpus(c, &ctx)?,
        None => 2,
    };
    let def_timeout = match &d.timeout {
        Some(t) => parse_duration(t.get_ref()).map_err(|e| ctx.error(t.span(), e))?,
        None => Duration::from_secs(300),
    };
    if let Some(user) = &d.as_user {
        check_user(user, &ctx)?;
    }
    if let Some(caps) = &d.caps {
        check_caps(caps, &ctx)?;
    }

    let empty_run = Spanned::new(0..0, RawRun::default());
    let run_spanned = raw.run.as_ref().unwrap_or(&empty_run);
    let run = run_spanned.get_ref();
    if let Some(user) = &run.as_user {
        check_user(user, &ctx)?;
    }
    let probe = run.probe.unwrap_or(true);
    if run.command.is_none() && !probe {
        return Err(ctx.error(
            run_spanned.span(),
            "nothing to run: [run] has neither command nor probe = true",
        ));
    }
    let workdir = match &run.workdir {
        Some(w) => {
            let dir = base.join(w.get_ref());
            if !dir.is_dir() {
                return Err(ctx.error(w.span(), format!("directory not found: {}", dir.display())));
            }
            dir
        }
        None => base.to_path_buf(),
    };
    let mut env = BTreeMap::new();
    for (k, v) in &run.env {
        if !valid_env_name(k.get_ref()) {
            return Err(ctx.error(
                k.span(),
                format!("invalid variable name \"{}\"", k.get_ref()),
            ));
        }
        env.insert(k.get_ref().clone(), v.clone());
    }

    if raw.target.is_empty() {
        return Err(ctx.error(0..0, "no targets: add at least one [[target]] block"));
    }
    let mut names = HashSet::new();
    let mut targets = Vec::new();
    for t in &raw.target {
        let name = t.name.get_ref();
        if name.is_empty()
            || !name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
        {
            return Err(ctx.error(
                t.name.span(),
                "invalid target name (letters, digits, \".\", \"_\" and \"-\")",
            ));
        }
        if !names.insert(name.clone()) {
            return Err(ctx.error(t.name.span(), format!("target \"{name}\" already defined")));
        }
        let ssh = match (&t.image, &t.ssh) {
            (Some(image), Some(_)) => {
                return Err(ctx.error(
                    image.span(),
                    "a target is either an image or an ssh host, not both",
                ))
            }
            (None, None) => {
                return Err(ctx.error(
                    t.name.span(),
                    format!("target \"{name}\" has neither image nor ssh"),
                ))
            }
            (Some(image), None) => {
                if images::find(image.get_ref()).is_none() {
                    return Err(ctx.error(
                        image.span(),
                        format!(
                            "unknown image \"{}\" (known: {})",
                            image.get_ref(),
                            images::names().join(", ")
                        ),
                    ));
                }
                if let Some(port) = &t.port {
                    return Err(ctx.error(port.span(), "port only applies to an ssh target"));
                }
                if let Some(identity) = &t.identity {
                    return Err(
                        ctx.error(identity.span(), "identity only applies to an ssh target")
                    );
                }
                None
            }
            (None, Some(host)) => Some(check_ssh(t, host, base, &ctx)?),
        };
        let memory_mib = match &t.memory {
            Some(m) => parse_size_mib(m.get_ref()).map_err(|e| ctx.error(m.span(), e))?,
            None => def_memory,
        };
        let cpus = match &t.cpus {
            Some(c) => check_cpus(c, &ctx)?,
            None => def_cpus,
        };
        let timeout = match &t.timeout {
            Some(v) => parse_duration(v.get_ref()).map_err(|e| ctx.error(v.span(), e))?,
            None => def_timeout,
        };
        let mut sysctl = BTreeMap::new();
        for (k, v) in &t.sysctl {
            let key = k.get_ref();
            if key.is_empty()
                || !key
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
                || key.starts_with('.')
                || key.contains("..")
            {
                return Err(ctx.error(k.span(), format!("invalid sysctl key \"{key}\"")));
            }
            let value = match v.get_ref() {
                toml::Value::String(s) => s.clone(),
                toml::Value::Integer(i) => i.to_string(),
                toml::Value::Boolean(b) => u8::from(*b).to_string(),
                _ => {
                    return Err(ctx.error(
                        v.span(),
                        "expected a sysctl value: string, integer or boolean",
                    ))
                }
            };
            sysctl.insert(key.clone(), value);
        }
        let kernel_cmdline = match &t.kernel_cmdline {
            Some(c) if c.get_ref().trim().is_empty() => {
                return Err(ctx.error(c.span(), "empty kernel_cmdline"))
            }
            Some(c) => Some(c.get_ref().trim().to_string()),
            None => None,
        };
        let user_spanned = t
            .as_user
            .as_ref()
            .or(run.as_user.as_ref())
            .or(d.as_user.as_ref());
        if let Some(u) = &t.as_user {
            check_user(u, &ctx)?;
        }
        let user = user_spanned.map_or_else(|| "root".to_string(), |u| u.get_ref().clone());
        let caps_spanned = t.caps.as_ref().or(d.caps.as_ref());
        let caps = match caps_spanned {
            Some(c) => {
                check_caps(c, &ctx)?;
                if user == "root" && !c.get_ref().is_empty() {
                    return Err(
                        ctx.error(c.span(), "caps only applies to a non-root user (as_user)")
                    );
                }
                c.get_ref().iter().map(|s| normalize_cap(s)).collect()
            }
            None => Vec::new(),
        };
        let mut file_caps = BTreeMap::new();
        for (path, caps) in &run.file_caps {
            if !path.starts_with('/') {
                return Err(ctx.error(
                    caps.span(),
                    format!("file_caps: `{path}` is not an absolute path"),
                ));
            }
            check_caps(caps, &ctx)?;
            file_caps.insert(
                path.clone(),
                caps.get_ref().iter().map(|c| normalize_cap(c)).collect(),
            );
        }
        targets.push(VmSpec {
            name: name.clone(),
            image: t
                .image
                .as_ref()
                .map_or_else(|| SSH_IMAGE.to_string(), |i| i.get_ref().clone()),
            ssh,
            memory_mib,
            cpus,
            timeout,
            sysctl,
            kernel_cmdline,
            user,
            caps,
            pmu: t.pmu.or(d.pmu).unwrap_or(true),
            setup: run.setup.clone(),
            command: run.command.clone(),
            env: env.clone(),
            workdir: Some(workdir.clone()),
            exclude: run
                .exclude
                .clone()
                .unwrap_or_else(|| DEFAULT_EXCLUDE.iter().map(|s| s.to_string()).collect()),
            include: run.include.clone(),
            probe,
            hold: false,
            file_caps,
            collect: run.collect.clone(),
        });
    }
    Ok(Config { targets })
}

/// What an ssh target's `image` reads as, in reports and in `DP_IMAGE`.
pub const SSH_IMAGE: &str = "ssh";

/// An ssh target: its host, port and key, and none of the settings that only
/// describe a VM -- a running machine's memory, kernel and PMU are its own,
/// and a sysctl written there would outlive the run.
fn check_ssh(
    t: &RawTarget,
    host: &Spanned<String>,
    base: &Path,
    ctx: &Ctx,
) -> Result<SshHost, ConfigError> {
    let h = host.get_ref();
    if h.is_empty()
        || h.starts_with('-')
        || !h
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._-@:[]".contains(c))
    {
        return Err(ctx.error(host.span(), format!("invalid ssh host \"{h}\"")));
    }
    let vm_only = [
        ("memory", t.memory.as_ref().map(Spanned::span)),
        ("cpus", t.cpus.as_ref().map(Spanned::span)),
        (
            "kernel_cmdline",
            t.kernel_cmdline.as_ref().map(Spanned::span),
        ),
        ("sysctl", t.sysctl.keys().next().map(Spanned::span)),
    ];
    for (key, span) in vm_only {
        if let Some(span) = span {
            return Err(ctx.error(
                span,
                format!(
                    "{key} describes a VM; an ssh target is a machine already running, \
                     whose {key} is its own"
                ),
            ));
        }
    }
    if t.pmu.is_some() {
        return Err(ctx.error(
            host.span(),
            "pmu describes a VM; an ssh target's PMU is its own",
        ));
    }
    let port = match &t.port {
        Some(p) => Some(
            u16::try_from(*p.get_ref())
                .ok()
                .filter(|p| *p > 0)
                .ok_or_else(|| ctx.error(p.span(), "port must be between 1 and 65535"))?,
        ),
        None => None,
    };
    let identity = match &t.identity {
        Some(i) => {
            let raw = i.get_ref();
            let path = match raw.strip_prefix("~/") {
                Some(rest) => crate::dirs::home().join(rest),
                None => base.join(raw),
            };
            if !path.is_file() {
                return Err(ctx.error(i.span(), format!("key not found: {}", path.display())));
            }
            Some(path)
        }
        None => None,
    };
    Ok(SshHost {
        host: h.clone(),
        port,
        identity,
    })
}

fn check_cpus(c: &Spanned<i64>, ctx: &Ctx) -> Result<u32, ConfigError> {
    match u32::try_from(*c.get_ref()) {
        Ok(n @ 1..=64) => Ok(n),
        _ => Err(ctx.error(c.span(), "cpus must be between 1 and 64")),
    }
}

fn check_user(u: &Spanned<String>, ctx: &Ctx) -> Result<(), ConfigError> {
    if valid_user(u.get_ref()) {
        Ok(())
    } else {
        Err(ctx.error(u.span(), format!("invalid user name \"{}\"", u.get_ref())))
    }
}

pub fn valid_user(u: &str) -> bool {
    let mut chars = u.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_lowercase() || c == '_')
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
        && u.len() <= 32
}

fn valid_env_name(k: &str) -> bool {
    let mut chars = k.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

pub fn normalize_cap(c: &str) -> String {
    let c = c.trim().to_ascii_lowercase();
    c.strip_prefix("cap_").unwrap_or(&c).to_string()
}

fn check_caps(c: &Spanned<Vec<String>>, ctx: &Ctx) -> Result<(), ConfigError> {
    for cap in c.get_ref() {
        check_cap(cap).map_err(|e| ctx.error(c.span(), e))?;
    }
    Ok(())
}

pub fn check_cap(cap: &str) -> Result<(), String> {
    if CAPABILITIES.contains(&normalize_cap(cap).as_str()) {
        Ok(())
    } else {
        Err(format!("unknown capability \"{cap}\""))
    }
}

/// `512M`, `1G`, `1.5G`, `2048` (MiB by default).
pub fn parse_size_mib(s: &str) -> Result<u64, String> {
    let t = s.trim();
    let (num, mult) = match t.char_indices().find(|(_, c)| c.is_ascii_alphabetic()) {
        Some((i, _)) => {
            let unit = t[i..].trim().to_ascii_uppercase();
            let mult = match unit.trim_end_matches("IB").trim_end_matches('B') {
                "M" => 1.0,
                "G" => 1024.0,
                "T" => 1024.0 * 1024.0,
                _ => return Err(format!("unknown size unit in \"{s}\" (M or G)")),
            };
            (&t[..i], mult)
        }
        None => (t, 1.0),
    };
    let n: f64 = num
        .trim()
        .parse()
        .map_err(|_| format!("invalid size \"{s}\" (for example 512M or 2G)"))?;
    let mib = (n * mult).round() as u64;
    if mib < 128 {
        return Err(format!("{s}: at least 128M of memory"));
    }
    Ok(mib)
}

/// `90s`, `5m`, `1h30m`, `300` (seconds).
pub fn parse_duration(s: &str) -> Result<Duration, String> {
    let t = s.trim();
    let err = || format!("invalid duration \"{s}\" (for example 90s, 5m, 1h30m)");
    if t.is_empty() {
        return Err(err());
    }
    if let Ok(secs) = t.parse::<u64>() {
        return Ok(Duration::from_secs(secs));
    }
    let mut total = 0u64;
    let mut num = String::new();
    for c in t.chars() {
        if c.is_ascii_digit() {
            num.push(c);
        } else {
            let n: u64 = num.parse().map_err(|_| err())?;
            num.clear();
            total += n * match c {
                's' => 1,
                'm' => 60,
                'h' => 3600,
                _ => return Err(err()),
            };
        }
    }
    if !num.is_empty() || total == 0 {
        return Err(err());
    }
    Ok(Duration::from_secs(total))
}

pub fn format_duration(d: Duration) -> String {
    let secs = d.as_secs();
    match (secs / 3600, secs % 3600 / 60, secs % 60) {
        (0, 0, s) => format!("{s}s"),
        (0, m, 0) => format!("{m}m"),
        (0, m, s) => format!("{m}m{s}s"),
        (h, 0, 0) => format!("{h}h"),
        (h, m, _) => format!("{h}h{m}m"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_str(text: &str) -> Result<Config, ConfigError> {
        parse(Path::new("distroproof.toml"), text)
    }

    #[test]
    fn roadmap_example_parses() {
        let cfg = parse_str(
            r#"
[defaults]
memory = "1G"
cpus = 2
timeout = "5m"

[[target]]
name = "rhel8"
image = "rocky-8"

[[target]]
name = "ubuntu-paranoid"
image = "ubuntu-22.04"
sysctl = { "kernel.perf_event_paranoid" = "3" }

[[target]]
name = "fedora-lockdown"
image = "fedora-latest"
kernel_cmdline = "lockdown=confidentiality"

[run]
setup = "./scripts/install-deps.sh"
command = "./target/release/mytool --selftest"
as_user = "test"
"#,
        )
        .unwrap();
        assert_eq!(cfg.targets.len(), 3);
        let t = &cfg.targets[1];
        assert_eq!(t.memory_mib, 1024);
        assert_eq!(t.timeout, Duration::from_secs(300));
        assert_eq!(t.user, "test");
        assert_eq!(t.sysctl["kernel.perf_event_paranoid"], "3");
        assert_eq!(
            cfg.targets[2].kernel_cmdline.as_deref(),
            Some("lockdown=confidentiality")
        );
        assert!(t.probe);
    }

    #[test]
    fn errors_point_to_line_and_column() {
        let err = parse_str(
            "[[target]]\nname = \"a\"\nimage = \"rocky-7\"\n\n[run]\ncommand = \"true\"\n",
        )
        .unwrap_err();
        assert_eq!((err.line, err.column), (3, 9));
        assert!(err.message.contains("unknown image"));
        let shown = err.to_string();
        assert!(shown.contains("distroproof.toml:3:9"));
        assert!(shown.contains("^^^^^^^^^"), "{shown}");
    }

    #[test]
    fn unknown_keys_and_syntax_errors() {
        let err = parse_str("[[target]]\nname = \"a\"\nimage = \"alpine\"\nmemroy = \"1G\"\n")
            .unwrap_err();
        assert_eq!(err.line, 4);
        assert!(err.message.contains("memroy"), "{}", err.message);
        let err = parse_str("[[target]\n").unwrap_err();
        assert_eq!(err.line, 1);
    }

    #[test]
    fn semantic_checks() {
        let base = "[[target]]\nname = \"a\"\nimage = \"alpine\"\n";
        let dup = format!("{base}{base}");
        assert!(parse_str(&dup)
            .unwrap_err()
            .message
            .contains("already defined"));
        let bad_timeout = format!("{base}timeout = \"5 minutes\"\n");
        assert!(parse_str(&bad_timeout)
            .unwrap_err()
            .message
            .contains("duration"));
        let caps_root = format!("{base}caps = [\"bpf\"]\n");
        assert!(parse_str(&caps_root)
            .unwrap_err()
            .message
            .contains("non-root"));
        let caps_user = format!("{base}caps = [\"CAP_BPF\", \"perfmon\"]\nas_user = \"test\"\n");
        assert_eq!(
            parse_str(&caps_user).unwrap().targets[0].caps,
            ["bpf", "perfmon"]
        );
        let bad_cap = format!("{base}caps = [\"fly\"]\nas_user = \"test\"\n");
        assert!(parse_str(&bad_cap).unwrap_err().message.contains("fly"));
        assert!(parse_str("").unwrap_err().message.contains("no targets"));
        let nothing = format!("{base}[run]\nprobe = false\n");
        assert!(parse_str(&nothing)
            .unwrap_err()
            .message
            .contains("nothing to run"));
        let int_sysctl = format!("{base}sysctl = {{ \"kernel.perf_event_paranoid\" = 3 }}\n");
        assert_eq!(
            parse_str(&int_sysctl).unwrap().targets[0].sysctl["kernel.perf_event_paranoid"],
            "3"
        );
    }

    /// An ssh target names a host, and is refused every setting that only
    /// describes a VM, at its own line.
    #[test]
    fn ssh_targets() {
        let ok = parse_str(
            "[[target]]\nname = \"metal\"\nssh = \"ubuntu@203.0.113.10\"\nport = 2222\n\
             timeout = \"30m\"\nas_user = \"test\"\n[run]\ncommand = \"true\"\n",
        )
        .unwrap();
        let spec = &ok.targets[0];
        assert_eq!(spec.image, SSH_IMAGE);
        let host = spec.ssh.as_ref().unwrap();
        assert_eq!(
            (host.host.as_str(), host.port, host.identity.is_none()),
            ("ubuntu@203.0.113.10", Some(2222), true)
        );
        let refused = |extra: &str, says: &str| {
            let text = format!(
                "[[target]]\nname = \"a\"\nssh = \"h\"\n{extra}\n[run]\ncommand = \"true\"\n"
            );
            let err = parse_str(&text).unwrap_err();
            assert!(err.message.contains(says), "{extra}: {}", err.message);
            assert_eq!(
                err.line, 4,
                "{extra}: the error is at the setting's own line"
            );
        };
        refused("memory = \"4G\"", "describes a VM");
        refused("cpus = 8", "describes a VM");
        refused("kernel_cmdline = \"quiet\"", "describes a VM");
        refused(
            "sysctl = { \"kernel.perf_event_paranoid\" = 1 }",
            "describes a VM",
        );
        refused("image = \"alpine\"", "either an image or an ssh host");
        refused("port = 0", "between 1 and 65535");
        refused("identity = \"no-such-key\"", "key not found");
        let neither = parse_str("[[target]]\nname = \"a\"\n[run]\ncommand = \"true\"\n");
        assert!(neither
            .unwrap_err()
            .message
            .contains("neither image nor ssh"));
        let port_on_vm = parse_str(
            "[[target]]\nname = \"a\"\nimage = \"alpine\"\nport = 22\n[run]\ncommand = \"true\"\n",
        );
        assert!(port_on_vm
            .unwrap_err()
            .message
            .contains("only applies to an ssh target"));
        for host in ["-oProxyCommand=x", "a b", ""] {
            let text =
                format!("[[target]]\nname = \"a\"\nssh = \"{host}\"\n[run]\ncommand = \"true\"\n");
            assert!(
                parse_str(&text)
                    .unwrap_err()
                    .message
                    .contains("invalid ssh host"),
                "{host}"
            );
        }
    }

    #[test]
    fn file_caps_and_collect() {
        let base = "[[target]]\nname = \"a\"\nimage = \"alpine\"\n";
        let ok = format!(
            "{base}[run]\ncommand = \"true\"\ncollect = [\"out/*.json\"]\n\
             file_caps = {{ \"/opt/omn/bin/omn\" = [\"CAP_BPF\", \"perfmon\"] }}\n"
        );
        let spec = &parse_str(&ok).unwrap().targets[0];
        assert_eq!(spec.collect, ["out/*.json"]);
        assert_eq!(spec.file_caps["/opt/omn/bin/omn"], ["bpf", "perfmon"]);
        let relative =
            format!("{base}[run]\ncommand = \"true\"\nfile_caps = {{ \"omn\" = [\"bpf\"] }}\n");
        assert!(parse_str(&relative)
            .unwrap_err()
            .message
            .contains("not an absolute path"));
        let bad =
            format!("{base}[run]\ncommand = \"true\"\nfile_caps = {{ \"/x\" = [\"fly\"] }}\n");
        assert!(parse_str(&bad).unwrap_err().message.contains("fly"));
    }

    #[test]
    fn sizes_and_durations() {
        assert_eq!(parse_size_mib("1G").unwrap(), 1024);
        assert_eq!(parse_size_mib("512M").unwrap(), 512);
        assert_eq!(parse_size_mib("1.5GiB").unwrap(), 1536);
        assert_eq!(parse_size_mib("2048").unwrap(), 2048);
        assert!(parse_size_mib("64M").is_err());
        assert!(parse_size_mib("1X").is_err());
        assert_eq!(parse_duration("5m").unwrap(), Duration::from_secs(300));
        assert_eq!(parse_duration("1h30m").unwrap(), Duration::from_secs(5400));
        assert_eq!(parse_duration("45").unwrap(), Duration::from_secs(45));
        assert!(parse_duration("5x").is_err());
        assert!(parse_duration("m").is_err());
        assert_eq!(format_duration(Duration::from_secs(330)), "5m30s");
        assert_eq!(format_duration(Duration::from_secs(300)), "5m");
    }
}
