#!/bin/sh
# Lance envprobe dans un rootfs de distro extrait (Alpine minirootfs, image
# LXC…), isolé par des namespaces user, mount et pid : ni root ni Docker requis.
# Le noyau reste celui de l'hôte, seul l'userspace change.
#
#   scripts/probe-rootfs.sh <rootfs> <binaire envprobe statique> [args…]
set -eu

rootfs=$(realpath "$1")
bin=$2
shift 2

install -m 755 "$bin" "$rootfs/envprobe"
trap 'rm -f "$rootfs/envprobe"' EXIT

unshare --user --map-root-user --mount --pid --fork sh -c '
    rootfs=$1; shift
    mount -t proc proc "$rootfs/proc"
    mount --rbind /sys "$rootfs/sys"
    exec chroot "$rootfs" /envprobe "$@"
' sh "$rootfs" "$@"
