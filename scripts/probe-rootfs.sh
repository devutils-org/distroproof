#!/bin/sh
# Runs envprobe in an extracted distro rootfs (Alpine minirootfs, LXC
# image…), isolated by user, mount and pid namespaces: neither root nor Docker
# needed. The kernel stays the host's; only the userspace changes.
#
#   scripts/probe-rootfs.sh <rootfs> <static envprobe binary> [args…]
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
