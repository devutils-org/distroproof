# distromatrix

POC Rust : décrire précisément un environnement Linux (userspace + kernel), puis
lancer un programme dans des VM de distros cibles et comparer les résultats.

| Crate | Rôle |
| --- | --- |
| `envprobe-core` | bibliothèque des sondes |
| `envprobe` | CLI de la sonde |
| `distromatrix` | orchestrateur de VM (à venir) |

## Build

```sh
rustup target add x86_64-unknown-linux-musl
cargo build --release --target x86_64-unknown-linux-musl
```

Le binaire est statique et tourne tel quel dans n'importe quelle VM x86_64.
L'édition de liens passe par `rust-lld` (voir `.cargo/config.toml`), ce qui
permet aussi la compilation croisée depuis arm64.

Feuille de route : [docs/ROADMAP.md](docs/ROADMAP.md).
