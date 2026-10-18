#!/bin/sh
# distroproof guest agent, run as root by cloud-init (runcmd) from the payload
# disk extracted into /distroproof.
#
# Channel to the host: /dev/ttyS1, one line per message.
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
PATH=$D/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
export PATH

stty -F $CH raw -echo 2>/dev/null || stty raw -echo <$CH 2>/dev/null
exec 3>$CH
say() { printf '@ %s\n' "$*" >&3; }
warn() { say WARN "$*"; echo "$*" >>"$OUT/warnings"; }

mkdir -p "$OUT"
say READY "$(cut -d' ' -f1 /proc/uptime)"

# Job parameters: DP_USER, DP_CAPS, DP_PROBE, DP_EXTRACT_KERNEL,
# DP_SETUP, DP_COMMAND (1 if job/setup.sh, job/command.sh exist).
. $D/job/env

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

if [ "$DP_PROBE" = 1 ]; then
    say STAGE probe
    sh -c "$(as_user $D/job/probe.sh)" >"$OUT/envprobe.json" 2>"$OUT/envprobe.err" </dev/null
    say EXIT probe $?
fi

[ -n "$DP_COMMAND" ] && run command "$(as_user $D/job/command.sh)"

# Kernel and initramfs for the following direct boots.
if [ "$DP_EXTRACT_KERNEL" = 1 ]; then
    say STAGE kernel
    r=$(uname -r)
    k=""
    for f in /boot/vmlinuz-$r /lib/modules/$r/vmlinuz /usr/lib/modules/$r/vmlinuz \
        /boot/vmlinuz-virt /boot/vmlinuz-lts /boot/vmlinuz-linux /boot/vmlinuz; do
        [ -f "$f" ] && { k=$f; break; }
    done
    i=""
    for f in /boot/initrd.img-$r /boot/initramfs-$r.img /boot/initramfs-virt \
        /boot/initramfs-lts /boot/initramfs-linux.img /boot/initrd.img /boot/initramfs.img; do
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
    systemctl stop serial-getty@ttyS0.service 2>/dev/null
    if grep -q ttyS0 /etc/inittab 2>/dev/null; then
        sed -i '/ttyS0/d' /etc/inittab && kill -HUP 1
        pkill -f 'getty.*ttyS0' 2>/dev/null
    fi
    stty -F /dev/ttyS0 sane 2>/dev/null
    printf '\r\ndistroproof: command finished, root shell on the console.\r\n' >/dev/ttyS0
    while :; do
        setsid sh -c 'exec sh -i </dev/ttyS0 >/dev/ttyS0 2>&1' || sleep 1
    done
fi
poweroff -f 2>/dev/null || echo o >/proc/sysrq-trigger
