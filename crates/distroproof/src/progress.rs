//! Consommateurs non interactifs du bus d'événements : texte et JSON lignes.

use std::collections::HashMap;
use std::io::{self, Write};
use std::time::{Duration, Instant};

use crate::event::{Event, EventRx, Stream, VmEvent};
use crate::model::Outcome;

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Progress {
    /// Lignes lisibles sur stderr.
    Text,
    /// Un événement JSON par ligne sur stderr.
    Json,
    /// Rien.
    None,
}

pub fn fmt_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["o", "Kio", "Mio", "Gio", "Tio"];
    let mut v = n as f64;
    let mut unit = 0;
    while v >= 1024.0 && unit < UNITS.len() - 1 {
        v /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{n} o")
    } else {
        format!("{v:.1} {}", UNITS[unit])
    }
}

pub fn fmt_ms(ms: u64) -> String {
    if ms < 60_000 {
        format!("{:.1} s", ms as f64 / 1000.0)
    } else {
        format!("{}m{:02}s", ms / 60_000, ms / 1000 % 60)
    }
}

/// Lit le flux jusqu'à sa fermeture et l'affiche sur stderr.
pub async fn consume(mut rx: EventRx, mode: Progress, verbose: bool) {
    let mut last_download: HashMap<String, (Instant, u64)> = HashMap::new();
    while let Some(event) = rx.recv().await {
        let res = match mode {
            Progress::None => Ok(()),
            Progress::Json => json_line(&event),
            Progress::Text => text_line(&event, verbose, &mut last_download),
        };
        if res.is_err() {
            // stderr fermé : on continue de vider le flux sans rien écrire.
            continue;
        }
    }
}

fn json_line(event: &Event) -> io::Result<()> {
    if matches!(event.kind, VmEvent::Console(_)) {
        return Ok(());
    }
    let line = serde_json::to_string(event).map_err(io::Error::other)?;
    writeln!(io::stderr().lock(), "{line}")
}

fn text_line(
    event: &Event,
    verbose: bool,
    last_download: &mut HashMap<String, (Instant, u64)>,
) -> io::Result<()> {
    let t = &event.target;
    let msg = match &event.kind {
        VmEvent::Queued | VmEvent::Console(_) | VmEvent::Metrics(_) | VmEvent::Stopped => {
            return Ok(())
        }
        VmEvent::Holding => "VM gardée : shell root sur la console série".into(),
        VmEvent::Info { message } => message.clone(),
        VmEvent::Downloading { done, total } => {
            let now = Instant::now();
            let entry = last_download
                .entry(t.clone())
                .or_insert((now - Duration::from_secs(60), 0));
            let finished = Some(*done) == *total;
            if !finished && *done != 0 && now.duration_since(entry.0) < Duration::from_secs(5) {
                return Ok(());
            }
            *entry = (now, *done);
            match total {
                Some(total) if *total > 0 => format!(
                    "téléchargement {:>3} % ({} / {})",
                    done * 100 / total,
                    fmt_bytes(*done),
                    fmt_bytes(*total)
                ),
                _ => format!("téléchargement {}", fmt_bytes(*done)),
            }
        }
        VmEvent::Verifying => "vérification du checksum".into(),
        VmEvent::ImageReady { file, cached } => {
            format!("image {file}{}", if *cached { " (cache)" } else { "" })
        }
        VmEvent::Preparing => "boot de préparation : extraction du noyau".into(),
        VmEvent::Booting {
            pid, direct_kernel, ..
        } => format!(
            "démarrage de la VM (pid {pid}, {})",
            if *direct_kernel {
                "boot direct sur le noyau de la distro"
            } else {
                "boot sur le disque"
            }
        ),
        VmEvent::Booted { boot_ms } => format!("agent prêt, boot en {}", fmt_ms(*boot_ms)),
        VmEvent::Stage { stage } => format!("étape {}", stage.label()),
        VmEvent::StageExited { stage, code } => {
            format!("{} terminé (code {code})", stage.label())
        }
        VmEvent::Output { stream, line } => {
            if !verbose {
                return Ok(());
            }
            let sep = if *stream == Stream::Stderr { "!" } else { "|" };
            format!("{sep} {line}")
        }
        VmEvent::Paused => "en pause".into(),
        VmEvent::Resumed => "reprise".into(),
        VmEvent::Finished(r) => {
            let boot = r
                .boot_ms
                .map(|b| format!(", boot {}", fmt_ms(b)))
                .unwrap_or_default();
            let kernel = r
                .kernel_release()
                .map(|k| format!(", noyau {k}"))
                .unwrap_or_default();
            let detail = match r.outcome {
                Outcome::Failure => r
                    .exit_code()
                    .map(|c| format!(" (code {c})"))
                    .or_else(|| r.error.as_ref().map(|e| format!(" : {e}")))
                    .unwrap_or_default(),
                Outcome::Error | Outcome::TimedOut | Outcome::Killed => r
                    .error
                    .as_ref()
                    .map(|e| format!(" : {e}"))
                    .unwrap_or_default(),
                Outcome::Success => String::new(),
            };
            format!(
                "{}{detail} en {}{boot}{kernel}",
                r.outcome.label(),
                fmt_ms(r.duration_ms)
            )
        }
    };
    writeln!(io::stderr().lock(), "[{t}] {msg}")
}
