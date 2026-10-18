# Roadmap — distroproof (POC Rust)

Oct 9, 2026 · @Mohamed Bekheira

## Vision et conventions

Objectif du POC : un outil Rust qui décrit précisément un environnement Linux (userspace + kernel), puis lance des VM « distro cible » pour y exécuter un programme et comparer les résultats. Nom : `distroproof`. On commence par la sonde (`envprobe`), seule et utile dès la phase 1.

Chaque phase est autonome : un agent la réalise sans lire les suivantes. Une phase n'est terminée que lorsque tous ses critères de sortie sont cochés.

Règles pour l'agent, valables à toutes les phases :

- Rust stable, édition 2021, workspace Cargo. `cargo fmt`, `cargo clippy -- -D warnings` et `cargo test` passent avant chaque commit.
- La sonde ne plante jamais : chaque information absente ou inaccessible devient une valeur `Unknown` / `Unavailable` avec la raison (permission, fichier absent, syscall refusé).
- Sonder les fonctionnalités, ne jamais déduire depuis un numéro de version (backports RHEL).
- La sonde n'a aucun effet de bord : lecture seule, pas d'écriture de sysctl, pas de chargement durable de programme BPF.
- Binaire de la sonde compilé en statique (`x86_64-unknown-linux-musl`) pour tourner tel quel dans n'importe quelle VM cible.
- Dépendances minimales et justifiées dans le message de commit. Pas de `unsafe` hors d'un module `sys` dédié et commenté.
- Ne pas présumer root : chaque sonde indique si le résultat dépend des privilèges.

## Phase 0 — Setup du projet

But : un squelette qui compile en statique et tourne en CI, avant toute fonctionnalité.

Tâches :

1. Créer un workspace Cargo avec trois crates : `envprobe-core` (lib, les sondes), `envprobe` (binaire CLI) et `distroproof` (binaire orchestrateur, vide pour l'instant).
2. Ajouter la cible musl et un profil release optimisé taille (`lto`, `strip`, `opt-level = "s"`).
3. CLI avec `clap` : `envprobe` sans argument affiche un résumé lisible ; `--json` est réservé pour la phase 2.
4. CI GitHub Actions : fmt, clippy, tests, build musl, et vérification que le binaire est bien statique (`file` ou `ldd` → « not a dynamic executable »).
5. `README.md` court et ce fichier de roadmap dans `docs/ROADMAP.md`.

Critères de sortie :

- [x] `cargo build --release --target x86_64-unknown-linux-musl` produit un binaire statique.
- [x] La CI est verte sur une PR vide.
- [x] `envprobe --version` fonctionne.

## Phase 1 — Sonde d'environnement (envprobe)

But : `envprobe` affiche tout ce qui peut changer le comportement d'un programme userspace ou kernel-facing (perf, BPF) sur la machine courante. Un module Rust par catégorie, chacun exposant une fonction `probe() -> Section`.

Ordre conseillé : identité et kernel d'abord, puis userspace, puis BPF/perf (le plus délicat).

| Catégorie | Informations | Sources |
| --- | --- | --- |
| Identité | distro, version, famille (`ID_LIKE`), arch, virtualisé ou non, conteneur ou non | `/etc/os-release`, `uname()`, `/sys/class/dmi/id/*`, `/proc/1/cgroup`, `/.dockerenv`, `/run/systemd/container` |
| Kernel | version, release, cmdline, config (`CONFIG_BPF*`, `KPROBES`, `UPROBES`, `FTRACE`, `DEBUG_INFO_BTF`, `PERF_EVENTS`) | `uname()`, `/proc/cmdline`, `/proc/config.gz` puis `/boot/config-$(uname -r)` |
| Modules | modules chargés, chargement possible | `/proc/modules`, `/sys/module` |
| Sysctls | `perf_event_paranoid`, `perf_event_max_sample_rate`, `unprivileged_bpf_disabled`, `bpf_jit_enable`, `bpf_jit_harden`, `kptr_restrict`, `dmesg_restrict`, `yama.ptrace_scope` | `/proc/sys/kernel/*`, `/proc/sys/net/core/*` |
| Sécurité | LSM actifs, mode SELinux, profils AppArmor, lockdown, Secure Boot | `/sys/kernel/security/lsm`, `/sys/fs/selinux/enforce`, `/sys/kernel/security/apparmor`, `/sys/kernel/security/lockdown`, efivars |
| Privilèges | uid/euid, capabilities effectives (`CAP_BPF`, `CAP_PERFMON`, `CAP_SYS_ADMIN`), seccomp, `NoNewPrivs` | `/proc/self/status` (`CapEff`, `Seccomp`) |
| BTF | présence et taille de vmlinux BTF, BTF des modules | `/sys/kernel/btf/vmlinux`, `/sys/kernel/btf/*` |
| BPF | types de programmes, types de maps, helpers clés, JIT | appels `bpf(BPF_PROG_LOAD)` minimaux, équivalent de `bpftool feature probe` |
| perf | `perf_event_open` en mode logiciel et matériel, PMU disponibles | appel test sur `PERF_COUNT_SW_CPU_CLOCK` et `PERF_COUNT_HW_CPU_CYCLES`, `/sys/bus/event_source/devices` |
| Tracing | tracefs monté et où, debugfs, kallsyms lisible | `/proc/mounts`, `/sys/kernel/tracing`, `/sys/kernel/debug/tracing`, `/proc/kallsyms` |
| libc | glibc ou musl, version | `gnu_get_libc_version()` si dispo, sinon lecture de `/lib/ld-musl-*` ou de l'interpréteur ELF de `/bin/sh` |
| Libs | présence et version des libs courantes (libbpf, libelf, zlib, openssl, libz) | `ldconfig -p` si dispo, sinon scan de `/lib`, `/lib64`, `/usr/lib*` |
| FHS | merged-usr, `/lib64` existe, multiarch Debian, chemins des libs | liens symboliques de `/bin`, `/lib`, `/sbin` ; `/usr/lib/x86_64-linux-gnu` |
| Shell et outils | cible de `/bin/sh`, coreutils GNU ou busybox | `readlink /bin/sh`, `/bin/ls` vers busybox |
| Init et services | PID 1 (systemd, OpenRC, runit…) | `/proc/1/comm`, `/proc/1/exe` |
| Gestionnaire de paquets | apt, dnf, pacman, zypper, apk, xbps, nix | présence des binaires dans le `PATH` |
| cgroups | v1, v2 ou hybride | `/sys/fs/cgroup`, `/proc/mounts` |
| XDG | variables définies et valeurs effectives | variables d'environnement |

Les sondes BPF et perf doivent utiliser des syscalls directs (via le crate `libc` ou `nix`), sans dépendre de libbpf, pour garder un binaire statique simple. Un échec `EPERM` ou `EACCES` est un résultat, pas une erreur.

Critères de sortie :

- [x] Toutes les catégories du tableau sont implémentées.
- [x] Exécuté en utilisateur normal puis en root, la sortie diffère là où c'est attendu (privilèges, BPF, perf).
- [x] Exécuté dans un conteneur Docker par défaut, la sonde détecte le conteneur et le seccomp sans planter.
- [x] Tests unitaires sur les parseurs (`os-release`, `config.gz`, `/proc/self/status`) avec des fixtures de plusieurs distros.

## Phase 2 — Sortie structurée et diff

But : rendre le rapport exploitable par une machine et comparer deux environnements, base de toute la suite.

Tâches :

1. Modèle de données versionné (`schema_version`) avec `serde`. Chaque valeur porte son statut : `Ok(valeur)`, `Unavailable(raison)`, `Denied(errno)`.
2. `envprobe --json` écrit le rapport complet sur stdout ; `--section kernel,bpf` filtre.
3. Schéma JSON publié dans le dépôt, généré avec `schemars`.
4. Sous-commande `envprobe diff a.json b.json` : affiche les différences par section, en couleur, avec un code de sortie non nul s'il y en a.
5. Collecter des rapports réels comme fixtures : ta machine, un conteneur Debian, un conteneur Alpine, plus tout ce que tu as sous la main.

Critères de sortie :

- [x] Le JSON valide contre le schéma publié.
- [x] `diff` entre ta machine et un conteneur Alpine fait ressortir libc, `/bin/sh` et gestionnaire de paquets, mais pas le kernel (même noyau).
- [x] Tests de non-régression sur au moins trois fixtures.

## Phase 3 — VM avec noyau et rootfs de distro

But : `distroproof run --target debian-12 -- envprobe --json` démarre une VM avec le **vrai noyau de la distro** et son rootfs, y lance la commande et récupère la sortie. C'est la phase la plus risquée : commencer par une seule cible qui marche de bout en bout.

Tâches :

1. Choix techniques : QEMU + KVM piloté en sous-processus (pas de lib de virtualisation au début). Images cloud officielles des distros (qcow2) comme source, car elles contiennent noyau, initramfs et config d'origine.
2. Cache local dans `$XDG_CACHE_HOME/distroproof/images` : téléchargement, vérification du checksum publié, overlay qcow2 jetable par exécution.
3. Injection : partage du répertoire de travail via virtiofs ou 9p, ou à défaut une seconde image disque. Configuration initiale via cloud-init (seed ISO) pour créer l'utilisateur et lancer la commande.
4. Récupération de stdout, stderr et du code de retour via la console série (`-serial`) ou virtio-vsock, puis extinction propre de la VM.
5. Timeouts et nettoyage garantis, même si la VM ne boote pas.
6. Cibles dans l'ordre : `debian-12`, `ubuntu-22.04`, `rocky-9`, `rocky-8`, `fedora-latest`, `arch`, `alpine`.

Exigence d'architecture (prépare la phase 6) : l'orchestrateur n'écrit jamais directement dans le terminal. Il émet des événements typés, par exemple `VmEvent { Downloading, Booting, Running, Output(line), Metrics(..), Exited(code), TimedOut }`, dans un channel tokio. La sortie texte, la sortie JSON et plus tard la TUI ne sont que des consommateurs de ce flux. Le PID de chaque processus QEMU et un socket QMP par VM (`-qmp unix:...`) sont exposés dès cette phase.

Piste d'accélération à évaluer ensuite : extraire noyau et initramfs de l'image et booter en direct (`-kernel`, `-initrd`), à la manière de virtme-ng, pour passer sous les 5 secondes.

Critères de sortie :

- [x] Les 7 cibles bootent et renvoient un rapport `envprobe` JSON.
- [x] Le rapport de chaque VM montre bien le noyau de la distro (4.18 sur Rocky 8, 5.14 sur Rocky 9…) et non celui de l'hôte.
- [x] Une exécution répétée réutilise le cache et laisse le disque propre.
- [x] Temps de boot mesuré et affiché par cible.

## Phase 4 — Fichier déclaratif et matrice

But : décrire sa matrice de test dans un fichier versionné du projet testé, à la manière d'une infra as code. `distroproof up` lit le fichier et lance toutes les cibles en parallèle.

Exemple de format visé (`distroproof.toml`) :

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
sysctl = { "kernel.perf_event_paranoid" = "3" }

[[target]]
name = "fedora-lockdown"
image = "fedora-latest"
kernel_cmdline = "lockdown=confidentiality"

[run]
setup = "./scripts/install-deps.sh"
command = "./target/release/montool --selftest"
as_user = "test"   # ou "root"
```

Tâches :

1. Parser et valider le fichier (`serde` + `toml`), messages d'erreur avec ligne et colonne.
2. Variantes par cible : sysctls, cmdline noyau, utilisateur root ou non, capabilities accordées, PMU exposée ou non (`-cpu host` vs modèle générique).
3. Exécution parallèle bornée (`--jobs`), avec affichage de la progression par cible.
4. `envprobe` est lancé automatiquement avant la commande de l'utilisateur et son rapport joint au résultat.

Critères de sortie :

- [x] Une matrice de 5 cibles avec variantes tourne en une commande.
- [x] Le rapport `envprobe` de chaque cible reflète bien les variantes demandées (paranoid, lockdown).
- [x] Un fichier invalide donne une erreur claire sans lancer de VM.

## Phase 5 — Tests et rapport

But : savoir en un coup d'œil où le programme marche, où il casse, et pourquoi, en reliant chaque échec à une différence d'environnement.

Tâches :

1. Rapport de matrice : tableau cibles × résultat (succès, échec, timeout), durée, extrait de stderr.
2. Corrélation : pour chaque cible en échec, `envprobe diff` automatique contre la cible réussie la plus proche, pour pointer le suspect (BTF absent, paranoid à 3, musl…).
3. Formats de sortie : terminal, JSON, JUnit XML (pour la CI) et Markdown (pour un commentaire de PR).
4. Intégration CI : exemple de workflow GitHub Actions avec KVM activé et cache des images.
5. Dogfooding : faire tourner `envprobe` lui-même dans la matrice comme premier projet testé.

Critères de sortie :

- [x] Un échec volontaire (programme exigeant BTF) est détecté sur une cible sans BTF, et le rapport désigne BTF comme cause probable.
- [x] La matrice tourne en CI sur une PR et publie un résumé Markdown.

## Phase 6 — TUI de monitoring

But : une TUI moderne qui donne la vue d'ensemble de la matrice en temps réel, dans l'esprit de [btop](https://github.com/aristocratos/btop) pour les graphes et jauges, et de [TUIOS](https://github.com/Gaurav-Gosain/tuios) pour la gestion des panneaux (tiling, interface modale façon vim, palette de commandes). Elle se branche uniquement sur le bus d'événements de la phase 3, sans toucher à l'orchestrateur.

Stack : `ratatui` + `crossterm`, `tokio` pour la boucle d'événements, `tui-term` + `vt100` pour afficher la console série d'une VM dans un panneau avec ses séquences ANSI.

Vues :

- **Dashboard** : une carte par cible (style btop) avec état coloré, temps de boot, durée, jauges CPU et mémoire, sparkline de charge sur les dernières secondes.
- **Détail** : logs série en direct de la cible sélectionnée, avec défilement et recherche.
- **Rapport** : le rapport `envprobe` de la cible et son diff contre une autre cible, sections repliables.
- **Console** : console série interactive d'une VM dans un panneau, pour déboguer à la main.
- **Layout** : panneaux en tiling redimensionnables, plusieurs workspaces (une matrice par workspace, par exemple).

Métriques : CPU et mémoire lus depuis `/proc/<pid QEMU>/stat` et `/proc/<pid>/status` (rafraîchissement 1 s), état de la VM et stats mémoire invité via QMP (`query-status`, balloon), débit disque et réseau via `query-blockstats`.

Interactions :

- Navigation modale façon vim (`hjkl`, `/` pour chercher), palette de commandes sur `:` ou `Ctrl+P`, support souris.
- Actions sur une cible : relancer, tuer, mettre en pause et reprendre (QMP `stop` / `cont`), ouvrir la console, lancer un diff.
- Filtrer les cibles par état (échecs seulement, en cours…).

Rendu : thèmes configurables dans `$XDG_CONFIG_HOME/distroproof/theme.toml`, true color avec repli 256 couleurs, caractères Braille pour les graphes comme btop. Activée seulement si stdout est un TTY ou avec `--tui` ; sinon repli automatique sur la sortie texte, ce qui garde la CI intacte.

Critères de sortie :

- [x] Une matrice de 7 cibles s'affiche en temps réel, avec états, jauges et logs, sans scintillement ni blocage de l'interface pendant les boots.
- [x] Pause, reprise, relance et kill fonctionnent depuis la TUI.
- [x] La console série interactive d'une VM est utilisable dans un panneau.
- [x] Sans TTY, la même commande produit la sortie texte habituelle.
- [x] La TUI reste fluide sur un terminal de 80 × 24 et s'adapte au redimensionnement.

## Pistes futures

Hors POC, à considérer une fois la phase 5 validée :

- **Boot rapide** : boot direct noyau + initramfs et rootfs partagé en virtiofs, voire microVM (Firecracker, Cloud Hypervisor, libkrun) pour viser la seconde.
- **Noyaux à la carte** : en plus des noyaux de distro, des noyaux LTS upstream (5.10, 5.15, 6.1, 6.6, 6.12) ou un noyau compilé par l'utilisateur.
- **Arm64** : mêmes cibles via QEMU TCG ou un runner arm64.
- **Mode `envprobe` standalone** : distribuer la sonde seule, comme `doctor` intégrable dans n'importe quel outil perf/BPF.
- **Base de profils** : publier les rapports `envprobe` des distros courantes pour comparer sans même lancer de VM.
