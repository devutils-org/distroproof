//! distroproof : lance un programme dans des VM qui tournent avec le vrai
//! noyau de chaque distro cible, et récupère sa sortie.

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

/// Lance un programme dans une matrice de VM de distros, chacune avec son
/// propre noyau.
#[derive(Parser, Debug)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Lance une commande dans une VM et renvoie sa sortie et son code.
    Run(RunArgs),
    /// Lance toute la matrice décrite dans distroproof.toml.
    Up(UpArgs),
    /// Valide distroproof.toml sans lancer de VM.
    Check {
        /// Fichier de matrice.
        #[arg(short, long, default_value = config::DEFAULT_FILE)]
        file: PathBuf,
    },
    /// Réaffiche un rapport de matrice JSON dans un autre format.
    Report {
        /// Rapport écrit par `distroproof up --json`.
        file: PathBuf,
        #[arg(long, value_enum, default_value_t = Format::Text)]
        format: Format,
    },
    /// Gère le cache des images.
    Images {
        #[command(subcommand)]
        action: Option<ImagesAction>,
    },
}

#[derive(Subcommand, Debug)]
enum ImagesAction {
    /// Catalogue et état du cache (défaut).
    List,
    /// Télécharge et vérifie des images.
    Pull {
        /// Images à télécharger (toutes si vide).
        names: Vec<String>,
    },
    /// Vide le cache des images et les exécutions abandonnées.
    Prune,
}

#[derive(Args, Debug, Clone)]
struct Common {
    /// Binaire envprobe statique à copier dans l'invité.
    #[arg(long, env = "DISTROPROOF_ENVPROBE")]
    envprobe: Option<PathBuf>,
    /// N'accède pas au réseau : images du cache uniquement.
    #[arg(long)]
    offline: bool,
    /// Toujours booter sur le disque, sans noyau extrait.
    #[arg(long)]
    no_direct_boot: bool,
    /// Garde les répertoires d'exécution (overlay, disques) pour déboguer.
    #[arg(long)]
    keep: bool,
    /// Affichage de la progression sur stderr.
    #[arg(long, value_enum, default_value_t = Progress::Text)]
    progress: Progress,
    /// Relaie aussi la sortie de la commande dans la progression.
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
    /// Fichier de matrice ; répétable, un workspace de TUI par fichier.
    #[arg(short, long, default_value = config::DEFAULT_FILE)]
    file: Vec<PathBuf>,
    /// Force la TUI (par défaut si stdout est un terminal).
    #[arg(long, conflicts_with = "no_tui")]
    tui: bool,
    /// Sortie texte même dans un terminal.
    #[arg(long)]
    no_tui: bool,
    /// Garde chaque VM allumée après la commande, avec un shell root sur la
    /// console série (TUI : touche c).
    #[arg(long)]
    hold: bool,
    /// Nombre de VM lancées en parallèle (défaut : moitié des cœurs, 8 au plus).
    #[arg(short, long)]
    jobs: Option<usize>,
    /// Ne lance que ces cibles (noms séparés par des virgules).
    #[arg(long, value_delimiter = ',')]
    only: Vec<String>,
    /// Rapport de matrice JSON.
    #[arg(long, value_name = "FICHIER")]
    json: Option<PathBuf>,
    /// Rapport JUnit XML, pour la CI.
    #[arg(long, value_name = "FICHIER")]
    junit: Option<PathBuf>,
    /// Résumé Markdown, pour un commentaire de PR.
    #[arg(long, value_name = "FICHIER")]
    markdown: Option<PathBuf>,
    #[command(flatten)]
    common: Common,
}

#[derive(Args, Debug)]
struct RunArgs {
    /// Image cible (voir `distroproof images`).
    #[arg(long, short)]
    target: String,
    #[arg(long, default_value = "1G")]
    memory: String,
    #[arg(long, default_value_t = 2)]
    cpus: u32,
    #[arg(long, default_value = "10m")]
    timeout: String,
    /// Utilisateur qui lance la commande : root ou un compte créé à la volée.
    #[arg(long, default_value = "root")]
    as_user: String,
    /// Capabilities accordées à l'utilisateur non root (bpf,perfmon…).
    #[arg(long, value_delimiter = ',')]
    caps: Vec<String>,
    /// Sysctl appliqué avant la commande (clé=valeur), répétable.
    #[arg(long = "sysctl", value_name = "CLÉ=VALEUR")]
    sysctl: Vec<String>,
    /// Arguments ajoutés à la ligne de commande du noyau.
    #[arg(long)]
    kernel_cmdline: Option<String>,
    /// Masque la PMU à l'invité.
    #[arg(long)]
    no_pmu: bool,
    /// Répertoire copié dans l'invité (/distroproof/work), où tourne la commande.
    #[arg(long)]
    workdir: Option<PathBuf>,
    /// Lance aussi envprobe avant la commande et joint le rapport au résultat.
    #[arg(long)]
    probe: bool,
    /// Écrit le résultat complet (JSON) dans ce fichier.
    #[arg(long)]
    result: Option<PathBuf>,
    #[command(flatten)]
    common: Common,
    /// Commande à lancer ; `envprobe` est dans le PATH de l'invité.
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

/// Cherche un envprobe statique : option, puis à côté de distroproof, puis
/// dans la cible musl du workspace.
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
            return Err(format!(
                "{} n'est pas un binaire x86_64 statique",
                c.display()
            ));
        }
    }
    Err(
        "envprobe statique introuvable : `cargo build --release --target \
         x86_64-unknown-linux-musl -p envprobe` ou --envprobe <chemin>"
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
                .ok_or_else(|| format!("--sysctl attend clé=valeur, reçu « {s} »"))
        })
        .collect()
}

async fn cmd_run(args: RunArgs) -> Result<ExitCode, String> {
    if images::find(&args.target).is_none() {
        return Err(format!(
            "image inconnue « {} » (connues : {})",
            args.target,
            images::names().join(", ")
        ));
    }
    if !config::valid_user(&args.as_user) {
        return Err(format!("nom d'utilisateur invalide « {} »", args.as_user));
    }
    for cap in &args.caps {
        config::check_cap(cap)?;
    }
    if !args.caps.is_empty() && args.as_user == "root" {
        return Err("--caps ne s'applique qu'avec --as-user <non root>".into());
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
            eprintln!("distroproof: interrompu");
            return Ok(ExitCode::from(130));
        }
    };
    drop(ctrl_tx);
    drop(ev);
    let _ = printer.await;

    if let Some(path) = &args.result {
        let json = serde_json::to_string_pretty(&result).expect("sérialisable");
        std::fs::write(path, json).map_err(|e| format!("{} : {e}", path.display()))?;
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
        "{} : {} cible(s) valides",
        path.display(),
        cfg.targets.len()
    );
    for t in &cfg.targets {
        let mut variants = Vec::new();
        for (k, v) in &t.sysctl {
            variants.push(format!("{k}={v}"));
        }
        if let Some(c) = &t.kernel_cmdline {
            variants.push(format!("cmdline « {c} »"));
        }
        if t.user != "root" {
            variants.push(format!("utilisateur {}", t.user));
        }
        if !t.caps.is_empty() {
            variants.push(format!("caps {}", t.caps.join(",")));
        }
        if !t.pmu {
            variants.push("sans PMU".into());
        }
        let _ = writeln!(
            out,
            "  {:<20} {:<14} {} Mio, {} CPU, timeout {}{}",
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
                .map_err(|e| format!("{} : {e}", path.display()))?;
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
    let text = std::fs::read_to_string(path).map_err(|e| format!("{} : {e}", path.display()))?;
    let saved: report::MatrixReport =
        serde_json::from_str(&text).map_err(|e| format!("{} : {e}", path.display()))?;
    // L'analyse est recalculée : elle profite des améliorations de l'heuristique.
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
    // Toutes les matrices sont validées avant de lancer la moindre VM.
    let mut specs: Vec<(VmSpec, usize)> = Vec::new();
    let mut groups = Vec::new();
    for (group, path) in args.file.iter().enumerate() {
        let cfg = load_config(path)?;
        for t in cfg.targets {
            if let Some((_, g)) = specs.iter().find(|(s, _)| s.name == t.name) {
                return Err(format!(
                    "cible « {} » définie dans {} et {}",
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
            return Err(format!("cible inconnue « {bad} » dans --only"));
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
        // Quitter la TUI arrête les VM encore allumées.
        let results = matrix.shutdown().await;
        ui.map_err(|e| format!("TUI : {e}"))?;
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
            eprintln!("distroproof: interrompu, arrêt des VM");
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
                            ", noyau extrait"
                        } else {
                            ""
                        }
                    ),
                    None => "absente du cache".into(),
                };
                let _ = writeln!(out, "{:<width$}  {:<45}  {state}", e.name, e.description);
            }
            let _ = writeln!(out, "\ncache : {}", dirs::images().display());
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
                        ev.info(format!("erreur : {e}"));
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
                std::fs::remove_dir_all(&dir).map_err(|e| format!("{} : {e}", dir.display()))?;
            }
            println!("{} libérés ({})", fmt_bytes(freed), dir.display());
            Ok(ExitCode::SUCCESS)
        }
    }
}
