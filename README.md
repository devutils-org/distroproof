# distroproof

POC Rust : décrire précisément un environnement Linux (userspace + kernel), puis
lancer un programme dans des VM de distros cibles et comparer les résultats.

| Crate | Rôle |
| --- | --- |
| `envprobe-core` | bibliothèque des sondes |
| `envprobe` | CLI de la sonde |
| `distroproof` | orchestrateur de VM (à venir) |

## Build

```sh
rustup target add x86_64-unknown-linux-musl
cargo build --release --target x86_64-unknown-linux-musl
```

Le binaire est statique et tourne tel quel dans n'importe quelle VM x86_64.
L'édition de liens passe par `rust-lld` (voir `.cargo/config.toml`), ce qui
permet aussi la compilation croisée depuis arm64.

## Utilisation

```sh
envprobe                         # résumé lisible
envprobe --json                  # rapport complet (schéma : schema/envprobe-report.schema.json)
envprobe --json --section kernel,bpf
envprobe diff a.json b.json      # code de sortie 1 s'il y a des différences
envprobe schema                  # schéma JSON du rapport
```

Chaque valeur porte un statut : `ok`, `unavailable` (absent, avec la raison)
ou `denied` (EPERM, EACCES…). Les entrées `privileged` changent selon les
droits du processus.

Pour sonder l'userspace d'une autre distro sans VM ni Docker (même noyau) :
`scripts/probe-rootfs.sh <rootfs extrait> <envprobe statique> --json`.

Feuille de route : [docs/ROADMAP.md](docs/ROADMAP.md).

## Licence

Au choix, sous [MIT](LICENSE-MIT) ou [Apache 2.0](LICENSE-APACHE).
