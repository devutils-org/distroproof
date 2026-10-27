//! Description of a run and of its result.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use envprobe_core::Report;
use serde::{Deserialize, Serialize};

/// Everything needed to run a target.
#[derive(Debug, Clone)]
pub struct VmSpec {
    /// Unique name in the matrix.
    pub name: String,
    /// Catalog image (`debian-12`, `rocky-8`…), or `ssh` for an ssh target.
    pub image: String,
    /// A machine already running, reached over SSH, instead of a VM.
    pub ssh: Option<SshHost>,
    pub memory_mib: u64,
    pub cpus: u32,
    pub timeout: Duration,
    pub sysctl: BTreeMap<String, String>,
    /// Arguments appended to the kernel command line (direct boot).
    pub kernel_cmdline: Option<String>,
    /// User that runs envprobe and the command: `root` or a created account.
    pub user: String,
    /// Capabilities granted to the non-root user (`bpf`, `perfmon`…).
    pub caps: Vec<String>,
    /// PMU exposed to the guest (`-cpu host`) or hidden (`pmu=off`).
    pub pmu: bool,
    /// Script run as root before the probe and the command.
    pub setup: Option<String>,
    /// Shell command run in the guest's working directory.
    pub command: Option<String>,
    pub env: BTreeMap<String, String>,
    /// Directory shared with the guest (copied to `/distroproof/work`).
    pub workdir: Option<PathBuf>,
    pub exclude: Vec<String>,
    pub include: Vec<String>,
    /// Run envprobe before the command and attach its report.
    pub probe: bool,
    /// Keep the VM up after the command, with a root shell on the serial
    /// console, until it is killed.
    pub hold: bool,
    /// Files whose capabilities are set as root after `setup`, by path:
    /// `setcap cap_<a>,cap_<b>+ep <path>`. A file capability is what a
    /// program's install grants; `caps` raises ambient ones on the user, which
    /// every program the command starts inherits.
    pub file_caps: BTreeMap<String, Vec<String>>,
    /// Paths, relative to the working directory, copied back after the
    /// command into `<workdir>/.distroproof/collect/<target>/`.
    pub collect: Vec<String>,
}

/// How an ssh target is reached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshHost {
    /// `user@host`, `host`, or an alias of `~/.ssh/config`.
    pub host: String,
    pub port: Option<u16>,
    /// The private key, when it is not the agent's or the config's.
    pub identity: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// The command returned 0.
    Success,
    /// The command returned a non-zero code.
    Failure,
    TimedOut,
    /// Interrupted from the TUI or by Ctrl+C.
    Killed,
    /// Orchestrator error: image, boot, unreachable agent…
    Error,
}

impl Outcome {
    pub fn label(self) -> &'static str {
        match self {
            Outcome::Success => "success",
            Outcome::Failure => "failed",
            Outcome::TimedOut => "timeout",
            Outcome::Killed => "killed",
            Outcome::Error => "error",
        }
    }

    pub fn is_success(self) -> bool {
        self == Outcome::Success
    }
}

/// Result of an agent stage (setup, command).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StageResult {
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

/// Variants applied to the target, copied into the result.
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
    /// File of the image used (exact version).
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
    /// Agent errors (sysctl refused, unknown capability…).
    #[serde(default)]
    pub warnings: Vec<String>,
    #[serde(default)]
    pub error: Option<String>,
    /// Last lines of the serial console, useful when the boot fails.
    #[serde(default)]
    pub console_tail: String,
    /// Files brought back by `collect`, as written on the host.
    #[serde(default)]
    pub collected: Vec<String>,
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
            collected: Vec::new(),
        }
    }

    /// Kernel release seen by envprobe in the guest.
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
