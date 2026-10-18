#!/usr/bin/env bash
# distroproof demo: each block runs the real binary on real VMs and shows the
# command, then its output. Nothing is simulated.
#
#   scripts/demo.sh           the five non-interactive acts (~1 min, warm cache)
#   scripts/demo.sh 3         a single act
#   scripts/demo.sh --list    the acts, without running anything
#
# Act 6 (the TUI) is interactive: see docs/how-to-use/demo.md.
# Requirements: qemu-system-x86_64, qemu-img, curl, /dev/kvm, OVMF for
# rocky-9. The first run downloads the images (~3 GiB).
# Nothing is written outside a temporary directory and the image cache.
set -u

ACTS=(
    "1|The catalog and the cache"
    "2|Each distro's real kernel"
    "3|Two distros, one diff"
    "4|A declarative matrix"
    "5|Why it breaks"
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
    echo "binaries missing: cargo build --release --target x86_64-unknown-linux-musl" >&2
    exit 2
fi
if ! command -v "${DISTROPROOF_QEMU:-qemu-system-x86_64}" >/dev/null; then
    echo "QEMU not found: install qemu (Arch: qemu-base edk2-ovmf)" >&2
    exit 2
fi

SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT
cd "$ROOT"

bold=''; dim=''; reset=''
if [ -t 1 ]; then bold=$'\033[1m'; dim=$'\033[2m'; reset=$'\033[0m'; fi

act() {
    printf '\n%sAct %s — %s%s\n' "$bold" "$1" "$2" "$reset"
    printf '%s%s%s\n' "$dim" "$(printf '─%.0s' $(seq 1 $((${#2} + 8))))" "$reset"
}

# Shows the command as one would type it, then its indented output.
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
    act 1 "The catalog and the cache"
    run "$BIN" images
fi

if wanted 2; then
    act 2 "Each distro's real kernel"
    run uname -r
    for t in rocky-8 rocky-9 alpine; do
        run "$BIN" run --target "$t" --progress none -- uname -r
    done
    run "$BIN" run --target rocky-8 -- sh -c 'cat /etc/redhat-release; ldd --version | head -1'
fi

if wanted 3; then
    act 3 "Two distros, one diff"
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
    act 4 "A declarative matrix"
    run "$BIN" check
    printf '[[target]]\nname = "rhel7"\nimage = "rocky-7"\n' >"$SCRATCH/bogus.toml"
    run "$BIN" up -f "$SCRATCH/bogus.toml"
    run "$BIN" up --no-tui --progress none --jobs 5
fi

if wanted 5; then
    act 5 "Why it breaks"
    run cat examples/require-btf/require-btf.sh
    run "$BIN" up -f examples/require-btf/distroproof.toml --no-tui --progress none \
        --markdown "$SCRATCH/summary.md"
    printf '\n%s$ head -8 summary.md%s\n' "$dim" "$reset"
    head -8 "$SCRATCH/summary.md" | sed 's/^/    /'
fi
