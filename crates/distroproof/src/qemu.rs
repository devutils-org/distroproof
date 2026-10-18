//! QEMU piloté en sous-processus, et son socket QMP.

use std::env;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{json, Value as Json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::UnixStream;
use tokio::process::Command;

/// Binaire QEMU : `$DISTROPROOF_QEMU` ou `qemu-system-x86_64` dans le PATH.
pub fn qemu_binary() -> OsString {
    env::var_os("DISTROPROOF_QEMU").unwrap_or_else(|| "qemu-system-x86_64".into())
}

pub fn qemu_img_binary() -> OsString {
    env::var_os("DISTROPROOF_QEMU_IMG").unwrap_or_else(|| "qemu-img".into())
}

/// KVM utilisable par l'utilisateur courant ?
pub fn kvm_available() -> bool {
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/kvm")
        .is_ok()
}

/// Firmware UEFI (OVMF) : code en lecture seule et modèle de variables.
pub struct Ovmf {
    pub code: PathBuf,
    pub vars: PathBuf,
}

/// Cherche OVMF : `$DISTROPROOF_OVMF_CODE` et `$DISTROPROOF_OVMF_VARS`, puis
/// les emplacements des paquets Arch, Debian/Ubuntu, Fedora et de QEMU.
pub fn find_ovmf() -> Option<Ovmf> {
    if let (Some(code), Some(vars)) = (
        env::var_os("DISTROPROOF_OVMF_CODE"),
        env::var_os("DISTROPROOF_OVMF_VARS"),
    ) {
        return Some(Ovmf {
            code: code.into(),
            vars: vars.into(),
        });
    }
    const PAIRS: &[(&str, &str)] = &[
        (
            "/usr/share/edk2/x64/OVMF_CODE.4m.fd",
            "/usr/share/edk2/x64/OVMF_VARS.4m.fd",
        ),
        (
            "/usr/share/OVMF/OVMF_CODE_4M.fd",
            "/usr/share/OVMF/OVMF_VARS_4M.fd",
        ),
        (
            "/usr/share/OVMF/OVMF_CODE.fd",
            "/usr/share/OVMF/OVMF_VARS.fd",
        ),
        (
            "/usr/share/edk2/ovmf/OVMF_CODE.fd",
            "/usr/share/edk2/ovmf/OVMF_VARS.fd",
        ),
        (
            "/usr/share/qemu/edk2-x86_64-code.fd",
            "/usr/share/qemu/edk2-i386-vars.fd",
        ),
    ];
    PAIRS.iter().find_map(|(code, vars)| {
        let (code, vars) = (Path::new(code), Path::new(vars));
        (code.is_file() && vars.is_file()).then(|| Ovmf {
            code: code.to_path_buf(),
            vars: vars.to_path_buf(),
        })
    })
}

/// Overlay qcow2 jetable au-dessus de l'image de base, qui reste intacte.
pub async fn create_overlay(base: &Path, overlay: &Path) -> Result<(), String> {
    let out = Command::new(qemu_img_binary())
        .args(["create", "-q", "-f", "qcow2", "-F", "qcow2", "-b"])
        .arg(base)
        .arg(overlay)
        .output()
        .await
        .map_err(|e| format!("qemu-img introuvable : {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "qemu-img create : {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(())
}

/// Démarrage direct sur un noyau extrait de l'image.
pub struct KernelBoot<'a> {
    pub kernel: &'a Path,
    pub initrd: Option<&'a Path>,
    pub cmdline: String,
}

pub struct Machine<'a> {
    pub name: &'a str,
    pub memory_mib: u64,
    pub cpus: u32,
    pub pmu: bool,
    pub overlay: &'a Path,
    pub seed: &'a Path,
    pub payload: &'a Path,
    pub output: &'a Path,
    pub qmp: &'a Path,
    pub console: &'a Path,
    pub agent: &'a Path,
    pub kernel: Option<KernelBoot<'a>>,
    /// Boot UEFI : code OVMF et copie des variables propre à cette VM.
    pub uefi: Option<(&'a Path, &'a Path)>,
}

impl Machine<'_> {
    /// Ligne de commande QEMU. La VM démarre en pause (`-S`) : l'hôte se
    /// connecte à tous les sockets avant de la lancer, rien n'est perdu.
    pub fn command(&self) -> Command {
        let mut cmd = Command::new(qemu_binary());
        let kvm = kvm_available();
        let cpu = match (kvm, self.pmu) {
            (true, true) => "host".to_string(),
            (true, false) => "host,pmu=off".to_string(),
            (false, true) => "max".to_string(),
            (false, false) => "max,pmu=off".to_string(),
        };
        cmd.arg("-name")
            .arg(format!("distroproof-{}", self.name))
            .args([
                "-machine",
                if kvm {
                    "q35,accel=kvm"
                } else {
                    "q35,accel=tcg"
                },
            ])
            .args(["-cpu", &cpu])
            .args(["-smp", &self.cpus.to_string()])
            .args(["-m", &format!("{}M", self.memory_mib)])
            .args(["-nodefaults", "-display", "none", "-no-reboot", "-S"]);
        if let Some((code, vars)) = self.uefi {
            let mut c = OsString::from("if=pflash,format=raw,readonly=on,file=");
            c.push(escape_opt(code));
            let mut v = OsString::from("if=pflash,format=raw,file=");
            v.push(escape_opt(vars));
            cmd.arg("-drive").arg(c).arg("-drive").arg(v);
        }
        cmd.arg("-qmp")
            .arg(unix_server(self.qmp))
            .arg("-chardev")
            .arg(socket_chardev("console", self.console))
            .args(["-serial", "chardev:console"])
            .arg("-chardev")
            .arg(socket_chardev("agent", self.agent))
            .args(["-serial", "chardev:agent"]);
        let disks = [
            (self.overlay, "qcow2", "dp-root", false),
            (self.seed, "raw", "dp-seed", true),
            (self.payload, "raw", "dp-payload", true),
            (self.output, "raw", "dp-output", false),
        ];
        for (i, (path, format, serial, readonly)) in disks.into_iter().enumerate() {
            let mut drive = OsString::from(format!("if=none,id=disk{i},format={format},file="));
            drive.push(escape_opt(path));
            if readonly {
                drive.push(",readonly=on");
            }
            cmd.arg("-drive").arg(drive);
            // Avec -kernel, QEMU réserve lui-même le premier index de boot.
            let boot = if i == 0 && self.kernel.is_none() {
                ",bootindex=0"
            } else {
                ""
            };
            cmd.arg("-device").arg(format!(
                "virtio-blk-pci,drive=disk{i},serial={serial}{boot}"
            ));
        }
        cmd.args(["-nic", "user,model=virtio-net-pci"])
            .args(["-device", "virtio-rng-pci"])
            .args(["-device", "virtio-balloon-pci,id=balloon0"]);
        if let Some(k) = &self.kernel {
            cmd.arg("-kernel").arg(k.kernel);
            if let Some(initrd) = k.initrd {
                cmd.arg("-initrd").arg(initrd);
            }
            cmd.arg("-append").arg(&k.cmdline);
        }
        cmd
    }
}

/// Les options QEMU séparées par des virgules : une virgule se double.
fn escape_opt(path: &Path) -> OsString {
    path.to_string_lossy().replace(',', ",,").into()
}

fn unix_server(path: &Path) -> OsString {
    let mut s = OsString::from("unix:");
    s.push(escape_opt(path));
    s.push(",server=on,wait=off");
    s
}

fn socket_chardev(id: &str, path: &Path) -> OsString {
    let mut s = OsString::from(format!("socket,id={id},path="));
    s.push(escape_opt(path));
    s.push(",server=on,wait=off");
    s
}

/// Connexion à un socket unix créé par QEMU, en attendant qu'il apparaisse.
pub async fn connect(path: &Path, wait: Duration) -> Result<UnixStream, String> {
    let deadline = tokio::time::Instant::now() + wait;
    loop {
        match UnixStream::connect(path).await {
            Ok(s) => return Ok(s),
            Err(e) if tokio::time::Instant::now() >= deadline => {
                return Err(format!("{} : {e}", path.display()))
            }
            Err(_) => tokio::time::sleep(Duration::from_millis(50)).await,
        }
    }
}

/// Client QMP minimal : une commande à la fois, événements ignorés.
pub struct Qmp {
    reader: BufReader<OwnedReadHalf>,
    writer: OwnedWriteHalf,
}

impl Qmp {
    pub async fn connect(path: &Path) -> Result<Qmp, String> {
        let stream = connect(path, Duration::from_secs(10)).await?;
        let (r, w) = stream.into_split();
        let mut qmp = Qmp {
            reader: BufReader::new(r),
            writer: w,
        };
        // Message d'accueil puis négociation des capacités.
        qmp.read_message().await?;
        qmp.execute("qmp_capabilities", None).await?;
        Ok(qmp)
    }

    async fn read_message(&mut self) -> Result<Json, String> {
        let mut line = String::new();
        loop {
            line.clear();
            let n = self
                .reader
                .read_line(&mut line)
                .await
                .map_err(|e| format!("QMP : {e}"))?;
            if n == 0 {
                return Err("QMP : connexion fermée".into());
            }
            let msg: Json = serde_json::from_str(&line).map_err(|e| format!("QMP : {e}"))?;
            if msg.get("event").is_none() {
                return Ok(msg);
            }
        }
    }

    pub async fn execute(&mut self, command: &str, args: Option<Json>) -> Result<Json, String> {
        let mut msg = json!({ "execute": command });
        if let Some(args) = args {
            msg["arguments"] = args;
        }
        let mut text = msg.to_string();
        text.push('\n');
        self.writer
            .write_all(text.as_bytes())
            .await
            .map_err(|e| format!("QMP : {e}"))?;
        let reply = tokio::time::timeout(Duration::from_secs(10), self.read_message())
            .await
            .map_err(|_| format!("QMP : pas de réponse à {command}"))??;
        if let Some(err) = reply.get("error") {
            return Err(format!(
                "QMP {command} : {}",
                err.get("desc").and_then(Json::as_str).unwrap_or("erreur")
            ));
        }
        Ok(reply.get("return").cloned().unwrap_or(Json::Null))
    }

    /// Octets lus et écrits sur l'ensemble des disques.
    pub async fn block_stats(&mut self) -> Option<(u64, u64)> {
        let ret = self.execute("query-blockstats", None).await.ok()?;
        let mut rd = 0;
        let mut wr = 0;
        for dev in ret.as_array()? {
            let stats = dev.get("stats")?;
            rd += stats.get("rd_bytes").and_then(Json::as_u64).unwrap_or(0);
            wr += stats.get("wr_bytes").and_then(Json::as_u64).unwrap_or(0);
        }
        Some((rd, wr))
    }

    pub async fn balloon(&mut self) -> Option<u64> {
        let ret = self.execute("query-balloon", None).await.ok()?;
        ret.get("actual").and_then(Json::as_u64)
    }
}

/// Répertoire d'une exécution, supprimé quoi qu'il arrive.
pub struct RunDir {
    pub dir: PathBuf,
    pub sockets: PathBuf,
    pub keep: bool,
}

impl RunDir {
    pub fn create(name: &str) -> Result<RunDir, String> {
        let id = format!(
            "{}-{}-{}",
            name.replace(
                |c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '.',
                "_"
            ),
            std::process::id(),
            unique()
        );
        let dir = crate::dirs::runs().join(&id);
        let sockets = crate::dirs::sockets().join(&id);
        for d in [&dir, &sockets] {
            std::fs::create_dir_all(d).map_err(|e| format!("{} : {e}", d.display()))?;
        }
        std::fs::write(dir.join("pid"), std::process::id().to_string())
            .map_err(|e| e.to_string())?;
        Ok(RunDir {
            dir,
            sockets,
            keep: false,
        })
    }

    pub fn path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    pub fn socket(&self, name: &str) -> PathBuf {
        self.sockets.join(name)
    }
}

impl Drop for RunDir {
    fn drop(&mut self) {
        if !self.keep {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
        let _ = std::fs::remove_dir_all(&self.sockets);
    }
}

fn unique() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos() as u64);
    (nanos << 8) ^ NEXT.fetch_add(1, Ordering::Relaxed)
}

/// Supprime les répertoires laissés par un distroproof tué (SIGKILL).
pub fn clean_stale_runs() {
    for base in [crate::dirs::runs(), crate::dirs::sockets()] {
        let Ok(entries) = std::fs::read_dir(&base) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            // <nom>-<pid>-<unique>
            let pid = name.rsplit('-').nth(1).and_then(|p| p.parse::<u32>().ok());
            if let Some(pid) = pid {
                if !crate::sys::pid_alive(pid) {
                    let _ = std::fs::remove_dir_all(entry.path());
                }
            }
        }
    }
}
