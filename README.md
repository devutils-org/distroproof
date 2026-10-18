# distroproof

[![CI](https://github.com/devutils-org/distroproof/actions/workflows/ci.yml/badge.svg)](https://github.com/devutils-org/distroproof/actions/workflows/ci.yml)

> distroless takes the distro out of your image. distroproof checks that your
> program holds up on every distro.

A program that works on your machine can break elsewhere: musl instead of
glibc, a RHEL 4.18 kernel, no BTF, `perf_event_paranoid` set to 3, lockdown
enabled… distroproof describes a Linux environment precisely (userspace and
kernel), runs your program in VMs booted on the **real kernel** of each target
distro, then ties every failure to the environment difference that explains
it.

Every phase of the POC is implemented: the `envprobe` probe (usable on its
own) and the `distroproof` orchestrator (VMs, matrix, report, TUI).

## Status

| Phase | Scope | Status |
| --- | --- | --- |
| 0 | Workspace, static musl binary, CI | ✅ |
| 1 | `envprobe` probe: 18 categories (kernel, BPF, perf, LSM, libc…) | ✅ |
| 2 | Versioned JSON report, published schema, `envprobe diff` | ✅ |
| 3 | QEMU/KVM VMs with the distro's kernel and rootfs | ✅ |
| 4 | Declarative matrix in `distroproof.toml` | ✅ |
| 5 | Matrix report, failure correlation, JUnit / Markdown | ✅ |
| 6 | Monitoring TUI | ✅ |

Details and exit criteria: [docs/ROADMAP.md](docs/ROADMAP.md). End-to-end
demo on real VMs: [`scripts/demo.sh`](scripts/demo.sh) and
[docs/how-to-use/demo.md](docs/how-to-use/demo.md).

## Crates

| Crate | Role |
| --- | --- |
| `envprobe-core` | Probe library, report model, diff |
| `envprobe` | Probe CLI |
| `distroproof` | VM orchestrator, matrix, report and TUI |

## envprobe

`envprobe` lists everything that can change how a userspace or kernel-facing
program behaves:

- **Identity**: distro, family, arch, virtualization, container.
- **Kernel**: release, cmdline, config (`CONFIG_BPF*`, `KPROBES`, `DEBUG_INFO_BTF`…), modules.
- **Security and privileges**: sysctls, LSM, SELinux, AppArmor, lockdown, Secure Boot,
  capabilities, seccomp, user namespaces.
- **BPF and perf**: program types, map types and helpers, probed with real
  `bpf()` calls; software and hardware `perf_event_open`, PMU; tracefs, kallsyms, BTF.
- **Userspace**: libc and its version, common libraries, FHS (merged-usr,
  multiarch), `/bin/sh`, coreutils, init, package manager, cgroups, XDG.

Features are probed directly, never inferred from a version number, because
RHEL kernels backport a lot. The probe is read-only and never crashes: missing
or denied information shows up with its reason.

```sh
envprobe                              # human-readable summary
envprobe --json                       # full report
envprobe --json --section kernel,bpf  # selected sections
envprobe diff a.json b.json           # differences, exit code 1 if any
envprobe schema                       # JSON schema of the report
```

Excerpt of a diff between an Arch host and an Alpine rootfs (same kernel):

```text
[libc]
- interpreter  /lib/ld-linux-aarch64.so.1
+ interpreter  /lib/ld-musl-aarch64.so.1
- kind         glibc
+ kind         musl

[shell]
- sh         /usr/bin/bash
+ sh         /bin/busybox

[packages]
- managers  pacman
+ managers  apk
```

### Report format

Each entry carries a status:

- `ok`: the value.
- `unavailable`: information missing, with the reason (file absent, syscall not implemented…).
- `denied`: access refused (EPERM, EACCES, seccomp…).

The `privileged` field flags results that depend on the process's privileges.
The format is versioned (`schema_version`) and its schema is published in
[`schema/envprobe-report.schema.json`](schema/envprobe-report.schema.json).

### Probing another distro without a VM

To compare another distro's userspace without a VM or Docker (the kernel stays
the host's):

```sh
scripts/probe-rootfs.sh <extracted rootfs> <static envprobe> --json
```

The script runs the probe in user, mount and pid namespaces, without root.

## distroproof

`distroproof` boots real QEMU/KVM VMs from each distro's official cloud
images: it is **the distro's kernel** that runs (4.18 on Rocky 8, 5.14 on
Rocky 9…), not the host's.

```sh
distroproof run --target rocky-8 -- envprobe --json > rocky8.json
distroproof up                        # matrix from distroproof.toml
distroproof check                     # validate the file without starting VMs
distroproof images                    # catalog and cache status
distroproof report matrix.json --format markdown
```

Host requirements: `qemu-system-x86_64`, `qemu-img`, `curl`, KVM (`/dev/kvm`,
automatic fallback to TCG otherwise) and OVMF (`edk2-ovmf`, `ovmf`) for images
that only boot with UEFI. Binaries can be chosen with `DISTROPROOF_QEMU`,
`DISTROPROOF_QEMU_IMG`, `DISTROPROOF_OVMF_CODE` and `DISTROPROOF_OVMF_VARS`.

### Targets

| Image | Observed kernel | Notes |
| --- | --- | --- |
| `debian-10`, `debian-11`, `debian-12`, `debian-13` | 4.19, 5.10, 6.1, 6.12 | 4.19 without BTF; 13 boots with UEFI |
| `ubuntu-18.04`, `ubuntu-20.04`, `ubuntu-22.04`, `ubuntu-24.04` | 4.15, 5.4, 5.15, 6.8 | 4.15 without BTF |
| `rocky-8`, `rocky-9` | 4.18, 5.14 | RHEL kernels with backports; 9 boots with UEFI |
| `fedora-latest` | 6.19 (Fedora 44) | latest release, resolved online |
| `arch` | rolling | |
| `alpine` | 6.18 `virt` | musl, busybox |

Images are downloaded once, verified against the checksum published by the
distro and kept in `$XDG_CACHE_HOME/distroproof/images`. Each run works on a
throwaway qcow2 overlay that is deleted at the end, even after a timeout or a
Ctrl+C; QEMU dies with distroproof.

### How it works

- QEMU starts paused (`-S`); distroproof connects to its QMP socket and to the
  two serial ports (console on ttyS0, agent channel on ttyS1), then starts the
  VM.
- cloud-init (NoCloud seed written in Rust) extracts a payload disk (raw tar
  archive: agent, static `envprobe`, working directory) and starts the agent.
  The agent applies the sysctls, creates the test user, runs `envprobe` then
  the command, and relays stdout and stderr line by line.
- Full outputs, exit codes and the report are written to an output disk (raw
  tar), then the VM shuts down. No 9p or virtiofs, which RHEL kernels lack.
- On first boot, the agent also extracts the kernel and initramfs: later runs
  boot directly (`-kernel`) in 6 to 13 s and can change the kernel command
  line.
- The orchestrator never writes to the terminal: it emits typed events
  (`Booting`, `Output`, `Metrics`, `Finished`…) consumed by the text output,
  the JSON output (`--progress json`) and the TUI.

### Matrix: `distroproof.toml`

```toml
[defaults]
memory = "1G"
cpus = 2
timeout = "5m"

[[target]]
name = "rhel8"
image = "rocky-8"

[[target]]
name = "ubuntu-paranoid"
image = "ubuntu-22.04"
sysctl = { "kernel.perf_event_paranoid" = 3 }

[[target]]
name = "fedora-lockdown"
image = "fedora-latest"
kernel_cmdline = "lockdown=confidentiality"

[[target]]
name = "debian-nopmu"
image = "debian-12"
pmu = false                 # -cpu host,pmu=off
caps = ["perfmon", "bpf"]   # capabilities of the test user

[run]
setup = "./scripts/install-deps.sh"   # as root, before the probe
command = "./target/release/mytool --selftest"
as_user = "test"                      # or "root"
# exclude = [".git", "target"]        # copied to /distroproof/work
# include = ["target/release/mytool"]
```

Errors (unknown key, nonexistent image, invalid duration…) are reported with
their line and column, without starting any VM. `distroproof up` runs the
targets in parallel (`--jobs`), attaches to each result the `envprobe` report
taken right before the command, then:

- prints the targets × result table, with duration, boot time, kernel and a
  stderr excerpt;
- compares each failed target with the closest successful one and points out
  the suspicious difference (missing BTF, paranoid set to 3, musl…);
- writes `--json`, `--junit` (CI) and `--markdown` (PR comment) on request.

```text
[ubuntu-18.04] failed (code 1)
  compared with ubuntu-22.04: 65 significant differences
  probable cause: btf.vmlinux: present, 5286152 bytes → unavailable (/sys/kernel/btf/vmlinux missing)
  also: kernel.CONFIG_DEBUG_INFO_BTF: y → not in the config
```

Examples: [`distroproof.toml`](distroproof.toml) (envprobe dogfooding on 10
targets, run in CI by [`matrix.yml`](.github/workflows/matrix.yml)) and
[`examples/require-btf`](examples/require-btf) (deliberate failure without BTF).

### TUI

In a terminal, `distroproof up` opens a TUI (`--no-tui` for text output,
automatic without a TTY): one card per target with state, boot time, CPU and
memory gauges and a Braille graph; live serial logs with search; `envprobe`
report folded by section and diff against another target; interactive serial
console. With `--hold`, each VM stays up after the command with a root shell
on the console.

| Key | Action |
| --- | --- |
| `hjkl`, arrows | selection, scrolling |
| `space`, `r`, `x` | pause / resume (QMP), restart, kill |
| `c`, `i`, `Ctrl+]` | open the console, type, leave |
| `/`, `n`, `N` | search the logs |
| `p` | pin the pane to the selected target |
| `d`, `D` | diff the report against another target |
| `D L R C` | pane view: dashboard, logs, report, console |
| `Ctrl+W` `s` `v` `q` `hjkl` `<>+-` | tiling: split, close, focus, resize |
| `1`…`9` | workspaces (one per file with several `-f`) |
| `f` | filter: all, running, failed, finished |
| `:` or `Ctrl+P` | command palette |
| `?`, `q` | help, quit |

Colors live in `$XDG_CONFIG_HOME/distroproof/theme.toml` (true color, 256-color
fallback):

```toml
[colors]
accent = "#7aa2f7"
success = "#9ece6a"
failure = "#f7768e"
```

## Build

```sh
rustup target add x86_64-unknown-linux-musl
cargo build --release --target x86_64-unknown-linux-musl
```

The binaries are static: `envprobe` runs as is in any x86_64 VM or container,
and `distroproof` finds the `envprobe` sitting next to it. Linking goes
through `rust-lld` (see `.cargo/config.toml`), which also allows building for
x86_64 from arm64.

Tests: `cargo test --workspace`. To regenerate the schema after changing the
model: `UPDATE_SCHEMA=1 cargo test -p envprobe-core --test reports`.

## License

Licensed under either [MIT](LICENSE-MIT) or [Apache 2.0](LICENSE-APACHE), at
your option.
