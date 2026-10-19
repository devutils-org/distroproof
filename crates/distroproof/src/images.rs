//! Catalog of official cloud images, and local cache.
//!
//! Cloud images (qcow2) contain the distro's original kernel, initramfs and
//! config: that kernel is what runs in the VM. They are downloaded with
//! `curl`, verified against the checksum published by the distro, then kept
//! in `$XDG_CACHE_HOME/distroproof/images/<image>/`.

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

/// Where to find the latest version of an image.
enum Source {
    /// Fixed URL and the checksums file published next to it.
    Direct {
        url: &'static str,
        checksums: &'static str,
    },
    /// HTML index: the most recent file that starts with `prefix` and ends
    /// with `suffix`. The checksum is in `<file><checksum_suffix>`.
    Listing {
        index: &'static str,
        prefix: &'static str,
        suffix: &'static str,
        checksum_suffix: &'static str,
    },
    /// A Fedora release that publishes a Cloud Generic image for `arch`: the
    /// latest when `skip` is 0, the one before it when 1.
    Fedora { skip: usize, arch: Arch },
    /// Gentoo's latest cloud-init disk image, named by the autobuilds'
    /// `latest-<build>.txt`, with its `.sha256` beside it.
    GentooLatest { build: &'static str },
    /// NixOS publishes no generic cloud image; Hydra builds a Proxmox one
    /// (cloud-init, NoCloud) as a zstd-compressed VMA backup: the latest
    /// finished build of `job` in the `nixos` project's `jobset`, its
    /// product's sha256 from Hydra's JSON, its disk extracted to a qcow2
    /// beside it once verified.
    HydraVma {
        jobset: &'static str,
        job: &'static str,
    },
}

/// The machine an image is built for. An image of another architecture than
/// the host's runs under emulation (TCG): slower, and the same kernel and
/// userspace a machine of that architecture boots.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Arch {
    X86_64,
    Aarch64,
}

impl Arch {
    /// The architecture distroproof runs on.
    pub fn host() -> Arch {
        if cfg!(target_arch = "aarch64") {
            Arch::Aarch64
        } else {
            Arch::X86_64
        }
    }

    /// `uname -m`'s name, which Fedora's mirrors also use.
    pub fn uname(self) -> &'static str {
        match self {
            Arch::X86_64 => "x86_64",
            Arch::Aarch64 => "aarch64",
        }
    }
}

pub struct ImageDef {
    pub name: &'static str,
    pub description: &'static str,
    source: Source,
    /// Booting from disk requires UEFI: the image's BIOS grub reboots the
    /// machine right away (Rocky 9, Debian 13). Direct boot does without it.
    /// An aarch64 machine has no BIOS: every aarch64 image says true.
    pub uefi: bool,
    pub arch: Arch,
}

pub const CATALOG: &[ImageDef] = &[
    ImageDef {
        name: "debian-10",
        description: "Debian 10 buster (archive), kernel 4.19 without BTF",
        source: Source::Direct {
            url: "https://cloud.debian.org/images/cloud/buster/latest/debian-10-generic-amd64.qcow2",
            checksums: "https://cloud.debian.org/images/cloud/buster/latest/SHA512SUMS",
        },
        uefi: false,
        arch: Arch::X86_64,
    },
    ImageDef {
        name: "debian-11",
        description: "Debian 11 bullseye, kernel 5.10",
        source: Source::Direct {
            url: "https://cloud.debian.org/images/cloud/bullseye/latest/debian-11-generic-amd64.qcow2",
            checksums: "https://cloud.debian.org/images/cloud/bullseye/latest/SHA512SUMS",
        },
        uefi: false,
        arch: Arch::X86_64,
    },
    ImageDef {
        name: "debian-12",
        description: "Debian 12 bookworm, generic kernel",
        source: Source::Direct {
            url: "https://cloud.debian.org/images/cloud/bookworm/latest/debian-12-generic-amd64.qcow2",
            checksums: "https://cloud.debian.org/images/cloud/bookworm/latest/SHA512SUMS",
        },
        uefi: false,
        arch: Arch::X86_64,
    },
    ImageDef {
        name: "debian-13",
        description: "Debian 13 trixie, generic kernel",
        source: Source::Direct {
            url: "https://cloud.debian.org/images/cloud/trixie/latest/debian-13-generic-amd64.qcow2",
            checksums: "https://cloud.debian.org/images/cloud/trixie/latest/SHA512SUMS",
        },
        // The image's BIOS grub reboots the machine as soon as the kernel loads.
        uefi: true,
        arch: Arch::X86_64,
    },
    ImageDef {
        name: "debian-sid",
        description: "Debian unstable (sid), daily image: the next kernel Debian ships",
        source: Source::Direct {
            url: "https://cloud.debian.org/images/cloud/sid/daily/latest/debian-sid-generic-amd64-daily.qcow2",
            checksums: "https://cloud.debian.org/images/cloud/sid/daily/latest/SHA512SUMS",
        },
        // As Debian 13's: the BIOS grub reboots as soon as the kernel loads.
        uefi: true,
        arch: Arch::X86_64,
    },
    ImageDef {
        name: "ubuntu-18.04",
        description: "Ubuntu 18.04 bionic, kernel 4.15 without BTF",
        source: Source::Direct {
            url: "https://cloud-images.ubuntu.com/releases/18.04/release/ubuntu-18.04-server-cloudimg-amd64.img",
            checksums: "https://cloud-images.ubuntu.com/releases/18.04/release/SHA256SUMS",
        },
        uefi: false,
        arch: Arch::X86_64,
    },
    ImageDef {
        name: "ubuntu-20.04",
        description: "Ubuntu 20.04 focal, kernel 5.4",
        source: Source::Direct {
            url: "https://cloud-images.ubuntu.com/releases/20.04/release/ubuntu-20.04-server-cloudimg-amd64.img",
            checksums: "https://cloud-images.ubuntu.com/releases/20.04/release/SHA256SUMS",
        },
        uefi: false,
        arch: Arch::X86_64,
    },
    ImageDef {
        name: "ubuntu-22.04",
        description: "Ubuntu 22.04 jammy",
        source: Source::Direct {
            url: "https://cloud-images.ubuntu.com/releases/22.04/release/ubuntu-22.04-server-cloudimg-amd64.img",
            checksums: "https://cloud-images.ubuntu.com/releases/22.04/release/SHA256SUMS",
        },
        uefi: false,
        arch: Arch::X86_64,
    },
    ImageDef {
        name: "ubuntu-24.04",
        description: "Ubuntu 24.04 noble",
        source: Source::Direct {
            url: "https://cloud-images.ubuntu.com/releases/24.04/release/ubuntu-24.04-server-cloudimg-amd64.img",
            checksums: "https://cloud-images.ubuntu.com/releases/24.04/release/SHA256SUMS",
        },
        uefi: false,
        arch: Arch::X86_64,
    },
    ImageDef {
        name: "rocky-9",
        description: "Rocky Linux 9 (RHEL 9, kernel 5.14)",
        source: Source::Direct {
            url: "https://dl.rockylinux.org/pub/rocky/9/images/x86_64/Rocky-9-GenericCloud-Base.latest.x86_64.qcow2",
            checksums: "https://dl.rockylinux.org/pub/rocky/9/images/x86_64/Rocky-9-GenericCloud-Base.latest.x86_64.qcow2.CHECKSUM",
        },
        uefi: true,
        arch: Arch::X86_64,
    },
    ImageDef {
        name: "rocky-8",
        description: "Rocky Linux 8 (RHEL 8, kernel 4.18)",
        source: Source::Direct {
            url: "https://dl.rockylinux.org/pub/rocky/8/images/x86_64/Rocky-8-GenericCloud-Base.latest.x86_64.qcow2",
            checksums: "https://dl.rockylinux.org/pub/rocky/8/images/x86_64/Rocky-8-GenericCloud-Base.latest.x86_64.qcow2.CHECKSUM",
        },
        uefi: false,
        arch: Arch::X86_64,
    },
    ImageDef {
        name: "fedora-latest",
        description: "Latest Fedora Cloud Base Generic",
        source: Source::Fedora {
            skip: 0,
            arch: Arch::X86_64,
        },
        uefi: false,
        arch: Arch::X86_64,
    },
    ImageDef {
        name: "fedora-previous",
        description: "The Fedora release before the latest, Cloud Base Generic",
        source: Source::Fedora {
            skip: 1,
            arch: Arch::X86_64,
        },
        uefi: false,
        arch: Arch::X86_64,
    },
    ImageDef {
        name: "arch",
        description: "Arch Linux, official cloud image",
        source: Source::Direct {
            url: "https://geo.mirror.pkgbuild.com/images/latest/Arch-Linux-x86_64-cloudimg.qcow2",
            checksums: "https://geo.mirror.pkgbuild.com/images/latest/Arch-Linux-x86_64-cloudimg.qcow2.SHA256",
        },
        uefi: false,
        arch: Arch::X86_64,
    },
    ImageDef {
        name: "gentoo",
        description: "Gentoo, the autobuilds' latest cloud-init disk image (OpenRC, dist-kernel)",
        source: Source::GentooLatest {
            build: "di-amd64-cloudinit",
        },
        // A GPT disk with an EFI system partition and no BIOS boot code:
        // under SeaBIOS the machine read its first sector and spun there.
        uefi: true,
        arch: Arch::X86_64,
    },
    ImageDef {
        name: "alpine",
        description: "Alpine stable, generic BIOS image with cloud-init",
        source: Source::Listing {
            index: "https://dl-cdn.alpinelinux.org/alpine/latest-stable/releases/cloud/",
            prefix: "generic_alpine-",
            suffix: "-x86_64-bios-cloudinit-r0.qcow2",
            checksum_suffix: ".sha512",
        },
        uefi: false,
        arch: Arch::X86_64,
    },
    ImageDef {
        name: "nixos",
        description: "NixOS 25.05, Hydra's Proxmox image (cloud-init, NoCloud), kernel 6.12",
        source: Source::HydraVma {
            jobset: "release-25.05",
            job: "nixos.proxmoxImage.x86_64-linux",
        },
        uefi: false,
        arch: Arch::X86_64,
    },
    // --- aarch64, under emulation on an x86_64 host (TCG): the same kernels
    // and userspaces an arm64 machine boots, from the same publishers.
    ImageDef {
        name: "debian-13-arm64",
        description: "Debian 13 trixie, generic kernel, arm64",
        source: Source::Direct {
            url: "https://cloud.debian.org/images/cloud/trixie/latest/debian-13-generic-arm64.qcow2",
            checksums: "https://cloud.debian.org/images/cloud/trixie/latest/SHA512SUMS",
        },
        uefi: true,
        arch: Arch::Aarch64,
    },
    ImageDef {
        name: "ubuntu-24.04-arm64",
        description: "Ubuntu 24.04 LTS, arm64, kernel 6.8",
        source: Source::Direct {
            url: "https://cloud-images.ubuntu.com/releases/24.04/release/ubuntu-24.04-server-cloudimg-arm64.img",
            checksums: "https://cloud-images.ubuntu.com/releases/24.04/release/SHA256SUMS",
        },
        uefi: true,
        arch: Arch::Aarch64,
    },
    ImageDef {
        name: "fedora-latest-aarch64",
        description: "Latest Fedora Cloud Base Generic, aarch64",
        source: Source::Fedora {
            skip: 0,
            arch: Arch::Aarch64,
        },
        uefi: true,
        arch: Arch::Aarch64,
    },
];

pub fn find(name: &str) -> Option<&'static ImageDef> {
    CATALOG.iter().find(|d| d.name == name)
}

pub fn names() -> Vec<&'static str> {
    CATALOG.iter().map(|d| d.name).collect()
}

/// Exact version of an image, as resolved online.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Resolved {
    url: String,
    file: String,
    /// Published hex checksum (sha256 or sha512 depending on its length).
    checksum: String,
}

/// Image ready to use.
#[derive(Debug, Clone)]
pub struct Image {
    pub name: String,
    pub path: PathBuf,
    pub file: String,
    pub uefi: bool,
    pub arch: Arch,
}

impl Image {
    /// Kernel and initramfs extracted from the image for direct boot.
    pub fn kernel_dir(&self) -> PathBuf {
        self.path.with_extension("kernel")
    }

    pub fn direct_kernel(&self) -> Option<DirectKernel> {
        DirectKernel::load(&self.kernel_dir())
    }
}

/// Kernel extracted by the agent during a first boot from disk.
#[derive(Debug, Clone)]
pub struct DirectKernel {
    pub kernel: PathBuf,
    pub initrd: Option<PathBuf>,
    /// Original command line, without `BOOT_IMAGE` or `console=`.
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

/// Removes from the original command line what distroproof sets itself.
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

/// Cache shared by every target: an image is downloaded only once, even when
/// several targets use it in parallel.
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
        let mut locks = self.locks.lock().expect("cache lock");
        locks.entry(name.to_string()).or_default().clone()
    }

    /// Exclusive lock on an image, to prepare its kernel without races.
    pub async fn exclusive(&self, name: &str) -> tokio::sync::OwnedMutexGuard<()> {
        self.lock(name).lock_owned().await
    }

    /// Verified local image, downloaded if needed.
    pub async fn ensure(&self, name: &str, ev: &Emitter) -> Result<Image, String> {
        let def = find(name)
            .ok_or_else(|| format!("unknown image \"{name}\" (known: {})", names().join(", ")))?;
        let lock = self.lock(name);
        let _guard = lock.lock().await;
        let dir = dirs::images().join(name);
        fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        // Inter-process lock: two distroproof instances never download the
        // same image at the same time.
        let lock_file = fs::File::create(dir.join(".lock")).map_err(|e| e.to_string())?;
        let _flock = tokio::task::spawn_blocking(move || {
            crate::sys::lock_exclusive(&lock_file).map(|()| lock_file)
        })
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("cache lock: {e}"))?;
        let current_path = dir.join("current.json");
        let current: Option<Resolved> = fs::read_to_string(&current_path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok());

        let resolved = if self.offline {
            current
                .clone()
                .ok_or_else(|| format!("{name}: not in the cache (offline mode)"))?
        } else {
            match resolve(def).await {
                Ok(r) => r,
                Err(err) => match &current {
                    Some(c) if dir.join(&c.file).is_file() => {
                        ev.info(format!("cannot resolve ({err}), using the cached image"));
                        c.clone()
                    }
                    _ => return Err(format!("{name}: {err}")),
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
                .map_err(|e| format!("reading {}: {e}", part.display()))?;
            if actual != resolved.checksum {
                let _ = fs::remove_file(&part);
                return Err(format!(
                    "{}: checksum mismatch (expected {}, got {actual})",
                    resolved.file, resolved.checksum
                ));
            }
            fs::rename(&part, &path).map_err(|e| e.to_string())?;
            // A kernel extracted from a previous version is no longer valid.
            let _ = fs::remove_dir_all(path.with_extension("kernel"));
            fs::write(&verified, &resolved.checksum).map_err(|e| e.to_string())?;
        }
        if current.as_ref() != Some(&resolved) {
            let json = serde_json::to_string_pretty(&resolved).expect("serializable");
            fs::write(&current_path, json).map_err(|e| e.to_string())?;
        }
        let path = match def.source {
            Source::HydraVma { .. } => {
                let qcow2 = dir.join(format!("{}.qcow2", resolved.file));
                if !qcow2.is_file() {
                    ev.info("extracting the VMA's disk to a qcow2".to_string());
                    let (from, to) = (path.clone(), qcow2.clone());
                    tokio::task::spawn_blocking(move || vma_to_qcow2(&from, &to))
                        .await
                        .map_err(|e| e.to_string())??;
                }
                qcow2
            }
            _ => path,
        };
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
            arch: def.arch,
        })
    }
}

/// The disk of a zstd-compressed Proxmox VMA backup, as a qcow2: `zstd -dc`
/// streams the archive, its first device's 64 KiB clusters are written to a
/// sparse raw file at their offsets (a cluster's mask says which of its 4 KiB
/// blocks follow; the rest are zero), and `qemu-img` converts that.
fn vma_to_qcow2(archive: &Path, qcow2: &Path) -> Result<(), String> {
    use std::io::{Seek, SeekFrom, Write};
    const CLUSTER: u64 = 65536;
    const BLOCK: usize = 4096;
    let mut zstd = std::process::Command::new("zstd")
        .args(["-dc", "--"])
        .arg(archive)
        .stdout(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("zstd, to read a VMA archive: {e}"))?;
    let mut input = io::BufReader::with_capacity(1 << 20, zstd.stdout.take().expect("piped"));
    let mut header = vec![0u8; 4096 + 256 * 32];
    input
        .read_exact(&mut header)
        .map_err(|e| format!("{}: {e}", archive.display()))?;
    if &header[..4] != b"VMA\0" {
        return Err(format!("{}: not a VMA archive", archive.display()));
    }
    let be32 = |at: usize| u32::from_be_bytes(header[at..at + 4].try_into().expect("4 bytes"));
    let header_size = be32(56) as usize;
    let size = u64::from_be_bytes(header[4096 + 32 + 8..4096 + 32 + 16].try_into().expect("8 bytes"));
    if size == 0 {
        return Err(format!("{}: the VMA's first device has no size", archive.display()));
    }
    let mut rest = vec![0u8; header_size.saturating_sub(header.len())];
    input.read_exact(&mut rest).map_err(|e| e.to_string())?;
    let raw = qcow2.with_extension("raw");
    let mut out = fs::File::create(&raw).map_err(|e| format!("{}: {e}", raw.display()))?;
    out.set_len(size).map_err(|e| e.to_string())?;
    let mut extent = [0u8; 512];
    let mut block = [0u8; BLOCK];
    loop {
        match input.read_exact(&mut extent) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(e.to_string()),
        }
        if &extent[..4] != b"VMAE" {
            return Err(format!("{}: a VMA extent without its magic", archive.display()));
        }
        for i in 0..59 {
            let at = 40 + i * 8;
            let info = u64::from_be_bytes(extent[at..at + 8].try_into().expect("8 bytes"));
            let (mask, device, cluster) = (info >> 48, (info >> 32) & 0xff, info & 0xffff_ffff);
            for b in 0..16u64 {
                if mask & (1 << b) == 0 {
                    continue;
                }
                input.read_exact(&mut block).map_err(|e| e.to_string())?;
                if device == 1 {
                    out.seek(SeekFrom::Start(cluster * CLUSTER + b * BLOCK as u64))
                        .and_then(|_| out.write_all(&block))
                        .map_err(|e| e.to_string())?;
                }
            }
        }
    }
    drop(out);
    let _ = zstd.wait();
    let converted = std::process::Command::new(crate::qemu::qemu_img_binary())
        .args(["convert", "-q", "-f", "raw", "-O", "qcow2"])
        .arg(&raw)
        .arg(qcow2)
        .status()
        .map_err(|e| format!("qemu-img: {e}"))?;
    let _ = fs::remove_file(&raw);
    if !converted.success() {
        let _ = fs::remove_file(qcow2);
        return Err(format!("qemu-img convert of {} failed", raw.display()));
    }
    Ok(())
}

/// Removes old versions of an image to keep the cache clean.
fn remove_stale(dir: &Path, keep: &str) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let keep_kernel = Path::new(keep).with_extension("kernel");
    // A Hydra VMA's disk, extracted beside it, and the kernel taken from it.
    let converted = format!("{keep}.qcow2");
    let converted_kernel = Path::new(&converted).with_extension("kernel");
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let wanted = name == keep
            || name == "current.json"
            || name == ".lock"
            || name == format!("{keep}.verified")
            || Path::new(&*name) == keep_kernel
            || name == converted
            || Path::new(&*name) == converted_kernel
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
                .ok_or_else(|| format!("checksum of {file} missing from {checksums}"))?;
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
                .ok_or_else(|| format!("no {prefix}*{suffix} file in {index}"))?;
            let url = format!("{index}{file}");
            let sums = fetch_text(&format!("{url}{checksum_suffix}")).await?;
            let checksum = find_checksum(&sums, &file)
                .ok_or_else(|| format!("checksum of {file} not found"))?;
            Ok(Resolved {
                url,
                file,
                checksum,
            })
        }
        Source::GentooLatest { build } => {
            let base = "https://distfiles.gentoo.org/releases/amd64/autobuilds/";
            let latest = fetch_text(&format!("{base}latest-{build}.txt")).await?;
            let path = gentoo_latest_path(&latest)
                .ok_or_else(|| format!("no .qcow2 named in {base}latest-{build}.txt"))?;
            let url = format!("{base}{path}");
            let file = path.rsplit('/').next().unwrap_or(&path).to_string();
            let sums = fetch_text(&format!("{url}.sha256")).await?;
            let checksum = find_checksum(&sums, &file)
                .ok_or_else(|| format!("checksum of {file} missing from {url}.sha256"))?;
            Ok(Resolved {
                url,
                file,
                checksum,
            })
        }
        Source::HydraVma { jobset, job } => {
            let latest = format!("https://hydra.nixos.org/job/nixos/{jobset}/{job}/latest-finished");
            let build: serde_json::Value = serde_json::from_str(&fetch_json(&latest).await?)
                .map_err(|e| format!("{latest}: {e}"))?;
            let id = build["id"]
                .as_u64()
                .ok_or_else(|| format!("{latest}: no build id"))?;
            let product = &build["buildproducts"]["1"];
            let (Some(file), Some(checksum)) =
                (product["name"].as_str(), product["sha256hash"].as_str())
            else {
                return Err(format!("{latest}: no product with a name and a sha256"));
            };
            Ok(Resolved {
                url: format!("https://hydra.nixos.org/build/{id}/download/1/{file}"),
                file: file.to_string(),
                checksum: checksum.to_string(),
            })
        }
        Source::Fedora { skip, arch } => {
            // The primary mirror rather than the download.fedoraproject.org
            // redirector: a mirror that is not yet in sync lists the new
            // release without its images.
            let base = "https://dl.fedoraproject.org/pub/fedora/linux/releases/";
            let html = fetch_text(base).await?;
            let mut releases: Vec<u32> = hrefs(&html)
                .filter_map(|h| h.strip_suffix('/')?.parse().ok())
                .collect();
            releases.sort_unstable_by(|a, b| b.cmp(a));
            let mut last = "no release listed".to_string();
            let mut skipped = 0;
            for release in releases.into_iter().take(3 + skip) {
                let index = format!("{base}{release}/Cloud/{}/images/", arch.uname());
                let html = match fetch_text(&index).await {
                    Ok(html) => html,
                    Err(e) => {
                        last = e;
                        continue;
                    }
                };
                let Some(file) = latest_listed(&html, "Fedora-Cloud-Base-Generic-", ".qcow2")
                else {
                    last = format!("no Cloud Generic image in {index}");
                    continue;
                };
                let Some(sums_file) = hrefs(&html).find(|h| h.ends_with("-CHECKSUM")) else {
                    last = format!("no CHECKSUM file in {index}");
                    continue;
                };
                // A release counts once it publishes an image: `skip` passes
                // over the newest such releases, not over directories.
                if skipped < *skip {
                    skipped += 1;
                    continue;
                }
                let sums = fetch_text(&format!("{index}{sums_file}")).await?;
                let checksum = find_checksum(&sums, &file)
                    .ok_or_else(|| format!("checksum of {file} missing from {sums_file}"))?;
                return Ok(Resolved {
                    url: format!("{index}{file}"),
                    file,
                    checksum,
                });
            }
            Err(format!(
                "no Fedora release with a Cloud Generic image ({last})"
            ))
        }
    }
}

/// The image a Gentoo autobuilds `latest-<build>.txt` names: one
/// `<directory>/<file> <size>` line, inside a PGP-signed text.
fn gentoo_latest_path(latest: &str) -> Option<String> {
    latest
        .lines()
        .map(str::trim)
        .filter(|line| !line.starts_with('#'))
        .filter_map(|line| line.split_whitespace().next())
        .find(|path| path.ends_with(".qcow2"))
        .map(str::to_string)
}

/// `href="…"` links of an HTML index.
fn hrefs(html: &str) -> impl Iterator<Item = &str> {
    html.split("href=\"")
        .skip(1)
        .filter_map(|rest| rest.split('"').next())
}

/// Most recent listed file, by natural version sort.
fn latest_listed(html: &str, prefix: &str, suffix: &str) -> Option<String> {
    let mut files: Vec<&str> = hrefs(html)
        .map(|h| h.rsplit('/').next().unwrap_or(h))
        .filter(|h| h.starts_with(prefix) && h.ends_with(suffix))
        .collect();
    files.sort_by(|a, b| natural_cmp(a, b));
    files.dedup();
    files.last().map(|s| s.to_string())
}

/// Compares treating digit runs as numbers (3.9 < 3.10).
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

/// Finds the checksum of `file` in a sums file, whatever its format: GNU
/// (`hash  file`, `hash *file`), BSD (`SHA256 (file) = hash`) or a bare
/// hash.
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
        .map_err(|e| format!("curl not found: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "{url}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// A JSON document, which Hydra serves only when asked for one.
async fn fetch_json(url: &str) -> Result<String, String> {
    let out = Command::new("curl")
        .args(["-fsSL", "--retry", "2", "--connect-timeout", "15"])
        .args(["-H", "Accept: application/json", url])
        .kill_on_drop(true)
        .output()
        .await
        .map_err(|e| format!("curl not found: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "{url}: {}",
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

/// Downloads to `<file>.part`, resuming if a partial file exists.
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
        .map_err(|e| format!("curl not found: {e}"))?;
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
        // Code 33: the server refuses to resume, start from scratch on the next try.
        if status.code() == Some(33) {
            let _ = fs::remove_file(&part);
        }
        return Err(format!("downloading {}: {}", resolved.url, err.trim()));
    }
    if let Some(total) = total {
        ev.emit(VmEvent::Downloading {
            done: total,
            total: Some(total),
        });
    }
    Ok(())
}

/// Cache status, for `distroproof images`.
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
    fn gentoo_latest_names_the_image_inside_the_signature() {
        let latest = "-----BEGIN PGP SIGNED MESSAGE-----
Hash: SHA256

# Latest as of Sun, 18 Oct 2026 21:00:01 +0000
# ts=1792357201
20261011T170049Z/di-amd64-cloudinit-20261011T170049Z.qcow2 1216151552
-----BEGIN PGP SIGNATURE-----

iQFPBAEBCAA5FiEEU05CCatJ7uHBnZYWLERpXbn2BD0FAmrVM5EbFIAAAAAABAAO
-----END PGP SIGNATURE-----
";
        assert_eq!(
            gentoo_latest_path(latest).as_deref(),
            Some("20261011T170049Z/di-amd64-cloudinit-20261011T170049Z.qcow2")
        );
        assert_eq!(gentoo_latest_path("no image here"), None);
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
