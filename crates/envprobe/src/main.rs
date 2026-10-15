use std::process::ExitCode;

use clap::Parser;

/// Décrit l'environnement Linux courant (userspace + kernel).
#[derive(Parser, Debug)]
#[command(version, about)]
struct Cli {
    /// Rapport JSON sur stdout (réservé, phase 2).
    #[arg(long)]
    json: bool,
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    if cli.json {
        eprintln!("envprobe: --json n'est pas encore implémenté");
        return ExitCode::from(2);
    }

    let sections = envprobe_core::probe_all();
    println!("envprobe {}", env!("CARGO_PKG_VERSION"));
    if sections.is_empty() {
        println!("aucune sonde disponible");
    }
    for section in &sections {
        println!("\n[{}]", section.name);
        for (key, value) in &section.entries {
            println!("  {key}: {value}");
        }
    }
    ExitCode::SUCCESS
}
