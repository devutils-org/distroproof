#!/bin/sh
# distroproof guest agent, run as root by cloud-init (runcmd) from the payload
# disk extracted into /distroproof.
#
# Channel to the host, one line per message: /dev/ttyS1 (COM2) on x86_64,
# the virtio-serial port `org.distroproof.agent` on aarch64, whose `virt`
# machine has one UART.
#   @ READY <uptime>          agent started
#   @ STAGE <stage>           stage start (setup, probe, command, kernel)
#   @ EXIT <stage> <code>     stage end
#   @ WARN <message>          non-blocking problem (sysctl refused…)
#   @ DONE                    output disk written, shutting down soon
#   > <line>                  stdout of the current stage
#   ! <line>                  stderr of the current stage
#
# Full outputs, exit codes and the envprobe report are written to a tar
# archive on the output disk (serial dp-output).
set -u
D=/distroproof
OUT=$D/out
CH=/dev/ttyS1
CONSOLE=/dev/ttyS0
PORT=/dev/virtio-ports/org.distroproof.agent
if [ "$(uname -m)" = aarch64 ]; then
    CONSOLE=/dev/ttyAMA0
    # udev names the port a moment after the device appears.
    i=0
    while [ ! -e "$PORT" ] && [ $i -lt 100 ]; do sleep 0.1; i=$((i + 1)); done
fi
[ -e "$PORT" ] && CH=$PORT
# NixOS keeps its tools in the system profile, none in the FHS paths, and
# cloud-init's own PATH is the one that found them: both kept behind.
PATH=$D/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin:/run/wrappers/bin:/run/current-system/sw/bin:${PATH:-}
export PATH

stty -F $CH raw -echo 2>/dev/null || stty raw -echo <$CH 2>/dev/null
exec 3>$CH
say() { printf '@ %s\n' "$*" >&3; }
warn() { say WARN "$*"; echo "$*" >>"$OUT/warnings"; }

mkdir -p "$OUT"
say READY "$(cut -d' ' -f1 /proc/uptime)"

# Job parameters: DP_USER, DP_TARGET, DP_IMAGE, DP_CAPS, DP_PROBE,
# DP_EXTRACT_KERNEL, DP_SETUP, DP_COMMAND (1 if job/setup.sh, job/command.sh
# exist). DP_TARGET and DP_IMAGE are exported to setup and the command.
. $D/job/env
export DP_TARGET DP_IMAGE

disk() {
    for d in /sys/block/*; do
        if [ "$(cat "$d/serial" 2>/dev/null)" = "$1" ]; then
            echo "/dev/${d##*/}"
            return 0
        fi
    done
    return 1
}

relay() {
    while IFS= read -r line || [ -n "$line" ]; do
        printf '%s %s\n' "$1" "$line" >&3
    done
}

# run <stage> <shell command>: outputs copied to $OUT/<stage>.{out,err}
# and relayed line by line on the channel.
run() {
    stage=$1
    shift
    say STAGE "$stage"
    rm -f $D/fifo.out $D/fifo.err
    mkfifo $D/fifo.out $D/fifo.err
    tee "$OUT/$stage.out" <$D/fifo.out | relay '>' &
    p1=$!
    tee "$OUT/$stage.err" <$D/fifo.err | relay '!' &
    p2=$!
    sh -c "$1" >$D/fifo.out 2>$D/fifo.err </dev/null
    rc=$?
    wait $p1 $p2
    echo $rc >"$OUT/$stage.code"
    say EXIT "$stage" $rc
    return $rc
}

# Test user. as_user <script> gives the command that runs the script under
# that user's identity, with the requested capabilities.
as_user() { echo "sh $1"; }
if [ "$DP_USER" != root ]; then
    if ! id "$DP_USER" >/dev/null 2>&1; then
        useradd -m -s /bin/sh "$DP_USER" 2>/dev/null ||
            adduser -D -s /bin/sh "$DP_USER" 2>/dev/null ||
            warn "cannot create user $DP_USER"
    fi
    home=$(getent passwd "$DP_USER" 2>/dev/null | cut -d: -f6)
    [ -n "$home" ] || home=/home/$DP_USER
    envs="HOME=$home USER=$DP_USER LOGNAME=$DP_USER PATH=$PATH"
    # util-linux setpriv only: busybox's ignores --reuid.
    full_setpriv() { setpriv --help 2>&1 | grep -q -- --reuid; }
    if [ -n "$DP_CAPS" ] && ! full_setpriv; then
        command -v apk >/dev/null 2>&1 && apk add -q setpriv >/dev/null 2>&1
    fi
    if full_setpriv; then
        caps=""
        if [ -n "$DP_CAPS" ]; then
            list=$(echo "$DP_CAPS" | sed 's/^/+/; s/,/,+/g')
            caps="--inh-caps=$list --ambient-caps=$list"
        fi
        prefix="setpriv --reuid=$DP_USER --regid=$(id -g "$DP_USER") --init-groups $caps env $envs"
        as_user() { echo "$prefix sh $1"; }
    else
        [ -n "$DP_CAPS" ] && warn "setpriv missing: capabilities $DP_CAPS not granted"
        as_user() { echo "su -s /bin/sh $DP_USER -c 'export $envs; sh $1'"; }
    fi
    chown -R "$DP_USER" $D/work 2>/dev/null
fi

# Runtime kernel variants.
if [ -f $D/job/sysctl ]; then
    while IFS='=' read -r key value; do
        [ -n "$key" ] || continue
        if ! printf '%s\n' "$value" >"/proc/sys/$(echo "$key" | tr . /)" 2>/dev/null; then
            warn "sysctl $key=$value refused"
        fi
    done <$D/job/sysctl
fi

cd $D/work || cd /
if [ -f $D/job/userenv ]; then
    set -a
    . $D/job/userenv
    set +a
fi

[ -n "$DP_SETUP" ] && run setup "sh $D/job/setup.sh"

# File capabilities, after setup installed the files: what a program's install
# grants, where `caps` would raise ambient ones every child inherits.
if [ -f $D/job/filecaps ]; then
    if ! command -v setcap >/dev/null 2>&1; then
        if command -v apk >/dev/null 2>&1; then apk add -q libcap-utils >/dev/null 2>&1
        elif command -v apt-get >/dev/null 2>&1; then apt-get install -qq -y libcap2-bin >/dev/null 2>&1
        elif command -v dnf >/dev/null 2>&1; then dnf install -q -y libcap >/dev/null 2>&1
        elif command -v pacman >/dev/null 2>&1; then pacman -Sq --noconfirm libcap >/dev/null 2>&1
        fi
    fi
    last=$(cat /proc/sys/kernel/cap_last_cap 2>/dev/null || echo 0)
    while IFS="$(printf '\t')" read -r path caps; do
        [ -n "$path" ] || continue
        if ! command -v setcap >/dev/null 2>&1; then
            warn "file_caps: no setcap in this image, $path keeps no capability"
        elif ! setcap "$caps" "$path" 2>"$OUT/filecaps.err"; then
            warn "file_caps: setcap $caps $path refused (cap_last_cap $last): $(head -c 200 "$OUT/filecaps.err")"
        fi
    done <$D/job/filecaps
fi

if [ "$DP_PROBE" = 1 ]; then
    say STAGE probe
    sh -c "$(as_user $D/job/probe.sh)" >"$OUT/envprobe.json" 2>"$OUT/envprobe.err" </dev/null
    say EXIT probe $?
fi

[ -n "$DP_COMMAND" ] && run command "$(as_user $D/job/command.sh)"

# Files the job asked back, kept with their paths under out/collect.
if [ -f $D/job/collect ]; then
    mkdir -p "$OUT/collect"
    while IFS= read -r pattern; do
        [ -n "$pattern" ] || continue
        found=""
        for f in $pattern; do
            [ -e "$f" ] || continue
            found=1
            tar -cf - "$f" 2>/dev/null | tar -xf - -C "$OUT/collect" 2>/dev/null ||
                warn "collect: cannot copy $f"
        done
        [ -n "$found" ] || warn "collect: nothing matches $pattern"
    done <$D/job/collect
fi

# Kernel and initramfs for the following direct boots.
if [ "$DP_EXTRACT_KERNEL" = 1 ]; then
    say STAGE kernel
    r=$(uname -r)
    k=""
    # /boot/kernel-$r is Gentoo's installkernel; its modules/$r/vmlinuz is a
    # link into a kernel source tree the image does not ship. NixOS keeps
    # the running system's kernel and initrd in its profile, none in /boot,
    # and its command line's init= is what boots that system again.
    for f in /boot/vmlinuz-$r /boot/kernel-$r /lib/modules/$r/vmlinuz /usr/lib/modules/$r/vmlinuz \
        /boot/vmlinuz-virt /boot/vmlinuz-lts /boot/vmlinuz-linux /boot/vmlinuz \
        /run/current-system/kernel; do
        [ -f "$f" ] && { k=$f; break; }
    done
    i=""
    for f in /boot/initrd.img-$r /boot/initramfs-$r.img /boot/initramfs-virt \
        /boot/initramfs-lts /boot/initramfs-linux.img /boot/initrd.img /boot/initramfs.img \
        /run/current-system/initrd; do
        [ -f "$f" ] && { i=$f; break; }
    done
    if [ -n "$k" ]; then
        mkdir -p "$OUT/kernel"
        cp "$k" "$OUT/kernel/vmlinuz"
        [ -n "$i" ] && cp "$i" "$OUT/kernel/initrd"
        cat /proc/cmdline >"$OUT/kernel/cmdline"
        echo "$r" >"$OUT/kernel/release"
        say EXIT kernel 0
    else
        warn "kernel not found in /boot for $r"
        say EXIT kernel 1
    fi
fi

if out=$(disk dp-output); then
    tar -cf "$out" -C "$OUT" . 2>/dev/null || warn "cannot write the output disk"
else
    warn "output disk not found"
fi
sync
say DONE

if [ "${DP_HOLD:-0}" = 1 ]; then
    # VM kept up for debugging: root shell on the serial console, instead
    # of the getty.
    tty=${CONSOLE#/dev/}
    systemctl stop "serial-getty@$tty.service" 2>/dev/null
    if grep -q "$tty" /etc/inittab 2>/dev/null; then
        sed -i "/$tty/d" /etc/inittab && kill -HUP 1
        pkill -f "getty.*$tty" 2>/dev/null
    fi
    stty -F "$CONSOLE" sane 2>/dev/null
    printf '\r\ndistroproof: command finished, root shell on the console.\r\n' >"$CONSOLE"
    while :; do
        setsid sh -c "exec sh -i <$CONSOLE >$CONSOLE 2>&1" || sleep 1
    done
fi
poweroff -f 2>/dev/null || echo o >/proc/sysrq-trigger
