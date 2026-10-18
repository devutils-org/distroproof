#!/usr/bin/env bash
# Démonstration de distroproof : chaque bloc lance le vrai binaire sur de
# vraies VM et montre la commande puis sa sortie. Rien n'est simulé.
#
#   scripts/demo.sh           les cinq actes non interactifs (~1 min, cache chaud)
#   scripts/demo.sh 3         un seul acte
#   scripts/demo.sh --list    les actes, sans rien lancer
#
# L'acte 6 (la TUI) est interactif : voir docs/how-to-use/demo.md.
# Prérequis : qemu-system-x86_64, qemu-img, curl, /dev/kvm, OVMF pour
# rocky-9. Le premier lancement télécharge les images (~3 Gio).
# Rien n'est écrit hors d'un répertoire temporaire et du cache d'images.
set -u

ACTS=(
    "1|Le catalogue et le cache"
    "2|Le vrai noyau de chaque distro"
    "3|Deux distros, un diff"
    "4|Une matrice déclarative"
    "5|Pourquoi ça casse"
)

case "${1:-}" in
    --list)
        for a in "${ACTS[@]}"; do printf '  %s. %s\n' "${a%%|*}" "${a#*|}"; done
        exit 0 ;;
    -h|--help)
        sed -n "2,12p" "$0" | sed "s/^# \{0,1\}//"
        exit 0 ;;
esac
ONLY="${1:-}"

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN="${BINARY:-$ROOT/target/x86_64-unknown-linux-musl/release/distroproof}"
ENVPROBE="$(dirname "$BIN")/envprobe"
if [ ! -x "$BIN" ] || [ ! -x "$ENVPROBE" ]; then
    echo "binaires absents : cargo build --release --target x86_64-unknown-linux-musl" >&2
    exit 2
fi
if ! command -v "${DISTROPROOF_QEMU:-qemu-system-x86_64}" >/dev/null; then
    echo "QEMU introuvable : installez qemu (Arch : qemu-base edk2-ovmf)" >&2
    exit 2
fi

SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT
cd "$ROOT"

bold=''; dim=''; reset=''
if [ -t 1 ]; then bold=$'\033[1m'; dim=$'\033[2m'; reset=$'\033[0m'; fi

act() {
    printf '\n%sActe %s — %s%s\n' "$bold" "$1" "$2" "$reset"
    printf '%s%s%s\n' "$dim" "$(printf '─%.0s' $(seq 1 $((${#2} + 9))))" "$reset"
}

# Montre la commande telle qu'on la taperait, puis sa sortie indentée.
run() {
    local shown=''
    for a in "$@"; do
        case "$a" in
            *[!A-Za-z0-9_./=:,@-]*) shown="$shown '$a'" ;;
            *) shown="$shown $a" ;;
        esac
    done
    printf '\n%s$%s%s\n' "$dim" "${shown/$BIN/distroproof}" "$reset"
    timeout 300 "$@" 2>&1 | sed 's/^/    /'
}

wanted() { [ -z "$ONLY" ] || [ "$ONLY" = "$1" ]; }

if wanted 1; then
    act 1 "Le catalogue et le cache"
    run "$BIN" images
fi

if wanted 2; then
    act 2 "Le vrai noyau de chaque distro"
    run uname -r
    for t in rocky-8 rocky-9 alpine; do
        run "$BIN" run --target "$t" --progress none -- uname -r
    done
    run "$BIN" run --target rocky-8 -- sh -c 'cat /etc/redhat-release; ldd --version | head -1'
fi

if wanted 3; then
    act 3 "Deux distros, un diff"
    "$BIN" run --target debian-12 --progress none -- envprobe --json >"$SCRATCH/debian.json"
    "$BIN" run --target alpine --progress none -- envprobe --json >"$SCRATCH/alpine.json"
    printf '\n%s$ distroproof run --target debian-12 -- envprobe --json > debian.json%s\n' "$dim" "$reset"
    printf '%s$ distroproof run --target alpine -- envprobe --json > alpine.json%s\n' "$dim" "$reset"
    printf '\n%s$ envprobe diff debian.json alpine.json --section libc,shell,packages,kernel%s\n' "$dim" "$reset"
    "$ENVPROBE" diff --color never "$SCRATCH/debian.json" "$SCRATCH/alpine.json" \
        | awk '/^\[/{show = ($0 ~ /^\[(libc|shell|packages|kernel)\]$/)} show' \
        | grep -vE '^[-+] (version|cmdline|config_source) ' | sed 's/^/    /'
fi

if wanted 4; then
    act 4 "Une matrice déclarative"
    run "$BIN" check
    printf '[[target]]\nname = "rhel7"\nimage = "rocky-7"\n' >"$SCRATCH/faux.toml"
    run "$BIN" up -f "$SCRATCH/faux.toml"
    run "$BIN" up --no-tui --progress none --jobs 5
fi

if wanted 5; then
    act 5 "Pourquoi ça casse"
    run cat examples/require-btf/require-btf.sh
    run "$BIN" up -f examples/require-btf/distroproof.toml --no-tui --progress none \
        --markdown "$SCRATCH/summary.md"
    printf '\n%s$ head -8 summary.md%s\n' "$dim" "$reset"
    head -8 "$SCRATCH/summary.md" | sed 's/^/    /'
fi
