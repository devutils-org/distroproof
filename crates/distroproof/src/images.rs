//! Catalogue des images cloud officielles et cache local.
//!
//! Les images cloud (qcow2) contiennent le noyau, l'initramfs et la config
//! d'origine de la distro : c'est ce noyau qui tourne dans la VM. Elles sont
//! téléchargées avec `curl`, vérifiées contre le checksum publié par la distro
//! puis gardées dans `$XDG_CACHE_HOME/distroproof/images/<image>/`.

use std::collections::HashMap;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256, Sha512};
use tokio::process::Command;
use tokio::sync::Mutex;

use crate::dirs;
use crate::event::{Emitter, VmEvent};

/// Où trouver la dernière version d'une image.
enum Source {
    /// URL fixe et fichier de checksums publié à côté.
    Direct {
        url: &'static str,
        checksums: &'static str,
    },
    /// Index HTML : le fichier le plus récent qui commence par `prefix` et
    /// finit par `suffix`. Le checksum est dans `<fichier><checksum_suffix>`.
    Listing {
        index: &'static str,
        prefix: &'static str,
        suffix: &'static str,
        checksum_suffix: &'static str,
    },
    /// Dernière release Fedora qui publie une image Cloud Generic.
    FedoraLatest,
}

pub struct ImageDef {
    pub name: &'static str,
    pub description: &'static str,
    source: Source,
    /// Le boot sur le disque exige UEFI : le grub BIOS de l'image redémarre
    /// la machine aussitôt (Rocky 9, Debian 13). Le boot direct s'en passe.
    pub uefi: bool,
}

pub const CATALOG: &[ImageDef] = &[
    ImageDef {
        name: "debian-10",
        description: "Debian 10 buster (archive), noyau 4.19 sans BTF",
        source: Source::Direct {
            url: "https://cloud.debian.org/images/cloud/buster/latest/debian-10-generic-amd64.qcow2",
            checksums: "https://cloud.debian.org/images/cloud/buster/latest/SHA512SUMS",
        },
        uefi: false,
    },
    ImageDef {
        name: "debian-11",
        description: "Debian 11 bullseye, noyau 5.10",
        source: Source::Direct {
            url: "https://cloud.debian.org/images/cloud/bullseye/latest/debian-11-generic-amd64.qcow2",
            checksums: "https://cloud.debian.org/images/cloud/bullseye/latest/SHA512SUMS",
        },
        uefi: false,
    },
    ImageDef {
        name: "debian-12",
        description: "Debian 12 bookworm, noyau generic",
        source: Source::Direct {
            url: "https://cloud.debian.org/images/cloud/bookworm/latest/debian-12-generic-amd64.qcow2",
            checksums: "https://cloud.debian.org/images/cloud/bookworm/latest/SHA512SUMS",
        },
        uefi: false,
    },
    ImageDef {
        name: "debian-13",
        description: "Debian 13 trixie, noyau generic",
        source: Source::Direct {
            url: "https://cloud.debian.org/images/cloud/trixie/latest/debian-13-generic-amd64.qcow2",
            checksums: "https://cloud.debian.org/images/cloud/trixie/latest/SHA512SUMS",
        },
        // Le grub BIOS de l'image redémarre la machine dès le chargement du noyau.
        uefi: true,
    },
    ImageDef {
        name: "ubuntu-18.04",
        description: "Ubuntu 18.04 bionic, noyau 4.15 sans BTF",
        source: Source::Direct {
            url: "https://cloud-images.ubuntu.com/releases/18.04/release/ubuntu-18.04-server-cloudimg-amd64.img",
            checksums: "https://cloud-images.ubuntu.com/releases/18.04/release/SHA256SUMS",
        },
        uefi: false,
    },
    ImageDef {
        name: "ubuntu-20.04",
        description: "Ubuntu 20.04 focal, noyau 5.4",
        source: Source::Direct {
            url: "https://cloud-images.ubuntu.com/releases/20.04/release/ubuntu-20.04-server-cloudimg-amd64.img",
            checksums: "https://cloud-images.ubuntu.com/releases/20.04/release/SHA256SUMS",
        },
        uefi: false,
    },
    ImageDef {
        name: "ubuntu-22.04",
        description: "Ubuntu 22.04 jammy",
        source: Source::Direct {
            url: "https://cloud-images.ubuntu.com/releases/22.04/release/ubuntu-22.04-server-cloudimg-amd64.img",
            checksums: "https://cloud-images.ubuntu.com/releases/22.04/release/SHA256SUMS",
        },
        uefi: false,
    },
    ImageDef {
        name: "ubuntu-24.04",
        description: "Ubuntu 24.04 noble",
        source: Source::Direct {
            url: "https://cloud-images.ubuntu.com/releases/24.04/release/ubuntu-24.04-server-cloudimg-amd64.img",
            checksums: "https://cloud-images.ubuntu.com/releases/24.04/release/SHA256SUMS",
        },
        uefi: false,
    },
    ImageDef {
        name: "rocky-9",
        description: "Rocky Linux 9 (RHEL 9, noyau 5.14)",
        source: Source::Direct {
            url: "https://dl.rockylinux.org/pub/rocky/9/images/x86_64/Rocky-9-GenericCloud-Base.latest.x86_64.qcow2",
            checksums: "https://dl.rockylinux.org/pub/rocky/9/images/x86_64/Rocky-9-GenericCloud-Base.latest.x86_64.qcow2.CHECKSUM",
        },
        uefi: true,
    },
    ImageDef {
        name: "rocky-8",
        description: "Rocky Linux 8 (RHEL 8, noyau 4.18)",
        source: Source::Direct {
            url: "https://dl.rockylinux.org/pub/rocky/8/images/x86_64/Rocky-8-GenericCloud-Base.latest.x86_64.qcow2",
            checksums: "https://dl.rockylinux.org/pub/rocky/8/images/x86_64/Rocky-8-GenericCloud-Base.latest.x86_64.qcow2.CHECKSUM",
        },
        uefi: false,
    },
    ImageDef {
        name: "fedora-latest",
        description: "Dernière Fedora Cloud Base Generic",
        source: Source::FedoraLatest,
        uefi: false,
    },
    ImageDef {
        name: "arch",
        description: "Arch Linux, image cloud officielle",
        source: Source::Direct {
            url: "https://geo.mirror.pkgbuild.com/images/latest/Arch-Linux-x86_64-cloudimg.qcow2",
            checksums: "https://geo.mirror.pkgbuild.com/images/latest/Arch-Linux-x86_64-cloudimg.qcow2.SHA256",
        },
        uefi: false,
    },
    ImageDef {
        name: "alpine",
        description: "Alpine stable, image generic BIOS avec cloud-init",
        source: Source::Listing {
            index: "https://dl-cdn.alpinelinux.org/alpine/latest-stable/releases/cloud/",
            prefix: "generic_alpine-",
            suffix: "-x86_64-bios-cloudinit-r0.qcow2",
            checksum_suffix: ".sha512",
        },
        uefi: false,
    },
];

pub fn find(name: &str) -> Option<&'static ImageDef> {
    CATALOG.iter().find(|d| d.name == name)
}

pub fn names() -> Vec<&'static str> {
    CATALOG.iter().map(|d| d.name).collect()
}

/// Version précise d'une image, telle que résolue en ligne.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Resolved {
    url: String,
    file: String,
    /// Checksum hexadécimal publié (sha256 ou sha512 selon la longueur).
    checksum: String,
}

/// Image prête à l'emploi.
#[derive(Debug, Clone)]
pub struct Image {
    pub name: String,
    pub path: PathBuf,
    pub file: String,
    pub uefi: bool,
}

impl Image {
    /// Noyau et initramfs extraits de l'image pour le boot direct.
    pub fn kernel_dir(&self) -> PathBuf {
        self.path.with_extension("kernel")
    }

    pub fn direct_kernel(&self) -> Option<DirectKernel> {
        DirectKernel::load(&self.kernel_dir())
    }
}

/// Noyau extrait par l'agent lors d'un premier boot depuis le disque.
#[derive(Debug, Clone)]
pub struct DirectKernel {
    pub kernel: PathBuf,
    pub initrd: Option<PathBuf>,
    /// Ligne de commande d'origine, sans `BOOT_IMAGE` ni `console=`.
    pub cmdline: String,
}

impl DirectKernel {
    fn load(dir: &Path) -> Option<DirectKernel> {
        let kernel = dir.join("vmlinuz");
        let cmdline = fs::read_to_string(dir.join("cmdline")).ok()?;
        if !kernel.is_file() {
            return None;
        }
        let initrd = Some(dir.join("initrd")).filter(|p| p.is_file());
        Some(DirectKernel {
            kernel,
            initrd,
            cmdline: clean_cmdline(&cmdline),
        })
    }
}

/// Retire de la ligne de commande d'origine ce que distroproof fixe lui-même.
pub fn clean_cmdline(cmdline: &str) -> String {
    cmdline
        .split_whitespace()
        .filter(|arg| {
            !arg.starts_with("BOOT_IMAGE=")
                && !arg.starts_with("console=")
                && !arg.starts_with("earlyprintk=")
                && !arg.starts_with("initrd=")
                && !arg.starts_with("crashkernel=")
                && *arg != "quiet"
                && *arg != "splash"
                && *arg != "rhgb"
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Cache partagé par toutes les cibles : une image n'est téléchargée qu'une
/// fois même si plusieurs cibles l'utilisent en parallèle.
#[derive(Clone, Default)]
pub struct Store {
    locks: Arc<StdMutex<HashMap<String, Arc<Mutex<()>>>>>,
    offline: bool,
}

impl Store {
    pub fn new(offline: bool) -> Self {
        Store {
            locks: Arc::default(),
            offline,
        }
    }

    fn lock(&self, name: &str) -> Arc<Mutex<()>> {
        let mut locks = self.locks.lock().expect("verrou du cache");
        locks.entry(name.to_string()).or_default().clone()
    }

    /// Verrou exclusif sur une image, pour préparer son noyau sans course.
    pub async fn exclusive(&self, name: &str) -> tokio::sync::OwnedMutexGuard<()> {
        self.lock(name).lock_owned().await
    }

    /// Image locale vérifiée, téléchargée si besoin.
    pub async fn ensure(&self, name: &str, ev: &Emitter) -> Result<Image, String> {
        let def = find(name).ok_or_else(|| {
            format!(
                "image inconnue « {name} » (connues : {})",
                names().join(", ")
            )
        })?;
        let lock = self.lock(name);
        let _guard = lock.lock().await;
        let dir = dirs::images().join(name);
        fs::create_dir_all(&dir).map_err(|e| format!("{} : {e}", dir.display()))?;
        // Verrou entre processus : deux distroproof ne téléchargent pas la
        // même image en même temps.
        let lock_file = fs::File::create(dir.join(".lock")).map_err(|e| e.to_string())?;
        let _flock = tokio::task::spawn_blocking(move || {
            crate::sys::lock_exclusive(&lock_file).map(|()| lock_file)
        })
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("verrou du cache : {e}"))?;
        let current_path = dir.join("current.json");
        let current: Option<Resolved> = fs::read_to_string(&current_path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok());

        let resolved = if self.offline {
            current
                .clone()
                .ok_or_else(|| format!("{name} : absente du cache (mode hors ligne)"))?
        } else {
            match resolve(def).await {
                Ok(r) => r,
                Err(err) => match &current {
                    Some(c) if dir.join(&c.file).is_file() => {
                        ev.info(format!(
                            "résolution impossible ({err}), image du cache utilisée"
                        ));
                        c.clone()
                    }
                    _ => return Err(format!("{name} : {err}")),
                },
            }
        };

        let path = dir.join(&resolved.file);
        let verified = dir.join(format!("{}.verified", resolved.file));
        let cached = path.is_file()
            && fs::read_to_string(&verified).is_ok_and(|v| v.trim() == resolved.checksum);
        if !cached {
            download(&resolved, &path, ev).await?;
            ev.emit(VmEvent::Verifying);
            let part = path.with_extension("part");
            let expected = resolved.checksum.clone();
            let part2 = part.clone();
            let actual = tokio::task::spawn_blocking(move || hash_file(&part2, expected.len()))
                .await
                .map_err(|e| e.to_string())?
                .map_err(|e| format!("lecture de {} : {e}", part.display()))?;
            if actual != resolved.checksum {
                let _ = fs::remove_file(&part);
                return Err(format!(
                    "{} : checksum invalide (attendu {}, obtenu {actual})",
                    resolved.file, resolved.checksum
                ));
            }
            fs::rename(&part, &path).map_err(|e| e.to_string())?;
            // Le noyau extrait d'une version précédente n'est plus valable.
            let _ = fs::remove_dir_all(path.with_extension("kernel"));
            fs::write(&verified, &resolved.checksum).map_err(|e| e.to_string())?;
        }
        if current.as_ref() != Some(&resolved) {
            let json = serde_json::to_string_pretty(&resolved).expect("sérialisable");
            fs::write(&current_path, json).map_err(|e| e.to_string())?;
        }
        remove_stale(&dir, &resolved.file);
        ev.emit(VmEvent::ImageReady {
            file: resolved.file.clone(),
            cached,
        });
        Ok(Image {
            name: name.to_string(),
            path,
            file: resolved.file,
            uefi: def.uefi,
        })
    }
}

/// Supprime les anciennes versions d'une image pour garder le cache propre.
fn remove_stale(dir: &Path, keep: &str) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let keep_kernel = Path::new(keep).with_extension("kernel");
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let wanted = name == keep
            || name == "current.json"
            || name == ".lock"
            || name == format!("{keep}.verified")
            || Path::new(&*name) == keep_kernel
            || name.ends_with(".part");
        if !wanted {
            let path = entry.path();
            if path.is_dir() {
                let _ = fs::remove_dir_all(&path);
            } else {
                let _ = fs::remove_file(&path);
            }
        }
    }
}

async fn resolve(def: &ImageDef) -> Result<Resolved, String> {
    match &def.source {
        Source::Direct { url, checksums } => {
            let file = url.rsplit('/').next().unwrap_or(url).to_string();
            let sums = fetch_text(checksums).await?;
            let checksum = find_checksum(&sums, &file)
                .ok_or_else(|| format!("checksum de {file} absent de {checksums}"))?;
            Ok(Resolved {
                url: url.to_string(),
                file,
                checksum,
            })
        }
        Source::Listing {
            index,
            prefix,
            suffix,
            checksum_suffix,
        } => {
            let html = fetch_text(index).await?;
            let file = latest_listed(&html, prefix, suffix)
                .ok_or_else(|| format!("aucun fichier {prefix}*{suffix} dans {index}"))?;
            let url = format!("{index}{file}");
            let sums = fetch_text(&format!("{url}{checksum_suffix}")).await?;
            let checksum = find_checksum(&sums, &file)
                .ok_or_else(|| format!("checksum de {file} introuvable"))?;
            Ok(Resolved {
                url,
                file,
                checksum,
            })
        }
        Source::FedoraLatest => {
            // Le miroir principal plutôt que le redirecteur
            // download.fedoraproject.org : un miroir pas encore synchronisé
            // liste la nouvelle release sans ses images.
            let base = "https://dl.fedoraproject.org/pub/fedora/linux/releases/";
            let html = fetch_text(base).await?;
            let mut releases: Vec<u32> = hrefs(&html)
                .filter_map(|h| h.strip_suffix('/')?.parse().ok())
                .collect();
            releases.sort_unstable_by(|a, b| b.cmp(a));
            let mut last = "aucune release listée".to_string();
            for release in releases.into_iter().take(3) {
                let index = format!("{base}{release}/Cloud/x86_64/images/");
                let html = match fetch_text(&index).await {
                    Ok(html) => html,
                    Err(e) => {
                        last = e;
                        continue;
                    }
                };
                let Some(file) = latest_listed(&html, "Fedora-Cloud-Base-Generic-", ".qcow2")
                else {
                    last = format!("pas d'image Cloud Generic dans {index}");
                    continue;
                };
                let Some(sums_file) = hrefs(&html).find(|h| h.ends_with("-CHECKSUM")) else {
                    last = format!("pas de fichier CHECKSUM dans {index}");
                    continue;
                };
                let sums = fetch_text(&format!("{index}{sums_file}")).await?;
                let checksum = find_checksum(&sums, &file)
                    .ok_or_else(|| format!("checksum de {file} absent de {sums_file}"))?;
                return Ok(Resolved {
                    url: format!("{index}{file}"),
                    file,
                    checksum,
                });
            }
            Err(format!(
                "aucune release Fedora avec une image Cloud Generic ({last})"
            ))
        }
    }
}

/// Liens `href="…"` d'un index HTML.
fn hrefs(html: &str) -> impl Iterator<Item = &str> {
    html.split("href=\"")
        .skip(1)
        .filter_map(|rest| rest.split('"').next())
}

/// Fichier listé le plus récent selon un tri de versions naturel.
fn latest_listed(html: &str, prefix: &str, suffix: &str) -> Option<String> {
    let mut files: Vec<&str> = hrefs(html)
        .map(|h| h.rsplit('/').next().unwrap_or(h))
        .filter(|h| h.starts_with(prefix) && h.ends_with(suffix))
        .collect();
    files.sort_by(|a, b| natural_cmp(a, b));
    files.dedup();
    files.last().map(|s| s.to_string())
}

/// Compare en traitant les suites de chiffres comme des nombres (3.9 < 3.10).
fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    fn chunks(s: &str) -> Vec<Result<u64, String>> {
        let mut out = Vec::new();
        let mut cur = String::new();
        let mut digit = false;
        for c in s.chars() {
            if c.is_ascii_digit() != digit && !cur.is_empty() {
                out.push(if digit {
                    Ok(cur.parse().unwrap_or(0))
                } else {
                    Err(std::mem::take(&mut cur))
                });
                cur.clear();
            }
            digit = c.is_ascii_digit();
            cur.push(c);
        }
        if !cur.is_empty() {
            out.push(if digit {
                Ok(cur.parse().unwrap_or(0))
            } else {
                Err(cur)
            });
        }
        out
    }
    chunks(a).cmp(&chunks(b))
}

/// Cherche le checksum de `file` dans un fichier de sommes, quel qu'en soit
/// le format : GNU (`hash  fichier`, `hash *fichier`), BSD
/// (`SHA256 (fichier) = hash`) ou un hash seul.
pub fn find_checksum(sums: &str, file: &str) -> Option<String> {
    let is_hash =
        |w: &str| (w.len() == 64 || w.len() == 128) && w.chars().all(|c| c.is_ascii_hexdigit());
    let lines: Vec<&str> = sums
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .collect();
    for line in &lines {
        let names_file = line
            .split(|c: char| c.is_whitespace() || c == '(' || c == ')' || c == '*')
            .any(|w| w == file);
        if names_file {
            if let Some(hash) = line.split_whitespace().find(|w| is_hash(w)) {
                return Some(hash.to_ascii_lowercase());
            }
        }
    }
    match lines.as_slice() {
        [single] => single
            .split_whitespace()
            .next()
            .filter(|w| is_hash(w))
            .map(str::to_ascii_lowercase),
        _ => None,
    }
}

fn hash_file(path: &Path, hex_len: usize) -> io::Result<String> {
    let mut file = fs::File::open(path)?;
    let mut buf = vec![0u8; 1 << 20];
    let mut sha256 = Sha256::new();
    let mut sha512 = Sha512::new();
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        if hex_len == 128 {
            sha512.update(&buf[..n]);
        } else {
            sha256.update(&buf[..n]);
        }
    }
    let digest: Vec<u8> = if hex_len == 128 {
        sha512.finalize().to_vec()
    } else {
        sha256.finalize().to_vec()
    };
    Ok(digest.iter().map(|b| format!("{b:02x}")).collect())
}

async fn fetch_text(url: &str) -> Result<String, String> {
    let out = Command::new("curl")
        .args(["-fsSL", "--retry", "2", "--connect-timeout", "15", url])
        .kill_on_drop(true)
        .output()
        .await
        .map_err(|e| format!("curl introuvable : {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "{url} : {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

async fn content_length(url: &str) -> Option<u64> {
    let out = Command::new("curl")
        .args(["-fsSIL", "--connect-timeout", "15", url])
        .output()
        .await
        .ok()?;
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .rev()
        .find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.eq_ignore_ascii_case("content-length")
                .then(|| v.trim().parse().ok())
                .flatten()
        })
}

/// Télécharge dans `<fichier>.part`, avec reprise si un fichier partiel existe.
async fn download(resolved: &Resolved, path: &Path, ev: &Emitter) -> Result<(), String> {
    let part = path.with_extension("part");
    let total = content_length(&resolved.url).await;
    ev.emit(VmEvent::Downloading { done: 0, total });
    let mut child = Command::new("curl")
        .args([
            "-fsSL",
            "--retry",
            "3",
            "--connect-timeout",
            "15",
            "-C",
            "-",
            "-o",
        ])
        .arg(&part)
        .arg(&resolved.url)
        .kill_on_drop(true)
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("curl introuvable : {e}"))?;
    let mut tick = tokio::time::interval(Duration::from_millis(500));
    let status = loop {
        tokio::select! {
            status = child.wait() => break status.map_err(|e| e.to_string())?,
            _ = tick.tick() => {
                let done = fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
                ev.emit(VmEvent::Downloading { done, total });
            }
        }
    };
    if !status.success() {
        let mut err = String::new();
        if let Some(mut stderr) = child.stderr.take() {
            use tokio::io::AsyncReadExt;
            let _ = stderr.read_to_string(&mut err).await;
        }
        // Code 33 : le serveur refuse la reprise, on repart de zéro au prochain essai.
        if status.code() == Some(33) {
            let _ = fs::remove_file(&part);
        }
        return Err(format!(
            "téléchargement de {} : {}",
            resolved.url,
            err.trim()
        ));
    }
    if let Some(total) = total {
        ev.emit(VmEvent::Downloading {
            done: total,
            total: Some(total),
        });
    }
    Ok(())
}

/// État du cache, pour `distroproof images`.
pub struct CacheEntry {
    pub name: &'static str,
    pub description: &'static str,
    pub file: Option<String>,
    pub bytes: u64,
    pub direct_kernel: bool,
}

pub fn cache_status() -> Vec<CacheEntry> {
    CATALOG
        .iter()
        .map(|def| {
            let dir = dirs::images().join(def.name);
            let current: Option<Resolved> = fs::read_to_string(dir.join("current.json"))
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok());
            let file = current.map(|c| c.file).filter(|f| dir.join(f).is_file());
            let bytes = dir_size(&dir);
            let direct_kernel = file.as_ref().is_some_and(|f| {
                DirectKernel::load(&dir.join(f).with_extension("kernel")).is_some()
            });
            CacheEntry {
                name: def.name,
                description: def.description,
                file,
                bytes,
                direct_kernel,
            }
        })
        .collect()
}

fn dir_size(path: &Path) -> u64 {
    let Ok(entries) = fs::read_dir(path) else {
        return 0;
    };
    entries
        .flatten()
        .map(|e| match e.metadata() {
            Ok(m) if m.is_dir() => dir_size(&e.path()),
            Ok(m) => m.len(),
            Err(_) => 0,
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksum_formats() {
        let gnu = "aaaa  other.qcow2\n\
            c2ffec728a10586010bd2a2edfde80d890326066fd4abb06ef98deefa29a8bfe8963c2a13046cda1cb00a275a486e740087851d2c89333c8a1dde1b1234b8e5f  debian-12-generic-amd64.qcow2\n";
        assert!(find_checksum(gnu, "debian-12-generic-amd64.qcow2")
            .unwrap()
            .starts_with("c2ffec"));
        let star = "3f1b8a5c47ce5c8a8b3b7e5bc5c6e6d1a9e1f5a1b1c1d1e1f1a1b1c1d1e1f1a1 *ubuntu-22.04-server-cloudimg-amd64.img";
        assert!(find_checksum(star, "ubuntu-22.04-server-cloudimg-amd64.img").is_some());
        let bsd = "# Rocky-9-GenericCloud-Base.latest.x86_64.qcow2: 645988352 bytes\n\
            SHA256 (Rocky-9-GenericCloud-Base.latest.x86_64.qcow2) = 92c206cc6f790c61583247eefe87890f8828420662c17cacf247cec78ab4eec8";
        assert_eq!(
            find_checksum(bsd, "Rocky-9-GenericCloud-Base.latest.x86_64.qcow2").unwrap(),
            "92c206cc6f790c61583247eefe87890f8828420662c17cacf247cec78ab4eec8"
        );
        let alone = "8d756f6fc7653daa4fb4e2e213d8a66007bcb1e5a846e28891af62c47b90685c694486c2746099ad99e9e8f5278db76b69d11dfe1e9361aa4c8406df16929a9c\n";
        assert!(find_checksum(alone, "whatever.qcow2").is_some());
        assert!(find_checksum("nothing here", "x").is_none());
    }

    #[test]
    fn picks_latest_listed_file() {
        let html = r#"<a href="generic_alpine-3.9.0-x86_64-bios-cloudinit-r0.qcow2">
            <a href="generic_alpine-3.24.1-x86_64-bios-cloudinit-r0.qcow2">
            <a href="generic_alpine-3.24.1-x86_64-bios-cloudinit-r0.qcow2.sha512">
            <a href="generic_alpine-3.24.1-x86_64-bios-tiny-r0.qcow2">"#;
        assert_eq!(
            latest_listed(html, "generic_alpine-", "-x86_64-bios-cloudinit-r0.qcow2").as_deref(),
            Some("generic_alpine-3.24.1-x86_64-bios-cloudinit-r0.qcow2")
        );
    }

    #[test]
    fn cleans_cmdline() {
        assert_eq!(
            clean_cmdline(
                "BOOT_IMAGE=/boot/vmlinuz-6.1.0 root=PARTUUID=abc ro console=tty0 console=ttyS0,115200 quiet\n"
            ),
            "root=PARTUUID=abc ro"
        );
    }
}
