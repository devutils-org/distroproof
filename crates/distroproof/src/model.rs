//! Description d'une exécution et de son résultat.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use envprobe_core::Report;
use serde::{Deserialize, Serialize};

/// Tout ce qu'il faut pour lancer une cible.
#[derive(Debug, Clone)]
pub struct VmSpec {
    /// Nom unique dans la matrice.
    pub name: String,
    /// Image du catalogue (`debian-12`, `rocky-8`…).
    pub image: String,
    pub memory_mib: u64,
    pub cpus: u32,
    pub timeout: Duration,
    pub sysctl: BTreeMap<String, String>,
    /// Arguments ajoutés à la ligne de commande du noyau (boot direct).
    pub kernel_cmdline: Option<String>,
    /// Utilisateur qui lance envprobe et la commande : `root` ou un compte créé.
    pub user: String,
    /// Capabilities accordées à l'utilisateur non root (`bpf`, `perfmon`…).
    pub caps: Vec<String>,
    /// PMU exposée à l'invité (`-cpu host`) ou masquée (`pmu=off`).
    pub pmu: bool,
    /// Script lancé en root avant la sonde et la commande.
    pub setup: Option<String>,
    /// Commande shell lancée dans le répertoire de travail de l'invité.
    pub command: Option<String>,
    pub env: BTreeMap<String, String>,
    /// Répertoire partagé avec l'invité (copié dans `/distroproof/work`).
    pub workdir: Option<PathBuf>,
    pub exclude: Vec<String>,
    pub include: Vec<String>,
    /// Lancer envprobe avant la commande et joindre son rapport.
    pub probe: bool,
    /// Garder la VM allumée après la commande, avec un shell root sur la
    /// console série, jusqu'à ce qu'on la tue.
    pub hold: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// La commande a renvoyé 0.
    Success,
    /// La commande a renvoyé un code non nul.
    Failure,
    TimedOut,
    /// Interrompue depuis la TUI ou par Ctrl+C.
    Killed,
    /// Erreur de l'orchestrateur : image, boot, agent injoignable…
    Error,
}

impl Outcome {
    pub fn label(self) -> &'static str {
        match self {
            Outcome::Success => "succès",
            Outcome::Failure => "échec",
            Outcome::TimedOut => "timeout",
            Outcome::Killed => "interrompu",
            Outcome::Error => "erreur",
        }
    }

    pub fn is_success(self) -> bool {
        self == Outcome::Success
    }
}

/// Résultat d'une étape de l'agent (setup, commande).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StageResult {
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

/// Variantes appliquées à la cible, recopiées dans le résultat.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Variant {
    #[serde(skip_serializing_if = "BTreeMap::is_empty", default)]
    pub sysctl: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub kernel_cmdline: Option<String>,
    pub user: String,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub caps: Vec<String>,
    pub pmu: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TargetResult {
    pub name: String,
    pub image: String,
    /// Fichier de l'image utilisée (version exacte).
    #[serde(default)]
    pub image_file: Option<String>,
    pub outcome: Outcome,
    pub variant: Variant,
    #[serde(default)]
    pub direct_kernel: bool,
    #[serde(default)]
    pub boot_ms: Option<u64>,
    pub duration_ms: u64,
    #[serde(default)]
    pub setup: Option<StageResult>,
    #[serde(default)]
    pub command: Option<StageResult>,
    #[serde(default)]
    pub envprobe: Option<Report>,
    /// Erreurs de l'agent (sysctl refusé, capability inconnue…).
    #[serde(default)]
    pub warnings: Vec<String>,
    #[serde(default)]
    pub error: Option<String>,
    /// Dernières lignes de la console série, utiles quand le boot échoue.
    #[serde(default)]
    pub console_tail: String,
}

impl TargetResult {
    pub fn new(spec: &VmSpec) -> Self {
        TargetResult {
            name: spec.name.clone(),
            image: spec.image.clone(),
            image_file: None,
            outcome: Outcome::Error,
            variant: Variant {
                sysctl: spec.sysctl.clone(),
                kernel_cmdline: spec.kernel_cmdline.clone(),
                user: spec.user.clone(),
                caps: spec.caps.clone(),
                pmu: spec.pmu,
            },
            direct_kernel: false,
            boot_ms: None,
            duration_ms: 0,
            setup: None,
            command: None,
            envprobe: None,
            warnings: Vec::new(),
            error: None,
            console_tail: String::new(),
        }
    }

    /// Release du noyau vue par envprobe dans l'invité.
    pub fn kernel_release(&self) -> Option<&str> {
        self.envprobe
            .as_ref()?
            .section("kernel")?
            .get("release")?
            .as_ok()
    }

    pub fn exit_code(&self) -> Option<i32> {
        self.command.as_ref().and_then(|c| c.exit_code)
    }

    pub fn stderr(&self) -> &str {
        self.command.as_ref().map_or("", |c| c.stderr.as_str())
    }
}
