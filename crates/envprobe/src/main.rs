use std::fs;
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum};
use envprobe_core::diff::{self, Change, SectionDiff};
use envprobe_core::{Report, Value, SCHEMA_VERSION};

/// Décrit l'environnement Linux courant (userspace + kernel).
#[derive(Parser, Debug)]
#[command(version, about, args_conflicts_with_subcommands = true)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    /// Rapport JSON complet sur stdout.
    #[arg(long)]
    json: bool,

    /// Sections à sonder, séparées par des virgules (par exemple kernel,bpf).
    #[arg(long, value_delimiter = ',')]
    section: Vec<String>,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Compare deux rapports JSON ; code de sortie 1 s'ils diffèrent.
    Diff {
        a: PathBuf,
        b: PathBuf,
        /// Couleurs dans la sortie.
        #[arg(long, value_enum, default_value_t = Color::Auto)]
        color: Color,
    },
    /// Affiche le schéma JSON du rapport.
    Schema,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum Color {
    Auto,
    Always,
    Never,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Some(Command::Diff { a, b, color }) => run_diff(&a, &b, color),
        Some(Command::Schema) => {
            print_json(&envprobe_core::json_schema()).map(|()| ExitCode::SUCCESS)
        }
        None => run_probe(&cli.section, cli.json),
    };
    result.unwrap_or_else(|err| {
        eprintln!("envprobe: {err}");
        ExitCode::from(2)
    })
}

fn run_probe(sections: &[String], json: bool) -> Result<ExitCode, String> {
    let known: Vec<&str> = envprobe_core::section_names().collect();
    if let Some(bad) = sections.iter().find(|s| !known.contains(&s.as_str())) {
        return Err(format!(
            "section inconnue « {bad} » (connues : {})",
            known.join(", ")
        ));
    }
    let only: Vec<&str> = sections.iter().map(String::as_str).collect();
    let report = envprobe_core::probe(&only);
    if json {
        print_json(&report)?;
    } else {
        quiet_epipe(print_text(&report))?;
    }
    Ok(ExitCode::SUCCESS)
}

/// Un lecteur fermé (`envprobe | head`) n'est pas une erreur.
fn quiet_epipe(res: io::Result<()>) -> Result<(), String> {
    match res {
        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        res => res.map_err(|e| e.to_string()),
    }
}

fn print_json(value: &impl serde::Serialize) -> Result<(), String> {
    let text = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
    quiet_epipe(writeln!(io::stdout().lock(), "{text}"))
}

fn print_text(report: &Report) -> io::Result<()> {
    let mut out = io::stdout().lock();
    writeln!(out, "envprobe {}", report.envprobe_version)?;
    for section in &report.sections {
        writeln!(out, "\n[{}]", section.name)?;
        let width = section
            .entries
            .iter()
            .map(|e| e.key.chars().count())
            .max()
            .unwrap_or(0);
        for entry in &section.entries {
            let mark = if entry.privileged { " *" } else { "" };
            writeln!(out, "  {:<width$}  {}{mark}", entry.key, entry.value)?;
        }
    }
    writeln!(out, "\n* résultat dépendant des privilèges")
}

fn load_report(path: &Path) -> Result<Report, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("{} : {e}", path.display()))?;
    let raw: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| format!("{} : JSON invalide : {e}", path.display()))?;
    match raw
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
    {
        Some(v) if v == u64::from(SCHEMA_VERSION) => {}
        Some(v) => {
            return Err(format!(
                "{} : schema_version {v} non pris en charge (attendu {SCHEMA_VERSION})",
                path.display()
            ))
        }
        None => return Err(format!("{} : schema_version absent", path.display())),
    }
    serde_json::from_value(raw).map_err(|e| format!("{} : rapport invalide : {e}", path.display()))
}

struct Palette {
    header: &'static str,
    minus: &'static str,
    plus: &'static str,
    reset: &'static str,
}

impl Palette {
    fn new(color: Color) -> Self {
        let enabled = match color {
            Color::Always => true,
            Color::Never => false,
            Color::Auto => io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none(),
        };
        if enabled {
            Palette {
                header: "\x1b[1;36m",
                minus: "\x1b[31m",
                plus: "\x1b[32m",
                reset: "\x1b[0m",
            }
        } else {
            Palette {
                header: "",
                minus: "",
                plus: "",
                reset: "",
            }
        }
    }
}

fn run_diff(a_path: &Path, b_path: &Path, color: Color) -> Result<ExitCode, String> {
    let a = load_report(a_path)?;
    let b = load_report(b_path)?;
    let diffs = diff::diff(&a, &b);
    let p = Palette::new(color);
    let mut out = io::stdout().lock();
    let res: io::Result<()> = (|| {
        writeln!(out, "{}--- {}{}", p.minus, a_path.display(), p.reset)?;
        writeln!(out, "{}+++ {}{}", p.plus, b_path.display(), p.reset)?;
        for d in &diffs {
            writeln!(out, "\n{}[{}]{}", p.header, d.name(), p.reset)?;
            match d {
                SectionDiff::OnlyInA(_) => writeln!(
                    out,
                    "{}- section absente du second rapport{}",
                    p.minus, p.reset
                )?,
                SectionDiff::OnlyInB(_) => writeln!(
                    out,
                    "{}+ section absente du premier rapport{}",
                    p.plus, p.reset
                )?,
                SectionDiff::Changed { changes, .. } => {
                    let width = changes
                        .iter()
                        .map(|c| c.key().chars().count())
                        .max()
                        .unwrap_or(0);
                    for change in changes {
                        print_change(&mut out, change, width, &p)?;
                    }
                }
            }
        }
        if diffs.is_empty() {
            writeln!(out, "\naucune différence")?;
        } else {
            writeln!(out, "\n{} section(s) différente(s)", diffs.len())?;
        }
        Ok(())
    })();
    quiet_epipe(res)?;
    Ok(if diffs.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    })
}

fn print_change(
    out: &mut impl Write,
    change: &Change,
    width: usize,
    p: &Palette,
) -> io::Result<()> {
    let line = |out: &mut dyn Write, sign: char, color: &str, key: &str, value: &Value| {
        writeln!(out, "{color}{sign} {key:<width$}  {value}{}", p.reset)
    };
    match change {
        Change::Removed { key, value } => line(out, '-', p.minus, key, value),
        Change::Added { key, value } => line(out, '+', p.plus, key, value),
        Change::Changed { key, a, b } => {
            line(out, '-', p.minus, key, a)?;
            line(out, '+', p.plus, key, b)
        }
    }
}
