#!/bin/sh
# Autotest d'envprobe lancé par distroproof dans chaque VM (dogfooding) :
# la sonde doit tourner sans planter, produire un rapport complet et se
# comparer à elle-même sans différence.
set -eu
envprobe --json >/tmp/envprobe.json
sections=$(grep -c '"name":' /tmp/envprobe.json)
if [ "$sections" -lt 18 ]; then
    echo "rapport incomplet : $sections sections sur 18" >&2
    exit 1
fi
envprobe diff --color never /tmp/envprobe.json /tmp/envprobe.json >/dev/null
envprobe schema >/dev/null
envprobe --section kernel,libc,btf,sysctl,security
