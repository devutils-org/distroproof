#!/bin/sh
# Program that requires kernel BTF, like a BPF CO-RE tool (libbpf): it fails
# on a kernel built without CONFIG_DEBUG_INFO_BTF.
if [ ! -r /sys/kernel/btf/vmlinux ]; then
    echo "libbpf: failed to find valid kernel BTF" >&2
    echo "error: kernel BTF not found (/sys/kernel/btf/vmlinux)" >&2
    exit 1
fi
echo "BTF present: $(wc -c </sys/kernel/btf/vmlinux) bytes"
