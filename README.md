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

Toutes les phases du POC sont implémentées : la sonde `envprobe` (utilisable
seule) et l'orchestrateur `distroproof` (VM, matrice, rapport, TUI).

## Avancement

| Phase | Contenu | État |
| --- | --- | --- |
| 0 | Workspace, binaire statique musl, CI | ✅ |
| 1 | Sonde `envprobe` : 18 catégories (kernel, BPF, perf, LSM, libc…) | ✅ |
| 2 | Rapport JSON versionné, schéma publié, `envprobe diff` | ✅ |
| 3 | VM QEMU/KVM avec noyau et rootfs de distro | ✅ |
| 4 | Matrice déclarative `distroproof.toml` | ✅ |
| 5 | Rapport de matrice, corrélation des échecs, JUnit / Markdown | ✅ |
| 6 | TUI de monitoring | ✅ |

Détail et critères de sortie : [docs/ROADMAP.md](docs/ROADMAP.md). Démo de bout en bout
sur de vraies VM : [`scripts/demo.sh`](scripts/demo.sh) et
[docs/how-to-use/demo.md](docs/how-to-use/demo.md).

## Crates

| Crate | Rôle |
| --- | --- |
| `envprobe-core` | Bibliothèque des sondes, modèle du rapport, diff |
| `envprobe` | CLI de la sonde |
| `distroproof` | Orchestrateur de VM, matrice, rapport et TUI |

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

## distroproof

`distroproof` démarre de vraies VM QEMU/KVM sur les images cloud officielles
de chaque distro : c'est **le noyau de la distro** qui tourne (4.18 sur
Rocky 8, 5.14 sur Rocky 9…), pas celui de l'hôte.

```sh
distroproof run --target rocky-8 -- envprobe --json > rocky8.json
distroproof up                        # matrice de distroproof.toml
distroproof check                     # valide le fichier sans lancer de VM
distroproof images                    # catalogue et état du cache
distroproof report matrix.json --format markdown
```

Prérequis sur l'hôte : `qemu-system-x86_64`, `qemu-img`, `curl`, KVM
(`/dev/kvm`, repli automatique sur TCG sinon) et OVMF (`edk2-ovmf`, `ovmf`)
pour les images qui ne démarrent qu'en UEFI. Les binaires se choisissent avec `DISTROPROOF_QEMU`,
`DISTROPROOF_QEMU_IMG`, `DISTROPROOF_OVMF_CODE` et `DISTROPROOF_OVMF_VARS`.

### Cibles

| Image | Noyau observé | Remarque |
| --- | --- | --- |
| `debian-10`, `debian-11`, `debian-12`, `debian-13` | 4.19, 5.10, 6.1, 6.12 | 4.19 sans BTF ; 13 démarre en UEFI |
| `ubuntu-18.04`, `ubuntu-20.04`, `ubuntu-22.04`, `ubuntu-24.04` | 4.15, 5.4, 5.15, 6.8 | 4.15 sans BTF |
| `rocky-8`, `rocky-9` | 4.18, 5.14 | noyaux RHEL avec backports ; 9 démarre en UEFI |
| `fedora-latest` | 6.19 (Fedora 44) | dernière release, résolue en ligne |
| `arch` | rolling | |
| `alpine` | 6.18 `virt` | musl, busybox |

Les images sont téléchargées une fois, vérifiées contre le checksum publié
par la distro et gardées dans `$XDG_CACHE_HOME/distroproof/images`. Chaque
exécution travaille sur un overlay qcow2 jetable, supprimé à la fin, même
après un timeout ou un Ctrl+C ; QEMU meurt avec distroproof.

### Fonctionnement

- QEMU démarre en pause (`-S`), distroproof se connecte à son socket QMP et
  aux deux ports série (console sur ttyS0, canal de l'agent sur ttyS1), puis
  lance la VM.
- cloud-init (seed NoCloud écrit en Rust) extrait un disque payload (archive
  tar brute : agent, `envprobe` statique, répertoire de travail) et lance
  l'agent. Celui-ci applique les sysctls, crée l'utilisateur de test, lance
  `envprobe` puis la commande, et relaie stdout et stderr ligne à ligne.
- Les sorties complètes, les codes de retour et le rapport sont écrits sur un
  disque de sortie (tar brut), puis la VM s'éteint. Pas de 9p ni de virtiofs,
  absents des noyaux RHEL.
- Au premier boot, l'agent extrait aussi noyau et initramfs : les exécutions
  suivantes bootent en direct (`-kernel`), en 6 à 13 s, et peuvent modifier
  la ligne de commande du noyau.
- L'orchestrateur n'écrit jamais dans le terminal : il émet des événements
  typés (`Booting`, `Output`, `Metrics`, `Finished`…) que la sortie texte,
  la sortie JSON (`--progress json`) et la TUI consomment.

### Matrice : `distroproof.toml`

```toml
[defaults]
memory = "1G"
cpus = 2
timeout = "5m"

[[target]]
name = "rhel8"
image = "rocky-8"

[[target]]
name = "ubuntu-paranoid"
image = "ubuntu-22.04"
sysctl = { "kernel.perf_event_paranoid" = 3 }

[[target]]
name = "fedora-lockdown"
image = "fedora-latest"
kernel_cmdline = "lockdown=confidentiality"

[[target]]
name = "debian-nopmu"
image = "debian-12"
pmu = false                 # -cpu host,pmu=off
caps = ["perfmon", "bpf"]   # capabilities de l'utilisateur de test

[run]
setup = "./scripts/install-deps.sh"   # en root, avant la sonde
command = "./target/release/montool --selftest"
as_user = "test"                      # ou "root"
# exclude = [".git", "target"]        # copié dans /distroproof/work
# include = ["target/release/montool"]
```

Une erreur (clé inconnue, image inexistante, durée invalide…) est signalée
avec sa ligne et sa colonne, sans lancer de VM. `distroproof up` lance les
cibles en parallèle (`--jobs`), joint à chaque résultat le rapport `envprobe`
pris juste avant la commande, puis :

- affiche le tableau cibles × résultat, avec durée, temps de boot, noyau et
  extrait de stderr ;
- compare chaque cible en échec à la cible réussie la plus proche et désigne
  la différence suspecte (BTF absent, paranoid à 3, musl…) ;
- écrit si demandé `--json`, `--junit` (CI) et `--markdown` (commentaire de PR).

```text
[ubuntu-18.04] échec (code 1)
  comparée à ubuntu-22.04 : 65 différences significatives
  cause probable : btf.vmlinux : présent, 5286152 octets → indisponible (/sys/kernel/btf/vmlinux absent)
  aussi : kernel.CONFIG_DEBUG_INFO_BTF : y → absente de la config
```

Exemples : [`distroproof.toml`](distroproof.toml) (dogfooding d'envprobe sur
10 cibles, lancé en CI par [`matrix.yml`](.github/workflows/matrix.yml)) et
[`examples/require-btf`](examples/require-btf) (échec volontaire sans BTF).

### TUI

Dans un terminal, `distroproof up` ouvre une TUI (`--no-tui` pour la sortie
texte, automatique sans TTY) : une carte par cible avec état, temps de boot,
jauges CPU et mémoire et graphe Braille ; logs série en direct avec
recherche ; rapport `envprobe` replié par section et diff contre une autre
cible ; console série interactive. Avec `--hold`, chaque VM reste allumée
après la commande avec un shell root sur la console.

| Touche | Action |
| --- | --- |
| `hjkl`, flèches | sélection, défilement |
| `espace`, `r`, `x` | pause / reprise (QMP), relance, kill |
| `c`, `i`, `Ctrl+]` | ouvrir la console, saisir, sortir |
| `/`, `n`, `N` | recherche dans les logs |
| `p` | épingler le panneau sur la cible sélectionnée |
| `d`, `D` | diff du rapport contre une autre cible |
| `D L R C` | vue dashboard, logs, rapport, console du panneau |
| `Ctrl+W` `s` `v` `q` `hjkl` `<>+-` | tiling : découper, fermer, focus, taille |
| `1`…`9` | workspaces (un par fichier avec plusieurs `-f`) |
| `f` | filtre : toutes, en cours, échecs, terminées |
| `:` ou `Ctrl+P` | palette de commandes |
| `?`, `q` | aide, quitter |

Couleurs dans `$XDG_CONFIG_HOME/distroproof/theme.toml` (true color, repli
256 couleurs) :

```toml
[colors]
accent = "#7aa2f7"
success = "#9ece6a"
failure = "#f7768e"
```

## Build

```sh
rustup target add x86_64-unknown-linux-musl
cargo build --release --target x86_64-unknown-linux-musl
```

Les binaires sont statiques : `envprobe` tourne tel quel dans n'importe quelle
VM ou conteneur x86_64, et `distroproof` trouve l'`envprobe` posé à côté de lui. L'édition de liens passe par `rust-lld` (voir
`.cargo/config.toml`), ce qui permet aussi de compiler depuis arm64 vers x86_64.

Tests : `cargo test --workspace`. Pour régénérer le schéma après un changement
du modèle : `UPDATE_SCHEMA=1 cargo test -p envprobe-core --test reports`.

## Licence

Au choix, sous [MIT](LICENSE-MIT) ou [Apache 2.0](LICENSE-APACHE).
