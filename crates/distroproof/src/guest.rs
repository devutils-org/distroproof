//! Ce qui est injecté dans l'invité (seed cloud-init, disque payload) et ce
//! qui en revient (disque de sortie).
//!
//! Les disques payload et sortie sont des archives tar brutes posées sur un
//! disque virtio : l'invité les lit et les écrit avec `tar -f /dev/vdX`, sans
//! système de fichiers ni module noyau particulier (9p et virtiofs manquent
//! sur les noyaux RHEL).

use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use crate::iso;
use crate::model::VmSpec;

pub const AGENT: &str = include_str!("agent.sh");

/// Taille du disque de sortie (fichier creux) : sorties, rapport, noyau extrait.
pub const OUTPUT_DISK_BYTES: u64 = 1 << 30;

/// Seed NoCloud : cloud-init extrait le payload puis lance l'agent.
pub fn seed_iso(instance: &str) -> Vec<u8> {
    let user_data = r#"#cloud-config
runcmd:
  - - sh
    - -c
    - |
      for d in /sys/block/*; do
        if [ "$(cat "$d/serial" 2>/dev/null)" = dp-payload ]; then
          mkdir -p /distroproof && tar -xf "/dev/${d##*/}" -C /distroproof && exec sh /distroproof/agent.sh
        fi
      done
      echo "@ FATAL disque payload introuvable" > /dev/ttyS1
"#;
    let meta_data = format!("instance-id: {instance}\nlocal-hostname: {instance}\n");
    iso::build(
        "cidata",
        &[
            ("user-data", user_data.as_bytes()),
            ("meta-data", meta_data.as_bytes()),
        ],
    )
}

/// Quote une valeur pour un script POSIX.
pub fn shell_quote(s: &str) -> String {
    if !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./=:,+@%".contains(c))
    {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

pub struct Payload<'a> {
    pub spec: &'a VmSpec,
    pub envprobe: &'a Path,
    pub extract_kernel: bool,
    /// Boot de préparation : aucune étape, seulement l'extraction du noyau.
    pub prepare_only: bool,
}

impl Payload<'_> {
    /// Écrit l'archive payload dans `path`.
    pub fn write(&self, path: &Path) -> Result<(), String> {
        let file = File::create(path).map_err(|e| format!("{} : {e}", path.display()))?;
        let mut tar = tar::Builder::new(file);
        tar.follow_symlinks(false);
        let spec = self.spec;
        let run_steps = !self.prepare_only;

        let mut env = String::new();
        let mut var = |k: &str, v: &str| env.push_str(&format!("{k}={}\n", shell_quote(v)));
        var("DP_USER", &spec.user);
        var("DP_CAPS", &spec.caps.join(","));
        var("DP_PROBE", if run_steps && spec.probe { "1" } else { "0" });
        var(
            "DP_EXTRACT_KERNEL",
            if self.extract_kernel { "1" } else { "0" },
        );
        let setup = spec.setup.as_deref().filter(|_| run_steps);
        let command = spec.command.as_deref().filter(|_| run_steps);
        var("DP_HOLD", if run_steps && spec.hold { "1" } else { "0" });
        var("DP_SETUP", if setup.is_some() { "1" } else { "" });
        var("DP_COMMAND", if command.is_some() { "1" } else { "" });

        let mut files: Vec<(String, Vec<u8>, u32)> = vec![
            ("agent.sh".into(), AGENT.as_bytes().to_vec(), 0o755),
            ("job/env".into(), env.into_bytes(), 0o644),
            (
                "job/probe.sh".into(),
                b"exec /distroproof/bin/envprobe --json\n".to_vec(),
                0o755,
            ),
        ];
        if let Some(setup) = setup {
            files.push(("job/setup.sh".into(), script(setup), 0o755));
        }
        if let Some(command) = command {
            files.push(("job/command.sh".into(), script(command), 0o755));
        }
        if !spec.sysctl.is_empty() {
            let lines: String = spec
                .sysctl
                .iter()
                .map(|(k, v)| format!("{k}={v}\n"))
                .collect();
            files.push(("job/sysctl".into(), lines.into_bytes(), 0o644));
        }
        if !spec.env.is_empty() {
            let lines: String = spec
                .env
                .iter()
                .map(|(k, v)| format!("{k}={}\n", shell_quote(v)))
                .collect();
            files.push(("job/userenv".into(), lines.into_bytes(), 0o644));
        }
        for (name, data, mode) in &files {
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(*mode);
            header.set_cksum();
            tar.append_data(&mut header, name, data.as_slice())
                .map_err(|e| e.to_string())?;
        }
        tar.append_path_with_name(self.envprobe, "bin/envprobe")
            .map_err(|e| format!("{} : {e}", self.envprobe.display()))?;

        let mut dir = tar::Header::new_gnu();
        dir.set_entry_type(tar::EntryType::Directory);
        dir.set_mode(0o755);
        dir.set_size(0);
        dir.set_cksum();
        tar.append_data(&mut dir, "work/", io::empty())
            .map_err(|e| e.to_string())?;
        if let (Some(workdir), true) = (&spec.workdir, run_steps) {
            for rel in work_files(workdir, &spec.exclude, &spec.include)
                .map_err(|e| format!("{} : {e}", workdir.display()))?
            {
                let full = workdir.join(&rel);
                tar.append_path_with_name(&full, Path::new("work").join(&rel))
                    .map_err(|e| format!("{} : {e}", full.display()))?;
            }
        }
        tar.into_inner()
            .and_then(|f| f.sync_all())
            .map_err(|e| e.to_string())
    }
}

fn script(body: &str) -> Vec<u8> {
    format!("#!/bin/sh\n{body}\n").into_bytes()
}

/// `*` et `?` sans traverser `/`.
fn glob(pat: &[u8], s: &[u8]) -> bool {
    match (pat.first(), s.first()) {
        (None, None) => true,
        (Some(b'*'), _) => {
            glob(&pat[1..], s) || (!s.is_empty() && s[0] != b'/' && glob(pat, &s[1..]))
        }
        (Some(b'?'), Some(c)) if *c != b'/' => glob(&pat[1..], &s[1..]),
        (Some(p), Some(c)) if p == c => glob(&pat[1..], &s[1..]),
        _ => false,
    }
}

/// Un motif désigne un chemin ou l'un de ses parents.
fn matches(pattern: &str, rel: &Path) -> bool {
    let pattern = pattern.trim_start_matches("./").trim_end_matches('/');
    let mut prefix = PathBuf::new();
    for comp in rel.components() {
        prefix.push(comp);
        if glob(pattern.as_bytes(), prefix.to_string_lossy().as_bytes()) {
            return true;
        }
    }
    false
}

/// Fichiers du répertoire de travail à copier, chemins relatifs triés.
/// `include` l'emporte sur `exclude`.
pub fn work_files(root: &Path, exclude: &[String], include: &[String]) -> io::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    let mut stack = vec![PathBuf::new()];
    while let Some(rel) = stack.pop() {
        let mut entries: Vec<_> = fs::read_dir(root.join(&rel))?.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            let child = rel.join(entry.file_name());
            let included = include.iter().any(|p| matches(p, &child))
                || include
                    .iter()
                    .any(|p| Path::new(p.trim_start_matches("./")).starts_with(&child));
            if !included && exclude.iter().any(|p| matches(p, &child)) {
                continue;
            }
            let ty = entry.file_type()?;
            if ty.is_dir() {
                stack.push(child);
            } else {
                out.push(child);
            }
        }
    }
    out.sort();
    Ok(out)
}

/// Contenu du disque de sortie écrit par l'agent.
#[derive(Debug, Default)]
pub struct GuestOutput {
    files: Vec<(String, Vec<u8>)>,
}

impl GuestOutput {
    pub fn read(path: &Path) -> io::Result<GuestOutput> {
        let mut archive = tar::Archive::new(File::open(path)?);
        let mut files = Vec::new();
        for entry in archive.entries()? {
            let mut entry = entry?;
            if entry.header().entry_type() != tar::EntryType::Regular {
                continue;
            }
            let name = entry
                .path()?
                .to_string_lossy()
                .trim_start_matches("./")
                .to_string();
            let mut data = Vec::new();
            entry.read_to_end(&mut data)?;
            files.push((name, data));
        }
        Ok(GuestOutput { files })
    }

    pub fn get(&self, name: &str) -> Option<&[u8]> {
        self.files
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, d)| d.as_slice())
    }

    pub fn text(&self, name: &str) -> Option<String> {
        self.get(name)
            .map(|d| String::from_utf8_lossy(d).into_owned())
    }

    pub fn code(&self, stage: &str) -> Option<i32> {
        self.text(&format!("{stage}.code"))?.trim().parse().ok()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_patterns() {
        assert!(matches("target", Path::new("target/debug/x")));
        assert!(matches(".git", Path::new(".git/HEAD")));
        assert!(matches("*.log", Path::new("a.log")));
        assert!(!matches("*.log", Path::new("dir/a.txt")));
        assert!(matches(
            "target/*/incremental",
            Path::new("target/debug/incremental/a")
        ));
        assert!(!matches("target", Path::new("src/target.rs")));
    }

    #[test]
    fn include_overrides_exclude() {
        let dir = std::env::temp_dir().join(format!("dp-work-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("target/release")).unwrap();
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(dir.join("target/release/tool"), "x").unwrap();
        fs::write(dir.join("target/release/other"), "x").unwrap();
        fs::write(dir.join("src/main.rs"), "x").unwrap();
        let files = work_files(&dir, &["target".into()], &["target/release/tool".into()]).unwrap();
        fs::remove_dir_all(&dir).unwrap();
        assert_eq!(
            files,
            vec![
                PathBuf::from("src/main.rs"),
                PathBuf::from("target/release/tool")
            ]
        );
    }

    #[test]
    fn quoting() {
        assert_eq!(shell_quote("abc"), "abc");
        assert_eq!(shell_quote("a b"), "'a b'");
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
        assert_eq!(shell_quote(""), "''");
    }
}
