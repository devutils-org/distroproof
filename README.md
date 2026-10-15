# distroproof

[![CI](https://github.com/moha-bekh/distroproof/actions/workflows/ci.yml/badge.svg)](https://github.com/moha-bekh/distroproof/actions/workflows/ci.yml)

> distroless retire la distro de ton image. distroproof vérifie que ton
> programme tient sur toutes les distros.

Un programme qui marche sur ta machine peut casser ailleurs : musl au lieu de
glibc, noyau RHEL 4.18, BTF absent, `perf_event_paranoid` à 3, lockdown actif…
distroproof décrit précisément un environnement Linux (userspace et noyau),
lance ton programme dans des VM qui tournent avec le **vrai noyau** de chaque
distro cible, puis relie chaque échec à la différence d'environnement qui
l'explique.

POC en cours : la sonde `envprobe` est utilisable seule, l'orchestrateur de VM
arrive en phase 3.

## Avancement

| Phase | Contenu | État |
| --- | --- | --- |
| 0 | Workspace, binaire statique musl, CI | ✅ |
| 1 | Sonde `envprobe` : 18 catégories (kernel, BPF, perf, LSM, libc…) | ✅ |
| 2 | Rapport JSON versionné, schéma publié, `envprobe diff` | ✅ |
| 3 | VM QEMU/KVM avec noyau et rootfs de distro | à venir |
| 4 | Matrice déclarative `distroproof.toml` | à venir |
| 5 | Rapport de matrice, corrélation des échecs, JUnit / Markdown | à venir |
| 6 | TUI de monitoring | à venir |

Détail et critères de sortie : [docs/ROADMAP.md](docs/ROADMAP.md).

## Crates

| Crate | Rôle |
| --- | --- |
| `envprobe-core` | Bibliothèque des sondes, modèle du rapport, diff |
| `envprobe` | CLI de la sonde |
| `distroproof` | Orchestrateur de VM (vide pour l'instant) |

## envprobe

`envprobe` liste tout ce qui peut changer le comportement d'un programme
userspace ou kernel-facing :

- **Identité** : distro, famille, arch, virtualisation, conteneur.
- **Noyau** : release, cmdline, config (`CONFIG_BPF*`, `KPROBES`, `DEBUG_INFO_BTF`…), modules.
- **Sécurité et privilèges** : sysctls, LSM, SELinux, AppArmor, lockdown, Secure Boot,
  capabilities, seccomp, user namespace.
- **BPF et perf** : types de programmes, types de maps et helpers, sondés par de
  vrais appels `bpf()` ; `perf_event_open` logiciel et matériel, PMU ; tracefs, kallsyms, BTF.
- **Userspace** : libc et sa version, bibliothèques courantes, FHS (merged-usr,
  multiarch), `/bin/sh`, coreutils, init, gestionnaire de paquets, cgroups, XDG.

Les fonctionnalités sont sondées directement, jamais déduites d'un numéro de
version, car les noyaux RHEL rétroportent beaucoup. La sonde est en lecture
seule et ne plante jamais : une information absente ou refusée apparaît avec
sa raison.

```sh
envprobe                              # résumé lisible
envprobe --json                       # rapport complet
envprobe --json --section kernel,bpf  # sections choisies
envprobe diff a.json b.json           # différences, code de sortie 1 s'il y en a
envprobe schema                       # schéma JSON du rapport
```

Extrait d'un diff entre un hôte Arch et un rootfs Alpine (même noyau) :

```text
[libc]
- interpreter  /lib/ld-linux-aarch64.so.1
+ interpreter  /lib/ld-musl-aarch64.so.1
- kind         glibc
+ kind         musl

[shell]
- sh         /usr/bin/bash
+ sh         /bin/busybox

[packages]
- managers  pacman
+ managers  apk
```

### Format du rapport

Chaque entrée porte un statut :

- `ok` : la valeur.
- `unavailable` : information absente, avec la raison (fichier absent, syscall inexistant…).
- `denied` : accès refusé (EPERM, EACCES, seccomp…).

Le champ `privileged` signale les résultats qui changent selon les droits du
processus. Le format est versionné (`schema_version`) et son schéma est publié
dans [`schema/envprobe-report.schema.json`](schema/envprobe-report.schema.json).

### Sonder une autre distro sans VM

Pour comparer l'userspace d'une autre distro sans VM ni Docker (le noyau reste
celui de l'hôte) :

```sh
scripts/probe-rootfs.sh <rootfs extrait> <envprobe statique> --json
```

Le script lance la sonde dans des namespaces user, mount et pid, sans root.

## Build

```sh
rustup target add x86_64-unknown-linux-musl
cargo build --release --target x86_64-unknown-linux-musl
```

Le binaire est statique : il tourne tel quel dans n'importe quelle VM ou
conteneur x86_64. L'édition de liens passe par `rust-lld` (voir
`.cargo/config.toml`), ce qui permet aussi de compiler depuis arm64 vers x86_64.

Tests : `cargo test --workspace`. Pour régénérer le schéma après un changement
du modèle : `UPDATE_SCHEMA=1 cargo test -p envprobe-core --test reports`.

## Licence

Au choix, sous [MIT](LICENSE-MIT) ou [Apache 2.0](LICENSE-APACHE).
