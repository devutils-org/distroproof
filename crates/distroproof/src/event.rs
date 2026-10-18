//! The orchestrator's event bus.
//!
//! The orchestrator never writes to the terminal: it emits [`Event`]s on a
//! tokio channel. The text output, the JSON output and the TUI are only
//! consumers of this stream.

use std::path::PathBuf;

use serde::Serialize;
use tokio::sync::mpsc;

use crate::model::TargetResult;

pub type EventTx = mpsc::UnboundedSender<Event>;
pub type EventRx = mpsc::UnboundedReceiver<Event>;

/// An event about one target of the matrix.
#[derive(Debug, Clone, Serialize)]
pub struct Event {
    pub target: String,
    #[serde(flatten)]
    pub kind: VmEvent,
}

/// Output stream of a command in the guest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Stream {
    Stdout,
    Stderr,
}

/// Stage run by the guest agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    Setup,
    Probe,
    Command,
    Kernel,
}

impl Stage {
    pub fn parse(s: &str) -> Option<Stage> {
        Some(match s {
            "setup" => Stage::Setup,
            "probe" => Stage::Probe,
            "command" => Stage::Command,
            "kernel" => Stage::Kernel,
            _ => return None,
        })
    }

    pub fn label(self) -> &'static str {
        match self {
            Stage::Setup => "setup",
            Stage::Probe => "envprobe",
            Stage::Command => "command",
            Stage::Kernel => "kernel extraction",
        }
    }
}

/// Metrics of the QEMU process and the VM, refreshed every second.
#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct Metrics {
    /// CPU used by QEMU, as a percentage of one core (can exceed 100).
    pub cpu_percent: f64,
    pub rss_bytes: u64,
    pub disk_read_bytes: u64,
    pub disk_write_bytes: u64,
    /// Memory seen by the guest according to the virtio balloon, if available.
    pub guest_memory_bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum VmEvent {
    /// Target queued, waiting for an execution slot.
    Queued,
    /// Informational message (image resolution, warning…).
    Info {
        message: String,
    },
    Downloading {
        done: u64,
        total: Option<u64>,
    },
    Verifying,
    /// Image ready, from the cache or freshly downloaded.
    ImageReady {
        file: String,
        cached: bool,
    },
    /// Preliminary boot to extract the kernel and initramfs (direct boot).
    Preparing,
    Booting {
        pid: u32,
        qmp: PathBuf,
        console: PathBuf,
        direct_kernel: bool,
    },
    /// The guest agent answered: time since the VM started.
    Booted {
        boot_ms: u64,
    },
    Stage {
        stage: Stage,
    },
    StageExited {
        stage: Stage,
        code: i32,
    },
    Output {
        stream: Stream,
        line: String,
    },
    /// Raw serial console bytes (ANSI sequences included).
    #[serde(skip)]
    Console(Vec<u8>),
    Metrics(Metrics),
    Paused,
    Resumed,
    /// Result available (the VM may stay up with --hold).
    Finished(Box<TargetResult>),
    /// VM kept up after the command: root shell on the serial console.
    Holding,
    /// QEMU process exited.
    Stopped,
}

/// Commands sent to a target by a consumer (TUI).
#[derive(Debug, Clone)]
pub enum Control {
    Pause,
    Resume,
    Kill,
    Restart,
    /// Bytes to write to the serial console.
    ConsoleInput(Vec<u8>),
}

/// Sender bound to one target.
#[derive(Clone)]
pub struct Emitter {
    target: String,
    tx: EventTx,
}

impl Emitter {
    pub fn new(target: impl Into<String>, tx: EventTx) -> Self {
        Emitter {
            target: target.into(),
            tx,
        }
    }

    pub fn emit(&self, kind: VmEvent) {
        // A consumer that went away (Ctrl+C) does not stop the orchestrator from
        // finishing cleanly.
        let _ = self.tx.send(Event {
            target: self.target.clone(),
            kind,
        });
    }

    pub fn info(&self, message: impl Into<String>) {
        self.emit(VmEvent::Info {
            message: message.into(),
        });
    }
}
