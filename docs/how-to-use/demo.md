# Démo de distroproof

Six actes, du plus simple au plus complet. Les cinq premiers sont scriptés
par [`scripts/demo.sh`](../../scripts/demo.sh) ; le sixième, la TUI, se joue
à la main en suivant le déroulé ci-dessous. Tout tourne sur de vraies VM.

## Préparation

```sh
# Arch : sudo pacman -S qemu-base edk2-ovmf ; Debian/Ubuntu : qemu-system-x86 qemu-utils ovmf
rustup target add x86_64-unknown-linux-musl
cargo build --release --target x86_64-unknown-linux-musl
export PATH="$PWD/target/x86_64-unknown-linux-musl/release:$PATH"
distroproof images pull debian-12 ubuntu-18.04 ubuntu-22.04 rocky-8 rocky-9 alpine
```

Le premier `pull` télécharge environ 3 Gio. Lancez une fois chaque cible
avant la démo : le premier boot extrait noyau et initramfs, les suivants
bootent en direct en quelques secondes.

```sh
scripts/demo.sh --list   # les actes
scripts/demo.sh          # actes 1 à 5 (~1 min avec le cache chaud)
scripts/demo.sh 5        # un seul acte
```

## Actes scriptés

1. **Le catalogue et le cache** : `distroproof images` liste les cibles, leur
   source et ce qui est déjà en cache (image vérifiée, noyau extrait).
2. **Le vrai noyau de chaque distro** : `uname -r` sur l'hôte, puis dans
   Rocky 8 (4.18), Rocky 9 (5.14) et Alpine (`virt`). Ce ne sont pas des
   conteneurs : chaque VM tourne sur le noyau de sa distro.
3. **Deux distros, un diff** : rapport `envprobe` dans Debian 12 et dans
   Alpine, puis `envprobe diff` : libc (glibc → musl), `/bin/sh`, gestionnaire
   de paquets, et cette fois le noyau aussi.
4. **Une matrice déclarative** : `distroproof check` valide
   `distroproof.toml` ; un fichier qui vise une image inexistante est refusé
   avec ligne et colonne, sans lancer de VM ; puis `distroproof up` lance la
   matrice de dogfooding avec ses variantes (paranoid, lockdown, PMU masquée).
5. **Pourquoi ça casse** : [`examples/require-btf`](../../examples/require-btf)
   exige `/sys/kernel/btf/vmlinux`. Il casse sur Ubuntu 18.04 et distroproof
   désigne le BTF comme cause probable, puis écrit le résumé Markdown destiné
   à la PR.

## Acte 6 — La TUI

```sh
distroproof up --hold
```

`--hold` garde chaque VM allumée après la commande, avec un shell root sur la
console série : c'est ce qui rend la console interactive utilisable une fois
la matrice terminée. Sans `--hold`, la console montre le boot puis s'arrête
avec la VM.

Déroulé conseillé :

| Étape | Touches | Ce qu'on montre |
| --- | --- | --- |
| Vue d'ensemble | — | une carte par cible : état, temps de boot, jauges CPU et mémoire, graphe Braille ; l'interface reste fluide pendant les boots |
| Navigation | `hjkl`, `f` | sélection d'une cible ; `f` filtre : toutes, en cours, échecs, terminées |
| Logs | `L`, `/` `noyau`, `n` `N` | logs série en direct de la cible, recherche et saut entre occurrences |
| Pause, reprise | `espace` | `stop` / `cont` via QMP : la jauge CPU retombe à zéro puis repart |
| Kill, relance | `x`, `r` | la carte passe en échec, puis la VM redémarre sur un overlay neuf |
| Rapport | `R`, `Entrée`, `h` `l` | rapport `envprobe` replié par section, déplier et replier |
| Diff | `d`, `d`, `D` | diff contre la cible suivante qui a un rapport ; `D` revient au rapport seul |
| Console | `c`, `i`, `Ctrl+]` | console série dans un panneau ; `i` pour saisir (`uname -a`, `dmesg \| tail`), `Ctrl+]` pour sortir |
| Tiling | `Ctrl+W v`, `Ctrl+W s`, `Ctrl+W hjkl`, `Ctrl+W <>+-`, `Ctrl+W q` | découper, changer de panneau, redimensionner, fermer |
| Épingler | `p` | fixe le panneau sur la cible sélectionnée, pour suivre deux cibles côte à côte |
| Palette | `:` ou `Ctrl+P` | toutes les actions par leur nom |
| Aide, sortie | `?`, `q` | `q` deux fois si des VM tournent encore : tout est arrêté et nettoyé |

Plusieurs matrices côte à côte, un workspace par fichier (`1`, `2` pour
passer de l'une à l'autre) :

```sh
distroproof up -f distroproof.toml -f examples/require-btf/distroproof.toml
```

Pour vérifier le repli : `distroproof up | cat` donne la sortie texte
habituelle, sans TUI, comme en CI.
