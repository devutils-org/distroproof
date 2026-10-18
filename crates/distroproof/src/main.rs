//! distroproof: runs a program in VMs booted on the real kernel of each
//! target distro, and collects its output.

mod config;
mod dirs;
mod event;
mod guest;
mod images;
mod iso;
mod matrix;
mod metrics;
mod model;
mod progress;
mod qemu;
mod report;
mod sys;
mod tui;
mod vm;

use std::collections::BTreeMap;
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};
use tokio::sync::mpsc;

use crate::event::Emitter;
use crate::model::{Outcome, VmSpec};
use crate::progress::{fmt_bytes, Progress};
use crate::vm::Runner;

/// Runs a program across a matrix of distro VMs, each one on its own
/// kernel.
#[derive(Parser, Debug)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Runs a command in a VM and returns its output and exit code.
    Run(RunArgs),
    /// Runs the whole matrix described in distroproof.toml.
    Up(UpArgs),
    /// Validates distroproof.toml without starting any VM.
    Check {
        /// Matrix file.
        #[arg(short, long, default_value = config::DEFAULT_FILE)]
        file: PathBuf,
    },
    /// Renders a JSON matrix report again in another format.
    Report {
        /// Report written by `distroproof up --json`.
        file: PathBuf,
        #[arg(long, value_enum, default_value_t = Format::Text)]
        format: Format,
    },
    /// Manages the image cache.
    Images {
        #[command(subcommand)]
        action: Option<ImagesAction>,
    },
}

#[derive(Subcommand, Debug)]
enum ImagesAction {
    /// Catalog and cache status (default).
    List,
    /// Downloads and verifies images.
    Pull {
        /// Images to download (all of them if empty).
        names: Vec<String>,
    },
    /// Empties the image cache and abandoned runs.
    Prune,
}

#[derive(Args, Debug, Clone)]
struct Common {
    /// Static envprobe binary to copy into the guest.
    #[arg(long, env = "DISTROPROOF_ENVPROBE")]
    envprobe: Option<PathBuf>,
    /// No network access: cached images only.
    #[arg(long)]
    offline: bool,
    /// Always boot from disk, without an extracted kernel.
    #[arg(long)]
    no_direct_boot: bool,
    /// Keeps the run directories (overlay, disks) for debugging.
    #[arg(long)]
    keep: bool,
    /// Progress display on stderr.
    #[arg(long, value_enum, default_value_t = Progress::Text)]
    progress: Progress,
    /// Also relays the command's output in the progress display.
    #[arg(short, long)]
    verbose: bool,
}

#[derive(Clone, Copy, Debug, clap::ValueEnum)]
enum Format {
    Text,
    Json,
    Junit,
    Markdown,
}

#[derive(Args, Debug)]
struct UpArgs {
    /// Matrix file; repeatable, one TUI workspace per file.
    #[arg(short, long, default_value = config::DEFAULT_FILE)]
    file: Vec<PathBuf>,
    /// Forces the TUI (the default when stdout is a terminal).
    #[arg(long, conflicts_with = "no_tui")]
    tui: bool,
    /// Text output even in a terminal.
    #[arg(long)]
    no_tui: bool,
    /// Keeps each VM up after the command, with a root shell on the serial
    /// console (TUI: key c).
    #[arg(long)]
    hold: bool,
    /// Number of VMs run in parallel (default: half the cores, 8 at most).
    #[arg(short, long)]
    jobs: Option<usize>,
    /// Runs only these targets (comma-separated names).
    #[arg(long, value_delimiter = ',')]
    only: Vec<String>,
    /// JSON matrix report.
    #[arg(long, value_name = "FILE")]
    json: Option<PathBuf>,
    /// JUnit XML report, for CI.
    #[arg(long, value_name = "FILE")]
    junit: Option<PathBuf>,
    /// Markdown summary, for a PR comment.
    #[arg(long, value_name = "FILE")]
    markdown: Option<PathBuf>,
    #[command(flatten)]
    common: Common,
}

#[derive(Args, Debug)]
struct RunArgs {
    /// Target image (see `distroproof images`).
    #[arg(long, short)]
    target: String,
    #[arg(long, default_value = "1G")]
    memory: String,
    #[arg(long, default_value_t = 2)]
    cpus: u32,
    #[arg(long, default_value = "10m")]
    timeout: String,
    /// User that runs the command: root or an account created on the fly.
    #[arg(long, default_value = "root")]
    as_user: String,
    /// Capabilities granted to the non-root user (bpf,perfmon…).
    #[arg(long, value_delimiter = ',')]
    caps: Vec<String>,
    /// Sysctl applied before the command (key=value), repeatable.
    #[arg(long = "sysctl", value_name = "KEY=VALUE")]
    sysctl: Vec<String>,
    /// Arguments appended to the kernel command line.
    #[arg(long)]
    kernel_cmdline: Option<String>,
    /// Hides the PMU from the guest.
    #[arg(long)]
    no_pmu: bool,
    /// Directory copied into the guest (/distroproof/work), where the command runs.
    #[arg(long)]
    workdir: Option<PathBuf>,
    /// Also runs envprobe before the command and attaches the report to the result.
    #[arg(long)]
    probe: bool,
    /// Writes the full result (JSON) to this file.
    #[arg(long)]
    result: Option<PathBuf>,
    #[command(flatten)]
    common: Common,
    /// Command to run; `envprobe` is in the guest's PATH.
    #[arg(last = true, required = true)]
    command: Vec<String>,
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    let res = match cli.command {
        Command::Run(args) => cmd_run(args).await,
        Command::Up(args) => cmd_up(args).await,
        Command::Check { file } => cmd_check(&file),
        Command::Report { file, format } => cmd_report(&file, format),
        Command::Images { action } => cmd_images(action.unwrap_or(ImagesAction::List)).await,
    };
    res.unwrap_or_else(|err| {
        eprintln!("distroproof: {err}");
        ExitCode::from(2)
    })
}

/// Looks for a static envprobe: the option, then next to distroproof, then in
/// the workspace's musl target.
fn find_envprobe(explicit: Option<&Path>) -> Result<PathBuf, String> {
    let mut candidates = Vec::new();
    if let Some(p) = explicit {
        candidates.push(p.to_path_buf());
    } else {
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                candidates.push(dir.join("envprobe"));
                for profile in ["release", "debug"] {
                    candidates
                        .push(dir.join(format!("../x86_64-unknown-linux-musl/{profile}/envprobe")));
                }
            }
        }
        candidates.push("target/x86_64-unknown-linux-musl/release/envprobe".into());
    }
    for c in &candidates {
        let Ok(data) = std::fs::read(c) else {
            continue;
        };
        let x86_64_elf = data.starts_with(b"\x7fELF") && data.get(18..20) == Some(&[62, 0]);
        let is_static = envprobe_core::parse::elf::interpreter(&data).is_none();
        if x86_64_elf && is_static {
            return Ok(c.clone());
        }
        if explicit.is_some() {
            return Err(format!("{} is not a static x86_64 binary", c.display()));
        }
    }
    Err(
        "static envprobe not found: `cargo build --release --target \
         x86_64-unknown-linux-musl -p envprobe` or --envprobe <path>"
            .into(),
    )
}

fn runner(common: &Common) -> Result<Runner, String> {
    qemu::clean_stale_runs();
    Ok(Runner {
        store: images::Store::new(common.offline),
        envprobe: find_envprobe(common.envprobe.as_deref())?,
        direct_boot: !common.no_direct_boot,
        keep: common.keep,
    })
}

fn parse_sysctls(items: &[String]) -> Result<BTreeMap<String, String>, String> {
    items
        .iter()
        .map(|s| {
            s.split_once('=')
                .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
                .ok_or_else(|| format!("--sysctl expects key=value, got \"{s}\""))
        })
        .collect()
}

async fn cmd_run(args: RunArgs) -> Result<ExitCode, String> {
    if images::find(&args.target).is_none() {
        return Err(format!(
            "unknown image \"{}\" (known: {})",
            args.target,
            images::names().join(", ")
        ));
    }
    if !config::valid_user(&args.as_user) {
        return Err(format!("invalid user name \"{}\"", args.as_user));
    }
    for cap in &args.caps {
        config::check_cap(cap)?;
    }
    if !args.caps.is_empty() && args.as_user == "root" {
        return Err("--caps only applies with --as-user <non-root>".into());
    }
    let command = args
        .command
        .iter()
        .map(|a| guest::shell_quote(a))
        .collect::<Vec<_>>()
        .join(" ");
    let spec = VmSpec {
        name: args.target.clone(),
        image: args.target.clone(),
        memory_mib: config::parse_size_mib(&args.memory)?,
        cpus: args.cpus.clamp(1, 64),
        timeout: config::parse_duration(&args.timeout)?,
        sysctl: parse_sysctls(&args.sysctl)?,
        kernel_cmdline: args.kernel_cmdline.clone(),
        user: args.as_user.clone(),
        caps: args.caps.iter().map(|c| config::normalize_cap(c)).collect(),
        pmu: !args.no_pmu,
        setup: None,
        command: Some(command),
        env: BTreeMap::new(),
        workdir: args.workdir.clone(),
        exclude: config::DEFAULT_EXCLUDE
            .iter()
            .map(|s| s.to_string())
            .collect(),
        include: Vec::new(),
        probe: args.probe,
        hold: false,
        file_caps: Default::default(),
        collect: Vec::new(),
    };
    let runner = runner(&args.common)?;
    let (tx, rx) = mpsc::unbounded_channel();
    let printer = tokio::spawn(progress::consume(
        rx,
        args.common.progress,
        args.common.verbose,
    ));
    let (ctrl_tx, mut ctrl_rx) = mpsc::unbounded_channel();
    let ev = Emitter::new(spec.name.clone(), tx);
    let run = runner.run(&spec, &ev, &mut ctrl_rx);
    let (result, _) = tokio::select! {
        r = run => r,
        _ = tokio::signal::ctrl_c() => {
            drop(ctrl_tx);
            eprintln!("distroproof: interrupted");
            return Ok(ExitCode::from(130));
        }
    };
    drop(ctrl_tx);
    drop(ev);
    let _ = printer.await;

    if let Some(path) = &args.result {
        let json = serde_json::to_string_pretty(&result).expect("serializable");
        std::fs::write(path, json).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    if let Some(cmd) = &result.command {
        let _ = io::stdout().write_all(cmd.stdout.as_bytes());
        let _ = io::stderr().write_all(cmd.stderr.as_bytes());
    }
    Ok(match result.outcome {
        Outcome::Success => ExitCode::SUCCESS,
        Outcome::Failure => ExitCode::from(result.exit_code().map_or(1, |c| c.clamp(1, 255) as u8)),
        Outcome::TimedOut => ExitCode::from(124),
        Outcome::Killed => ExitCode::from(130),
        Outcome::Error => ExitCode::from(2),
    })
}

fn load_config(path: &Path) -> Result<config::Config, String> {
    config::load(path).map_err(|e| e.to_string())
}

fn cmd_check(path: &Path) -> Result<ExitCode, String> {
    let cfg = load_config(path)?;
    let mut out = io::stdout().lock();
    let _ = writeln!(
        out,
        "{}: {} valid target(s)",
        path.display(),
        cfg.targets.len()
    );
    for t in &cfg.targets {
        let mut variants = Vec::new();
        for (k, v) in &t.sysctl {
            variants.push(format!("{k}={v}"));
        }
        if let Some(c) = &t.kernel_cmdline {
            variants.push(format!("cmdline \"{c}\""));
        }
        if t.user != "root" {
            variants.push(format!("user {}", t.user));
        }
        if !t.caps.is_empty() {
            variants.push(format!("caps {}", t.caps.join(",")));
        }
        if !t.pmu {
            variants.push("no PMU".into());
        }
        let _ = writeln!(
            out,
            "  {:<20} {:<14} {} MiB, {} CPU, timeout {}{}",
            t.name,
            t.image,
            t.memory_mib,
            t.cpus,
            config::format_duration(t.timeout),
            if variants.is_empty() {
                String::new()
            } else {
                format!(", {}", variants.join(", "))
            }
        );
    }
    Ok(ExitCode::SUCCESS)
}

fn write_reports(m: &report::MatrixReport, args: &UpArgs) -> Result<(), String> {
    let outputs = [
        (&args.json, Format::Json),
        (&args.junit, Format::Junit),
        (&args.markdown, Format::Markdown),
    ];
    for (path, format) in outputs {
        if let Some(path) = path {
            std::fs::write(path, render(m, format))
                .map_err(|e| format!("{}: {e}", path.display()))?;
        }
    }
    Ok(())
}

fn render(m: &report::MatrixReport, format: Format) -> String {
    match format {
        Format::Text => m.to_text(false),
        Format::Json => m.to_json(),
        Format::Junit => m.to_junit(),
        Format::Markdown => m.to_markdown(),
    }
}

fn cmd_report(path: &Path, format: Format) -> Result<ExitCode, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let saved: report::MatrixReport =
        serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    // The analysis is recomputed: it benefits from heuristic improvements.
    let mut m = report::MatrixReport::build(saved.targets, saved.config);
    m.generated_at = saved.generated_at;
    let out = match format {
        Format::Text => m.to_text(io::stdout().is_terminal()),
        f => render(&m, f),
    };
    let _ = io::stdout().write_all(out.as_bytes());
    Ok(if m.all_succeeded() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    })
}

fn default_jobs() -> usize {
    std::thread::available_parallelism()
        .map_or(2, |n| n.get() / 2)
        .clamp(1, 8)
}

async fn cmd_up(args: UpArgs) -> Result<ExitCode, String> {
    // Every matrix is validated before any VM starts.
    let mut specs: Vec<(VmSpec, usize)> = Vec::new();
    let mut groups = Vec::new();
    for (group, path) in args.file.iter().enumerate() {
        let cfg = load_config(path)?;
        for t in cfg.targets {
            if let Some((_, g)) = specs.iter().find(|(s, _)| s.name == t.name) {
                return Err(format!(
                    "target \"{}\" defined in both {} and {}",
                    t.name,
                    args.file[*g].display(),
                    path.display()
                ));
            }
            specs.push((t, group));
        }
        groups.push(
            path.parent()
                .and_then(|p| p.file_name())
                .filter(|_| args.file.len() > 1)
                .map_or_else(
                    || path.display().to_string(),
                    |n| n.to_string_lossy().into_owned(),
                ),
        );
    }
    if !args.only.is_empty() {
        if let Some(bad) = args
            .only
            .iter()
            .find(|n| !specs.iter().any(|(s, _)| &s.name == *n))
        {
            return Err(format!("unknown target \"{bad}\" in --only"));
        }
        specs.retain(|(s, _)| args.only.contains(&s.name));
    }
    for (s, _) in &mut specs {
        s.hold = args.hold;
    }
    let use_tui =
        args.tui || (!args.no_tui && io::stdout().is_terminal() && io::stdin().is_terminal());
    let runner = runner(&args.common)?;
    let jobs = args.jobs.unwrap_or_else(default_jobs);
    let (tx, rx) = mpsc::unbounded_channel();

    if use_tui {
        let matrix = matrix::Matrix::start(
            runner,
            specs.iter().map(|(s, _)| s.clone()).collect(),
            jobs,
            tx,
            true,
        );
        let controls = matrix.controls.clone();
        let ui = tui::run(specs, groups, controls, rx).await;
        // Quitting the TUI stops the VMs still running.
        let results = matrix.shutdown().await;
        ui.map_err(|e| format!("TUI: {e}"))?;
        return finish_up(results, &args);
    }

    let printer = tokio::spawn(progress::consume(
        rx,
        args.common.progress,
        args.common.verbose,
    ));
    let matrix = matrix::Matrix::start(
        runner,
        specs.into_iter().map(|(s, _)| s).collect(),
        jobs,
        tx,
        args.hold,
    );
    let controls = matrix.controls.clone();
    let wait = matrix.wait();
    tokio::pin!(wait);
    let results = tokio::select! {
        r = &mut wait => r,
        _ = tokio::signal::ctrl_c() => {
            eprintln!("distroproof: interrupted, stopping the VMs");
            for tx in controls.values() {
                let _ = tx.send(event::Control::Kill);
            }
            drop(controls);
            wait.await
        }
    };
    let _ = printer.await;
    finish_up(results, &args)
}

fn finish_up(results: Vec<model::TargetResult>, args: &UpArgs) -> Result<ExitCode, String> {
    let config = args
        .file
        .iter()
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    let m = report::MatrixReport::build(results, Some(config));
    write_reports(&m, args)?;
    let _ = io::stdout().write_all(m.to_text(io::stdout().is_terminal()).as_bytes());
    Ok(if m.all_succeeded() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    })
}

async fn cmd_images(action: ImagesAction) -> Result<ExitCode, String> {
    match action {
        ImagesAction::List => {
            let mut out = io::stdout().lock();
            let entries = images::cache_status();
            let width = entries.iter().map(|e| e.name.len()).max().unwrap_or(0);
            for e in entries {
                let state = match &e.file {
                    Some(file) => format!(
                        "{file} ({}{})",
                        fmt_bytes(e.bytes),
                        if e.direct_kernel {
                            ", kernel extracted"
                        } else {
                            ""
                        }
                    ),
                    None => "not cached".into(),
                };
                let _ = writeln!(out, "{:<width$}  {:<45}  {state}", e.name, e.description);
            }
            let _ = writeln!(out, "\ncache: {}", dirs::images().display());
            Ok(ExitCode::SUCCESS)
        }
        ImagesAction::Pull { names } => {
            let names: Vec<String> = if names.is_empty() {
                images::names().iter().map(|s| s.to_string()).collect()
            } else {
                names
            };
            let store = images::Store::new(false);
            let (tx, rx) = mpsc::unbounded_channel();
            let printer = tokio::spawn(progress::consume(rx, Progress::Text, false));
            let mut tasks = Vec::new();
            for name in names {
                let store = store.clone();
                let ev = Emitter::new(name.clone(), tx.clone());
                tasks.push(tokio::spawn(async move {
                    let r = store.ensure(&name, &ev).await;
                    if let Err(e) = &r {
                        ev.info(format!("error: {e}"));
                    }
                    r.is_ok()
                }));
            }
            drop(tx);
            let mut ok = true;
            for t in tasks {
                ok &= t.await.unwrap_or(false);
            }
            let _ = printer.await;
            Ok(if ok {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            })
        }
        ImagesAction::Prune => {
            qemu::clean_stale_runs();
            let dir = dirs::images();
            let freed: u64 = images::cache_status().iter().map(|e| e.bytes).sum();
            if dir.exists() {
                std::fs::remove_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            }
            println!("{} freed ({})", fmt_bytes(freed), dir.display());
            Ok(ExitCode::SUCCESS)
        }
    }
}
