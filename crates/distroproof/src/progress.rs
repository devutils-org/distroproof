//! Non-interactive consumers of the event bus: text and JSON lines.

use std::collections::HashMap;
use std::io::{self, Write};
use std::time::{Duration, Instant};

use crate::event::{Event, EventRx, Stream, VmEvent};
use crate::model::Outcome;

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Progress {
    /// Human-readable lines on stderr.
    Text,
    /// One JSON event per line on stderr.
    Json,
    /// Nothing.
    None,
}

pub fn fmt_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = n as f64;
    let mut unit = 0;
    while v >= 1024.0 && unit < UNITS.len() - 1 {
        v /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{n} B")
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

/// Reads the stream until it closes and prints it on stderr.
pub async fn consume(mut rx: EventRx, mode: Progress, verbose: bool) {
    let mut last_download: HashMap<String, (Instant, u64)> = HashMap::new();
    while let Some(event) = rx.recv().await {
        let res = match mode {
            Progress::None => Ok(()),
            Progress::Json => json_line(&event),
            Progress::Text => text_line(&event, verbose, &mut last_download),
        };
        if res.is_err() {
            // stderr closed: keep draining the stream without writing anything.
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
        VmEvent::Holding => "VM kept up: root shell on the serial console".into(),
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
                    "downloading {:>3}% ({} / {})",
                    done * 100 / total,
                    fmt_bytes(*done),
                    fmt_bytes(*total)
                ),
                _ => format!("downloading {}", fmt_bytes(*done)),
            }
        }
        VmEvent::Verifying => "verifying checksum".into(),
        VmEvent::ImageReady { file, cached } => {
            format!("image {file}{}", if *cached { " (cache)" } else { "" })
        }
        VmEvent::Preparing => "preparation boot: extracting the kernel".into(),
        VmEvent::Booting {
            pid, direct_kernel, ..
        } => format!(
            "starting the VM (pid {pid}, {})",
            if *direct_kernel {
                "direct boot on the distro kernel"
            } else {
                "booting from disk"
            }
        ),
        VmEvent::Booted { boot_ms } => format!("agent ready, booted in {}", fmt_ms(*boot_ms)),
        VmEvent::Stage { stage } => format!("stage {}", stage.label()),
        VmEvent::StageExited { stage, code } => {
            format!("{} finished (code {code})", stage.label())
        }
        VmEvent::Output { stream, line } => {
            if !verbose {
                return Ok(());
            }
            let sep = if *stream == Stream::Stderr { "!" } else { "|" };
            format!("{sep} {line}")
        }
        VmEvent::Paused => "paused".into(),
        VmEvent::Resumed => "resumed".into(),
        VmEvent::Finished(r) => {
            let boot = r
                .boot_ms
                .map(|b| format!(", boot {}", fmt_ms(b)))
                .unwrap_or_default();
            let kernel = r
                .kernel_release()
                .map(|k| format!(", kernel {k}"))
                .unwrap_or_default();
            let detail = match r.outcome {
                Outcome::Failure => r
                    .exit_code()
                    .map(|c| format!(" (code {c})"))
                    .or_else(|| r.error.as_ref().map(|e| format!(": {e}")))
                    .unwrap_or_default(),
                Outcome::Error | Outcome::TimedOut | Outcome::Killed => r
                    .error
                    .as_ref()
                    .map(|e| format!(": {e}"))
                    .unwrap_or_default(),
                Outcome::Success => String::new(),
            };
            format!(
                "{}{detail} in {}{boot}{kernel}",
                r.outcome.label(),
                fmt_ms(r.duration_ms)
            )
        }
    };
    writeln!(io::stderr().lock(), "[{t}] {msg}")
}
