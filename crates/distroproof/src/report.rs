//! Matrix report: results table, correlation of failures with environment
//! differences, and terminal, JSON, JUnit and Markdown renderings.

use std::fmt::Write as _;

use envprobe_core::diff::{self, Change, SectionDiff};
use envprobe_core::{Report, Value};
use serde::{Deserialize, Serialize};

use crate::model::{Outcome, TargetResult};
use crate::progress::fmt_ms;

/// Entries that change from one VM to the next without saying anything about
/// the environment.
const NOISE: &[(&str, &str)] = &[
    ("kernel", "cmdline"),
    ("kernel", "version"),
    ("kernel", "config_source"),
    ("modules", "loaded"),
    ("modules", "loaded_count"),
    ("cgroups", "self"),
    ("sysctl", "kernel.perf_event_max_sample_rate"),
    ("btf", "modules"),
    ("libs", "source"),
];

fn is_noise(section: &str, key: &str) -> bool {
    NOISE.iter().any(|(s, k)| *s == section && *k == key)
}

/// Weight of a difference: how likely it is, a priori, to explain a failure.
fn weight(section: &str, key: &str) -> u32 {
    match (section, key) {
        ("btf", "vmlinux") => 10,
        ("kernel", "CONFIG_DEBUG_INFO_BTF") => 9,
        ("libc", "kind") => 9,
        ("libc", "version") => 7,
        ("libc", "interpreter") => 6,
        ("sysctl", "kernel.perf_event_paranoid" | "kernel.unprivileged_bpf_disabled") => 8,
        ("sysctl", _) => 5,
        ("security", "lockdown") => 8,
        ("security", _) => 5,
        ("bpf", "syscall") => 8,
        ("bpf", k)
            if k.starts_with("prog.") || k.starts_with("map.") || k.starts_with("helper.") =>
        {
            6
        }
        ("bpf", _) => 5,
        ("perf", _) => 7,
        ("privileges", k) if k.starts_with("CAP_") => 7,
        ("privileges", "uid" | "euid") => 6,
        ("privileges", "seccomp") => 5,
        ("privileges", _) => 4,
        ("libs", _) => 6,
        ("tracing", _) => 5,
        ("kernel", k) if k.starts_with("CONFIG_") => 5,
        ("kernel", "release") => 3,
        ("shell" | "cgroups", _) => 4,
        ("fhs" | "modules", _) => 3,
        ("init" | "packages", _) => 2,
        // the machine, not what it runs: rarely why a program fails
        ("identity" | "xdg" | "hardware", _) => 1,
        _ => 2,
    }
}

/// stderr keywords, the report entries they point to (`section` or
/// `section.key`, prefixes accepted) and the bonus they give: a precise word
/// ("btf") weighs more than a generic one ("bpf", found in "libbpf").
const KEYWORDS: &[(&[&str], &[&str], u32)] = &[
    (
        &["btf", "vmlinux"],
        &["btf", "kernel.CONFIG_DEBUG_INFO_BTF"],
        12,
    ),
    (
        &["perf_event", "perf event", "paranoid"],
        &["perf", "sysctl.kernel.perf_event_paranoid"],
        12,
    ),
    (&["lockdown"], &["security.lockdown"], 12),
    (
        &["glibc", "libc.so", "musl", "ld-linux", "gnu_get_libc"],
        &["libc"],
        12,
    ),
    (
        &["bpf"],
        &[
            "bpf",
            "sysctl.kernel.unprivileged_bpf_disabled",
            "privileges.CAP_BPF",
        ],
        5,
    ),
    (
        &[
            "operation not permitted",
            "permission denied",
            "eperm",
            "eacces",
        ],
        &["privileges", "sysctl", "security"],
        6,
    ),
    (
        &["kallsyms", "kprobe"],
        &[
            "tracing",
            "sysctl.kernel.kptr_restrict",
            "kernel.CONFIG_KPROBES",
        ],
        10,
    ),
    (
        &["tracefs", "debugfs", "ftrace"],
        &["tracing", "kernel.CONFIG_FTRACE"],
        10,
    ),
    (&["uprobe"], &["kernel.CONFIG_UPROBES", "tracing"], 10),
    (&["cgroup"], &["cgroups"], 10),
    (&["selinux", "avc:"], &["security.selinux"], 10),
    (&["apparmor"], &["security.apparmor"], 10),
    (
        &[
            "error while loading shared libraries",
            "cannot open shared object",
        ],
        &["libs", "libc", "fhs"],
        10,
    ),
    (
        &["not found", "no such file"],
        &["shell", "fhs", "libs", "packages"],
        4,
    ),
    (
        &["ptrace"],
        &[
            "sysctl.kernel.yama.ptrace_scope",
            "privileges.CAP_SYS_PTRACE",
        ],
        10,
    ),
];

/// Highest bonus among the keywords present in the output that point to this
/// entry.
fn keyword_hits(text: &str, section: &str, key: &str) -> Option<(&'static str, u32)> {
    let full = format!("{section}.{key}");
    KEYWORDS
        .iter()
        .filter(|(_, targets, _)| {
            targets
                .iter()
                .any(|t| *t == section || full == *t || full.starts_with(&format!("{t}.")))
        })
        .filter_map(|(words, _, boost)| {
            words
                .iter()
                .find(|w| text.contains(*w))
                .map(|w| (*w, *boost))
        })
        .max_by_key(|(_, boost)| *boost)
}

/// Maximum number of suspects shown, overall and per section: a whole section
/// disappearing (every BPF helper) does not hide the other leads.
const MAX_SUSPECTS: usize = 8;
const MAX_PER_SECTION: usize = 2;

/// Environment difference suspected of explaining a failure.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Suspect {
    pub section: String,
    pub key: String,
    /// Value on the failed target.
    pub failed: String,
    /// Value on the reference target, if it exists.
    pub reference: Option<String>,
    pub score: u32,
    pub reasons: Vec<String>,
}

impl Suspect {
    pub fn id(&self) -> String {
        format!("{}.{}", self.section, self.key)
    }

    pub fn describe(&self) -> String {
        let change = match &self.reference {
            Some(r) => format!("{r} → {}", self.failed),
            None => self.failed.clone(),
        };
        let reasons = if self.reasons.is_empty() {
            String::new()
        } else {
            format!(" [{}]", self.reasons.join(", "))
        };
        format!("{}: {change}{reasons}", self.id())
    }
}

/// Analysis of a failed target.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Analysis {
    pub target: String,
    /// Closest successful target, used as the diff reference.
    pub reference: Option<String>,
    /// Number of significant differences with the reference.
    pub differences: usize,
    pub suspects: Vec<Suspect>,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatrixReport {
    pub distroproof_version: String,
    pub generated_at: String,
    #[serde(default)]
    pub config: Option<String>,
    pub targets: Vec<TargetResult>,
    pub analysis: Vec<Analysis>,
}

/// Significant differences (noise excluded) between two reports.
fn significant(a: &Report, b: &Report) -> Vec<(String, Change)> {
    let mut out = Vec::new();
    for d in diff::diff(a, b) {
        if let SectionDiff::Changed { name, changes } = d {
            for c in changes {
                if !is_noise(&name, c.key()) {
                    out.push((name.clone(), c));
                }
            }
        }
    }
    out
}

fn show(v: &Value) -> String {
    let s = v.to_string();
    if s.chars().count() > 80 {
        format!("{}…", s.chars().take(80).collect::<String>())
    } else {
        s
    }
}

/// Ties the failure of `failed` to its differences with the closest successful target.
pub fn analyze(failed: &TargetResult, all: &[TargetResult]) -> Analysis {
    let mut analysis = Analysis {
        target: failed.name.clone(),
        reference: None,
        differences: 0,
        suspects: Vec::new(),
        note: None,
    };
    let Some(report) = &failed.envprobe else {
        analysis.note = Some("no envprobe report for this target".into());
        return analysis;
    };
    let text = format!(
        "{}\n{}\n{}",
        failed.stderr(),
        failed.command.as_ref().map_or("", |c| c.stdout.as_str()),
        failed.error.as_deref().unwrap_or("")
    )
    .to_lowercase();

    // Reference: the closest success, preferably with the same image.
    let reference = all
        .iter()
        .filter(|r| r.outcome.is_success() && r.name != failed.name)
        .filter_map(|r| r.envprobe.as_ref().map(|rep| (r, significant(rep, report))))
        .min_by_key(|(r, diffs)| (diffs.len(), r.image != failed.image));

    match reference {
        Some((reference, diffs)) => {
            analysis.reference = Some(reference.name.clone());
            analysis.differences = diffs.len();
            for (section, change) in diffs {
                let (failed_value, ref_value) = match &change {
                    Change::Changed { a, b, .. } => (b.clone(), Some(a.clone())),
                    Change::Added { value, .. } => (value.clone(), None),
                    Change::Removed { value, .. } => (
                        Value::unavailable("missing from the report"),
                        Some(value.clone()),
                    ),
                };
                let key = change.key().to_string();
                let mut score = weight(&section, &key);
                let mut reasons = Vec::new();
                let lost = !matches!(failed_value, Value::Ok(_))
                    && matches!(ref_value, Some(Value::Ok(_)));
                if lost {
                    score += 3;
                    reasons.push("unavailable here, present on the reference".to_string());
                }
                if let Some((word, boost)) = keyword_hits(&text, &section, &key) {
                    score += boost;
                    reasons.push(format!("\"{word}\" in the output"));
                }
                analysis.suspects.push(Suspect {
                    section,
                    key,
                    failed: show(&failed_value),
                    reference: ref_value.as_ref().map(show),
                    score,
                    reasons,
                });
            }
        }
        None => {
            analysis.note = Some("no successful target to compare with".into());
            // Without a reference: the missing or denied entries that the
            // program's output points to.
            for section in &report.sections {
                for entry in &section.entries {
                    if matches!(entry.value, Value::Ok(_)) || is_noise(&section.name, &entry.key) {
                        continue;
                    }
                    if let Some((word, boost)) = keyword_hits(&text, &section.name, &entry.key) {
                        analysis.suspects.push(Suspect {
                            section: section.name.clone(),
                            key: entry.key.clone(),
                            failed: show(&entry.value),
                            reference: None,
                            score: weight(&section.name, &entry.key) + boost,
                            reasons: vec![format!("\"{word}\" in the output")],
                        });
                    }
                }
            }
        }
    }
    analysis
        .suspects
        .sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.id().cmp(&b.id())));
    let mut per_section: std::collections::HashMap<String, usize> = Default::default();
    analysis.suspects.retain(|s| {
        let n = per_section.entry(s.section.clone()).or_default();
        *n += 1;
        *n <= MAX_PER_SECTION
    });
    analysis.suspects.truncate(MAX_SUSPECTS);
    analysis
}

impl MatrixReport {
    pub fn build(targets: Vec<TargetResult>, config: Option<String>) -> MatrixReport {
        let analysis = targets
            .iter()
            .filter(|t| matches!(t.outcome, Outcome::Failure | Outcome::TimedOut))
            .map(|t| analyze(t, &targets))
            .collect();
        MatrixReport {
            distroproof_version: env!("CARGO_PKG_VERSION").to_string(),
            generated_at: now_rfc3339(),
            config,
            targets,
            analysis,
        }
    }

    pub fn all_succeeded(&self) -> bool {
        self.targets.iter().all(|t| t.outcome.is_success())
    }

    pub fn analysis_for(&self, target: &str) -> Option<&Analysis> {
        self.analysis.iter().find(|a| a.target == target)
    }

    fn counts(&self) -> (usize, usize) {
        let ok = self
            .targets
            .iter()
            .filter(|t| t.outcome.is_success())
            .count();
        (ok, self.targets.len())
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("serializable report")
    }

    /// Table for the terminal, with ANSI colors if `color`.
    pub fn to_text(&self, color: bool) -> String {
        let paint = |code: &str, s: &str| {
            if color {
                format!("\x1b[{code}m{s}\x1b[0m")
            } else {
                s.to_string()
            }
        };
        let rows: Vec<[String; 6]> = self
            .targets
            .iter()
            .map(|t| {
                [
                    t.name.clone(),
                    t.image.clone(),
                    t.kernel_release().unwrap_or("?").to_string(),
                    outcome_cell(t),
                    t.boot_ms.map(fmt_ms).unwrap_or_else(|| "-".into()),
                    fmt_ms(t.duration_ms),
                ]
            })
            .collect();
        let header = ["target", "image", "kernel", "result", "boot", "duration"];
        let mut widths = header.map(|h| h.chars().count());
        for row in &rows {
            for (w, cell) in widths.iter_mut().zip(row) {
                *w = (*w).max(cell.chars().count());
            }
        }
        let mut out = String::new();
        let line = |cells: &[String; 6], out: &mut String, outcome: Option<Outcome>| {
            for (i, (cell, w)) in cells.iter().zip(widths).enumerate() {
                let padded = format!("{cell:<w$}");
                let painted = match (i, outcome) {
                    (3, Some(Outcome::Success)) => paint("32", &padded),
                    (3, Some(Outcome::Failure | Outcome::Error)) => paint("31", &padded),
                    (3, Some(_)) => paint("33", &padded),
                    _ => padded,
                };
                out.push_str(&painted);
                out.push_str("  ");
            }
            let trimmed = out.trim_end().len();
            out.truncate(trimmed);
            out.push('\n');
        };
        line(&header.map(String::from), &mut out, None);
        for (row, t) in rows.iter().zip(&self.targets) {
            line(row, &mut out, Some(t.outcome));
        }
        let (ok, total) = self.counts();
        let _ = writeln!(out, "\n{ok}/{total} targets passed");

        for t in self.targets.iter().filter(|t| !t.outcome.is_success()) {
            let _ = writeln!(
                out,
                "\n{}",
                paint("1", &format!("[{}] {}", t.name, outcome_cell(t)))
            );
            if let Some(e) = &t.error {
                let _ = writeln!(out, "  error: {e}");
            }
            if let Some(a) = self.analysis_for(&t.name) {
                write_analysis_text(&mut out, a);
            }
            let excerpt = stderr_excerpt(t, 8);
            if !excerpt.is_empty() {
                let _ = writeln!(out, "  stderr:");
                for l in excerpt.lines() {
                    let _ = writeln!(out, "    {l}");
                }
            }
        }
        out
    }

    pub fn to_markdown(&self) -> String {
        let (ok, total) = self.counts();
        let mut md = String::new();
        let status = if ok == total { "✅" } else { "❌" };
        let _ = writeln!(md, "## {status} distroproof: {ok}/{total} targets passed\n");
        let _ = writeln!(md, "| Target | Image | Kernel | Result | Boot | Duration |");
        let _ = writeln!(md, "| --- | --- | --- | --- | --- | --- |");
        for t in &self.targets {
            let icon = match t.outcome {
                Outcome::Success => "✅",
                Outcome::Failure => "❌",
                Outcome::TimedOut => "⏱️",
                Outcome::Killed => "⏹️",
                Outcome::Error => "⚠️",
            };
            let _ = writeln!(
                md,
                "| `{}` | {} | `{}` | {icon} {} | {} | {} |",
                t.name,
                t.image,
                t.kernel_release().unwrap_or("?"),
                outcome_cell(t),
                t.boot_ms.map(fmt_ms).unwrap_or_else(|| "-".into()),
                fmt_ms(t.duration_ms)
            );
        }
        for t in self.targets.iter().filter(|t| !t.outcome.is_success()) {
            let analysis = self.analysis_for(&t.name);
            let cause = analysis
                .and_then(|a| a.suspects.first())
                .map(|s| format!(" — probable cause: `{}`", s.id()))
                .unwrap_or_default();
            let _ = writeln!(
                md,
                "\n<details><summary><b>{}</b>: {}{cause}</summary>\n",
                t.name,
                outcome_cell(t)
            );
            if let Some(e) = &t.error {
                let _ = writeln!(md, "Error: {}\n", md_escape(e));
            }
            if let Some(a) = analysis {
                match &a.reference {
                    Some(r) => {
                        let _ = writeln!(
                            md,
                            "Compared with `{r}` ({} significant differences):\n",
                            a.differences
                        );
                    }
                    None => {
                        if let Some(n) = &a.note {
                            let _ = writeln!(md, "{n}\n");
                        }
                    }
                }
                for s in a.suspects.iter().take(5) {
                    let _ = writeln!(md, "- {}", md_escape(&s.describe()));
                }
                md.push('\n');
            }
            let excerpt = stderr_excerpt(t, 15);
            if !excerpt.is_empty() {
                let _ = writeln!(md, "```text\n{}\n```\n", excerpt.replace("```", "'''"));
            }
            md.push_str("</details>\n");
        }
        md
    }

    pub fn to_junit(&self) -> String {
        let failures = self
            .targets
            .iter()
            .filter(|t| matches!(t.outcome, Outcome::Failure | Outcome::TimedOut))
            .count();
        let errors = self
            .targets
            .iter()
            .filter(|t| matches!(t.outcome, Outcome::Error | Outcome::Killed))
            .count();
        let time: f64 = self
            .targets
            .iter()
            .map(|t| t.duration_ms as f64 / 1000.0)
            .sum();
        let mut x = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
        let _ = writeln!(
            x,
            "<testsuites name=\"distroproof\" tests=\"{}\" failures=\"{failures}\" errors=\"{errors}\" time=\"{time:.3}\">",
            self.targets.len()
        );
        let _ = writeln!(
            x,
            "  <testsuite name=\"distroproof\" tests=\"{}\" failures=\"{failures}\" errors=\"{errors}\" skipped=\"0\" time=\"{time:.3}\" timestamp=\"{}\">",
            self.targets.len(),
            xml(&self.generated_at)
        );
        for t in &self.targets {
            let _ = writeln!(
                x,
                "    <testcase name=\"{}\" classname=\"distroproof.{}\" time=\"{:.3}\">",
                xml(&t.name),
                xml(&t.image),
                t.duration_ms as f64 / 1000.0
            );
            let mut body = String::new();
            if let Some(a) = self.analysis_for(&t.name) {
                if let Some(r) = &a.reference {
                    let _ = writeln!(body, "compared with {r} ({} differences)", a.differences);
                }
                for s in &a.suspects {
                    let _ = writeln!(body, "suspect: {}", s.describe());
                }
            }
            if let Some(e) = &t.error {
                let _ = writeln!(body, "error: {e}");
            }
            let message = match self.analysis_for(&t.name).and_then(|a| a.suspects.first()) {
                Some(s) => format!("{}; probable cause: {}", outcome_cell(t), s.id()),
                None => outcome_cell(t),
            };
            match t.outcome {
                Outcome::Success => {}
                Outcome::Failure | Outcome::TimedOut => {
                    let _ = writeln!(
                        x,
                        "      <failure message=\"{}\" type=\"{}\">{}</failure>",
                        xml(&message),
                        if t.outcome == Outcome::TimedOut {
                            "timeout"
                        } else {
                            "failure"
                        },
                        xml(&body)
                    );
                }
                Outcome::Error | Outcome::Killed => {
                    let _ = writeln!(
                        x,
                        "      <error message=\"{}\" type=\"{}\">{}</error>",
                        xml(&message),
                        if t.outcome == Outcome::Killed {
                            "killed"
                        } else {
                            "error"
                        },
                        xml(&format!("{body}\n{}", t.console_tail))
                    );
                }
            }
            if let Some(c) = &t.command {
                if !c.stdout.is_empty() {
                    let _ = writeln!(x, "      <system-out>{}</system-out>", xml(&c.stdout));
                }
                if !c.stderr.is_empty() {
                    let _ = writeln!(x, "      <system-err>{}</system-err>", xml(&c.stderr));
                }
            }
            let _ = writeln!(x, "    </testcase>");
        }
        x.push_str("  </testsuite>\n</testsuites>\n");
        x
    }
}

fn write_analysis_text(out: &mut String, a: &Analysis) {
    match &a.reference {
        Some(r) => {
            let _ = writeln!(
                out,
                "  compared with {r}: {} significant differences",
                a.differences
            );
        }
        None => {
            if let Some(n) = &a.note {
                let _ = writeln!(out, "  {n}");
            }
        }
    }
    for (i, s) in a.suspects.iter().take(5).enumerate() {
        let label = if i == 0 { "probable cause" } else { "also" };
        let _ = writeln!(out, "  {label}: {}", s.describe());
    }
}

pub fn outcome_cell(t: &TargetResult) -> String {
    match (t.outcome, t.exit_code()) {
        (Outcome::Failure, Some(c)) if c != 0 => format!("failed (code {c})"),
        (o, _) => o.label().to_string(),
    }
}

/// Tail of the command's stderr, or failing that the error and the console.
pub fn stderr_excerpt(t: &TargetResult, lines: usize) -> String {
    let source = match &t.command {
        Some(c) if !c.stderr.trim().is_empty() => c.stderr.as_str(),
        Some(c) if t.outcome == Outcome::Failure && !c.stdout.trim().is_empty() => {
            c.stdout.as_str()
        }
        _ => match t.outcome {
            Outcome::Error | Outcome::TimedOut => t.console_tail.as_str(),
            _ => "",
        },
    };
    let all: Vec<&str> = source.lines().collect();
    all[all.len().saturating_sub(lines)..].join("\n")
}

fn md_escape(s: &str) -> String {
    s.replace('|', "\\|")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn xml(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            '\n' | '\r' | '\t' => out.push(c),
            c if (c as u32) < 0x20 => {}
            c => out.push(c),
        }
    }
    out
}

/// UTC date in RFC 3339 format, without a dependency.
pub fn now_rfc3339() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs()) as i64;
    rfc3339(secs)
}

fn rfc3339(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // Howard Hinnant's "civil from days" algorithm.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{StageResult, VmSpec};
    use envprobe_core::Section;
    use std::collections::BTreeMap;
    use std::time::Duration;

    fn spec(name: &str, image: &str) -> VmSpec {
        VmSpec {
            name: name.into(),
            image: image.into(),
            ssh: None,
            memory_mib: 1024,
            cpus: 2,
            timeout: Duration::from_secs(60),
            sysctl: BTreeMap::new(),
            kernel_cmdline: None,
            user: "root".into(),
            caps: Vec::new(),
            pmu: true,
            setup: None,
            command: Some("true".into()),
            env: BTreeMap::new(),
            workdir: None,
            exclude: Vec::new(),
            include: Vec::new(),
            probe: true,
            hold: false,
            file_caps: Default::default(),
            collect: Vec::new(),
        }
    }

    fn report(btf: Value, release: &str, paranoid: &str) -> Report {
        let mut kernel = Section::new("kernel");
        kernel.push("release", Value::ok(release));
        kernel.push("cmdline", Value::ok(format!("root=UUID={release}")));
        let mut b = Section::new("btf");
        b.push("vmlinux", btf);
        let mut s = Section::new("sysctl");
        s.push("kernel.perf_event_paranoid", Value::ok(paranoid));
        Report {
            schema_version: envprobe_core::SCHEMA_VERSION,
            envprobe_version: "test".into(),
            sections: vec![kernel, b, s],
        }
    }

    fn result(
        name: &str,
        image: &str,
        outcome: Outcome,
        rep: Report,
        stderr: &str,
    ) -> TargetResult {
        let mut r = TargetResult::new(&spec(name, image));
        r.outcome = outcome;
        r.envprobe = Some(rep);
        r.command = Some(StageResult {
            exit_code: Some(if outcome == Outcome::Success { 0 } else { 1 }),
            stdout: String::new(),
            stderr: stderr.into(),
        });
        r
    }

    #[test]
    fn btf_is_designated_as_probable_cause() {
        let ok = result(
            "ubuntu-22.04",
            "ubuntu-22.04",
            Outcome::Success,
            report(Value::ok("present, 5 MiB"), "5.15.0", "4"),
            "",
        );
        let other = result(
            "debian-12",
            "debian-12",
            Outcome::Success,
            report(Value::ok("present, 6 MiB"), "6.1.0", "3"),
            "",
        );
        let ko = result(
            "ubuntu-18.04",
            "ubuntu-18.04",
            Outcome::Failure,
            report(
                Value::unavailable("/sys/kernel/btf/vmlinux missing"),
                "4.15.0",
                "4",
            ),
            "error: kernel BTF not found\n",
        );
        let all = vec![ok, other, ko];
        let m = MatrixReport::build(all, None);
        let a = m.analysis_for("ubuntu-18.04").unwrap();
        // ubuntu-22.04 is closer (same paranoid) than debian-12.
        assert_eq!(a.reference.as_deref(), Some("ubuntu-22.04"));
        // kernel.cmdline is noise: only release and vmlinux count.
        assert_eq!(a.differences, 2);
        assert_eq!(a.suspects[0].id(), "btf.vmlinux");
        assert!(a.suspects[0].reasons.iter().any(|r| r.contains("btf")));
        let text = m.to_text(false);
        assert!(text.contains("probable cause: btf.vmlinux"), "{text}");
        let md = m.to_markdown();
        assert!(md.contains("probable cause: `btf.vmlinux`"), "{md}");
        assert!(md.contains("2/3 targets passed"));
        let junit = m.to_junit();
        assert!(junit.contains("failures=\"1\""));
        assert!(junit.contains("probable cause: btf.vmlinux"));
        assert!(!m.all_succeeded());
        let back: MatrixReport = serde_json::from_str(&m.to_json()).unwrap();
        assert_eq!(back.targets.len(), 3);
    }

    #[test]
    fn without_reference_keywords_still_point_to_cause() {
        let ko = result(
            "alone",
            "ubuntu-18.04",
            Outcome::Failure,
            report(Value::unavailable("missing"), "4.15.0", "4"),
            "libbpf: failed to find valid kernel BTF\n",
        );
        let a = analyze(&ko, std::slice::from_ref(&ko));
        assert!(a.reference.is_none());
        assert_eq!(a.suspects[0].id(), "btf.vmlinux");
    }

    #[test]
    fn xml_and_dates() {
        assert_eq!(xml("a<b & \"c\"\u{1}"), "a&lt;b &amp; &quot;c&quot;");
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339(1_791_000_000), "2026-10-03T04:00:00Z");
    }
}
