#!/bin/sh
# envprobe self-test run by distroproof in each VM (dogfooding): the probe
# must run without crashing, produce a complete report and compare with
# itself without any difference.
set -eu
envprobe --json >/tmp/envprobe.json
sections=$(grep -c '"name":' /tmp/envprobe.json)
if [ "$sections" -lt 18 ]; then
    echo "incomplete report: $sections of 18 sections" >&2
    exit 1
fi
envprobe diff --color never /tmp/envprobe.json /tmp/envprobe.json >/dev/null
envprobe schema >/dev/null
envprobe --section kernel,libc,btf,sysctl,security
