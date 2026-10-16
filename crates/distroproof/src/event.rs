//! Bus d'événements de l'orchestrateur.
//!
//! L'orchestrateur n'écrit jamais dans le terminal : il émet des [`Event`]
//! dans un channel tokio. La sortie texte, la sortie JSON et la TUI ne sont
//! que des consommateurs de ce flux.

use std::path::PathBuf;

use serde::Serialize;
use tokio::sync::mpsc;

use crate::model::TargetResult;

pub type EventTx = mpsc::UnboundedSender<Event>;
pub type EventRx = mpsc::UnboundedReceiver<Event>;

/// Un événement concernant une cible de la matrice.
#[derive(Debug, Clone, Serialize)]
pub struct Event {
    pub target: String,
    #[serde(flatten)]
    pub kind: VmEvent,
}

/// Flux de sortie d'une commande dans l'invité.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Stream {
    Stdout,
    Stderr,
}

/// Étape exécutée par l'agent invité.
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
            Stage::Command => "commande",
            Stage::Kernel => "extraction du noyau",
        }
    }
}

/// Métriques du processus QEMU et de la VM, rafraîchies chaque seconde.
#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct Metrics {
    /// CPU consommé par QEMU, en pourcentage d'un cœur (peut dépasser 100).
    pub cpu_percent: f64,
    pub rss_bytes: u64,
    pub disk_read_bytes: u64,
    pub disk_write_bytes: u64,
    /// Mémoire vue par l'invité selon le ballon virtio, si disponible.
    pub guest_memory_bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum VmEvent {
    /// Cible mise en file, en attente d'un slot d'exécution.
    Queued,
    /// Message informatif (résolution d'image, avertissement…).
    Info {
        message: String,
    },
    Downloading {
        done: u64,
        total: Option<u64>,
    },
    Verifying,
    /// Image prête, issue du cache ou fraîchement téléchargée.
    ImageReady {
        file: String,
        cached: bool,
    },
    /// Boot préalable pour extraire noyau et initramfs (boot direct).
    Preparing,
    Booting {
        pid: u32,
        qmp: PathBuf,
        console: PathBuf,
        direct_kernel: bool,
    },
    /// L'agent invité a répondu : temps depuis le démarrage de la VM.
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
    /// Octets bruts de la console série (séquences ANSI comprises).
    #[serde(skip)]
    Console(Vec<u8>),
    Metrics(Metrics),
    Paused,
    Resumed,
    /// Résultat disponible (la VM peut rester allumée avec --hold).
    Finished(Box<TargetResult>),
    /// VM gardée après la commande : shell root sur la console série.
    Holding,
    /// Processus QEMU terminé.
    Stopped,
}

/// Commandes envoyées à une cible depuis un consommateur (TUI).
#[derive(Debug, Clone)]
pub enum Control {
    Pause,
    Resume,
    Kill,
    Restart,
    /// Octets à écrire sur la console série.
    ConsoleInput(Vec<u8>),
}

/// Émetteur lié à une cible.
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
        // Un consommateur parti (Ctrl+C) n'empêche pas l'orchestrateur de
        // terminer proprement.
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
