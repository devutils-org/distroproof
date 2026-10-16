#!/bin/sh
# Programme exigeant le BTF du noyau, comme un outil BPF CO-RE (libbpf) :
# il échoue sur un noyau compilé sans CONFIG_DEBUG_INFO_BTF.
if [ ! -r /sys/kernel/btf/vmlinux ]; then
    echo "libbpf: failed to find valid kernel BTF" >&2
    echo "erreur : BTF du noyau introuvable (/sys/kernel/btf/vmlinux)" >&2
    exit 1
fi
echo "BTF présent : $(wc -c </sys/kernel/btf/vmlinux) octets"
