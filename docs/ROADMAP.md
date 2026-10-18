# Roadmap — distroproof (Rust POC)

Oct 9, 2026 · @Mohamed Bekheira

## Vision and conventions

POC goal: a Rust tool that precisely describes a Linux environment (userspace + kernel), then starts "target distro" VMs to run a program in them and compare the results. Name: `distroproof`. We start with the probe (`envprobe`), standalone and useful from phase 1.

Each phase is self-contained: an agent completes it without reading the following ones. A phase is only done when all its exit criteria are checked.

Rules for the agent, valid in every phase:

- Stable Rust, 2021 edition, Cargo workspace. `cargo fmt`, `cargo clippy -- -D warnings` and `cargo test` pass before every commit.
- The probe never crashes: any missing or inaccessible information becomes an `Unknown` / `Unavailable` value with the reason (permission, missing file, refused syscall).
- Probe features, never infer from a version number (RHEL backports).
- The probe has no side effects: read-only, no sysctl writes, no persistent BPF program loading.
- The probe binary is built statically (`x86_64-unknown-linux-musl`) so it runs as is in any target VM.
- Minimal dependencies, justified in the commit message. No `unsafe` outside a dedicated, commented `sys` module.
- Don't assume root: each probe states whether its result depends on privileges.

## Phase 0 — Project setup

Goal: a skeleton that builds statically and runs in CI, before any feature.

Tasks:

1. Create a Cargo workspace with three crates: `envprobe-core` (lib, the probes), `envprobe` (CLI binary) and `distroproof` (orchestrator binary, empty for now).
2. Add the musl target and a size-optimized release profile (`lto`, `strip`, `opt-level = "s"`).
3. CLI with `clap`: `envprobe` with no argument prints a human-readable summary; `--json` is reserved for phase 2.
4. GitHub Actions CI: fmt, clippy, tests, musl build, and a check that the binary is really static (`file` or `ldd` → "not a dynamic executable").
5. A short `README.md` and this roadmap file in `docs/ROADMAP.md`.

Exit criteria:

- [x] `cargo build --release --target x86_64-unknown-linux-musl` produces a static binary.
- [x] CI is green on an empty PR.
- [x] `envprobe --version` works.

## Phase 1 — Environment probe (envprobe)

Goal: `envprobe` shows everything that can change the behavior of a userspace or kernel-facing program (perf, BPF) on the current machine. One Rust module per category, each exposing a `probe() -> Section` function.

Suggested order: identity and kernel first, then userspace, then BPF/perf (the trickiest).

| Category | Information | Sources |
| --- | --- | --- |
| Identity | distro, version, family (`ID_LIKE`), arch, virtualized or not, container or not | `/etc/os-release`, `uname()`, `/sys/class/dmi/id/*`, `/proc/1/cgroup`, `/.dockerenv`, `/run/systemd/container` |
| Kernel | version, release, cmdline, config (`CONFIG_BPF*`, `KPROBES`, `UPROBES`, `FTRACE`, `DEBUG_INFO_BTF`, `PERF_EVENTS`) | `uname()`, `/proc/cmdline`, `/proc/config.gz` then `/boot/config-$(uname -r)` |
| Modules | loaded modules, whether loading is possible | `/proc/modules`, `/sys/module` |
| Sysctls | `perf_event_paranoid`, `perf_event_max_sample_rate`, `unprivileged_bpf_disabled`, `bpf_jit_enable`, `bpf_jit_harden`, `kptr_restrict`, `dmesg_restrict`, `yama.ptrace_scope` | `/proc/sys/kernel/*`, `/proc/sys/net/core/*` |
| Security | active LSMs, SELinux mode, AppArmor profiles, lockdown, Secure Boot | `/sys/kernel/security/lsm`, `/sys/fs/selinux/enforce`, `/sys/kernel/security/apparmor`, `/sys/kernel/security/lockdown`, efivars |
| Privileges | uid/euid, effective capabilities (`CAP_BPF`, `CAP_PERFMON`, `CAP_SYS_ADMIN`), seccomp, `NoNewPrivs` | `/proc/self/status` (`CapEff`, `Seccomp`) |
| BTF | presence and size of vmlinux BTF, module BTF | `/sys/kernel/btf/vmlinux`, `/sys/kernel/btf/*` |
| BPF | program types, map types, key helpers, JIT | minimal `bpf(BPF_PROG_LOAD)` calls, equivalent to `bpftool feature probe` |
| perf | software and hardware `perf_event_open`, available PMUs | test call on `PERF_COUNT_SW_CPU_CLOCK` and `PERF_COUNT_HW_CPU_CYCLES`, `/sys/bus/event_source/devices` |
| Tracing | tracefs mounted and where, debugfs, kallsyms readable | `/proc/mounts`, `/sys/kernel/tracing`, `/sys/kernel/debug/tracing`, `/proc/kallsyms` |
| libc | glibc or musl, version | `gnu_get_libc_version()` if available, otherwise read `/lib/ld-musl-*` or the ELF interpreter of `/bin/sh` |
| Libs | presence and version of common libs (libbpf, libelf, zlib, openssl, libz) | `ldconfig -p` if available, otherwise scan `/lib`, `/lib64`, `/usr/lib*` |
| FHS | merged-usr, `/lib64` exists, Debian multiarch, library paths | symlinks of `/bin`, `/lib`, `/sbin`; `/usr/lib/x86_64-linux-gnu` |
| Shell and tools | target of `/bin/sh`, GNU coreutils or busybox | `readlink /bin/sh`, `/bin/ls` pointing to busybox |
| Init and services | PID 1 (systemd, OpenRC, runit…) | `/proc/1/comm`, `/proc/1/exe` |
| Package manager | apt, dnf, pacman, zypper, apk, xbps, nix | binaries present in `PATH` |
| cgroups | v1, v2 or hybrid | `/sys/fs/cgroup`, `/proc/mounts` |
| XDG | defined variables and effective values | environment variables |

The BPF and perf probes must use direct syscalls (through the `libc` or `nix` crate), without depending on libbpf, to keep a simple static binary. An `EPERM` or `EACCES` failure is a result, not an error.

Exit criteria:

- [x] Every category in the table is implemented.
- [x] Run as a normal user then as root, the output differs where expected (privileges, BPF, perf).
- [x] Run in a default Docker container, the probe detects the container and seccomp without crashing.
- [x] Unit tests on the parsers (`os-release`, `config.gz`, `/proc/self/status`) with fixtures from several distros.

## Phase 2 — Structured output and diff

Goal: make the report machine-readable and compare two environments, the foundation for everything that follows.

Tasks:

1. Versioned data model (`schema_version`) with `serde`. Each value carries its status: `Ok(value)`, `Unavailable(reason)`, `Denied(errno)`.
2. `envprobe --json` writes the full report to stdout; `--section kernel,bpf` filters it.
3. JSON schema published in the repository, generated with `schemars`.
4. `envprobe diff a.json b.json` subcommand: shows the differences per section, in color, with a non-zero exit code if there are any.
5. Collect real reports as fixtures: your machine, a Debian container, an Alpine container, plus anything else at hand.

Exit criteria:

- [x] The JSON validates against the published schema.
- [x] `diff` between your machine and an Alpine container shows libc, `/bin/sh` and the package manager, but not the kernel (same kernel).
- [x] Regression tests on at least three fixtures.

## Phase 3 — VM with the distro's kernel and rootfs

Goal: `distroproof run --target debian-12 -- envprobe --json` starts a VM with the **real distro kernel** and its rootfs, runs the command in it and collects the output. This is the riskiest phase: start with a single target that works end to end.

Tasks:

1. Technical choices: QEMU + KVM driven as a subprocess (no virtualization library at first). Official distro cloud images (qcow2) as the source, since they contain the original kernel, initramfs and config.
2. Local cache in `$XDG_CACHE_HOME/distroproof/images`: download, verification of the published checksum, throwaway qcow2 overlay per run.
3. Injection: share the working directory through virtiofs or 9p, or failing that a second disk image. Initial configuration through cloud-init (seed ISO) to create the user and start the command.
4. Collect stdout, stderr and the exit code through the serial console (`-serial`) or virtio-vsock, then shut the VM down cleanly.
5. Guaranteed timeouts and cleanup, even if the VM fails to boot.
6. Targets in order: `debian-12`, `ubuntu-22.04`, `rocky-9`, `rocky-8`, `fedora-latest`, `arch`, `alpine`.

Architecture requirement (prepares phase 6): the orchestrator never writes directly to the terminal. It emits typed events, for example `VmEvent { Downloading, Booting, Running, Output(line), Metrics(..), Exited(code), TimedOut }`, on a tokio channel. The text output, the JSON output and later the TUI are only consumers of this stream. The PID of each QEMU process and one QMP socket per VM (`-qmp unix:...`) are exposed from this phase on.

Speed-up to evaluate afterwards: extract the kernel and initramfs from the image and boot them directly (`-kernel`, `-initrd`), like virtme-ng does, to get under 5 seconds.

Exit criteria:

- [x] The 7 targets boot and return a JSON `envprobe` report.
- [x] Each VM's report shows the distro's kernel (4.18 on Rocky 8, 5.14 on Rocky 9…) and not the host's.
- [x] A repeated run reuses the cache and leaves the disk clean.
- [x] Boot time is measured and shown per target.

## Phase 4 — Declarative file and matrix

Goal: describe the test matrix in a versioned file of the project under test, infrastructure-as-code style. `distroproof up` reads the file and runs every target in parallel.

Target format example (`distroproof.toml`):

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
sysctl = { "kernel.perf_event_paranoid" = "3" }

[[target]]
name = "fedora-lockdown"
image = "fedora-latest"
kernel_cmdline = "lockdown=confidentiality"

[run]
setup = "./scripts/install-deps.sh"
command = "./target/release/mytool --selftest"
as_user = "test"   # or "root"
```

Tasks:

1. Parse and validate the file (`serde` + `toml`), with error messages giving line and column.
2. Per-target variants: sysctls, kernel cmdline, root or non-root user, granted capabilities, PMU exposed or not (`-cpu host` vs a generic model).
3. Bounded parallel execution (`--jobs`), with per-target progress display.
4. `envprobe` runs automatically before the user's command and its report is attached to the result.

Exit criteria:

- [x] A 5-target matrix with variants runs in one command.
- [x] Each target's `envprobe` report reflects the requested variants (paranoid, lockdown).
- [x] An invalid file gives a clear error without starting any VM.

## Phase 5 — Tests and report

Goal: see at a glance where the program works, where it breaks, and why, by tying each failure to an environment difference.

Tasks:

1. Matrix report: targets × result table (success, failure, timeout), duration, stderr excerpt.
2. Correlation: for each failed target, an automatic `envprobe diff` against the closest successful target, to point out the suspect (missing BTF, paranoid set to 3, musl…).
3. Output formats: terminal, JSON, JUnit XML (for CI) and Markdown (for a PR comment).
4. CI integration: example GitHub Actions workflow with KVM enabled and image caching.
5. Dogfooding: run `envprobe` itself in the matrix as the first project under test.

Exit criteria:

- [x] A deliberate failure (a program requiring BTF) is detected on a target without BTF, and the report names BTF as the probable cause.
- [x] The matrix runs in CI on a PR and publishes a Markdown summary.

## Phase 6 — Monitoring TUI

Goal: a modern TUI giving a real-time overview of the matrix, in the spirit of [btop](https://github.com/aristocratos/btop) for graphs and gauges, and of [TUIOS](https://github.com/Gaurav-Gosain/tuios) for pane management (tiling, vim-style modal interface, command palette). It plugs only into the phase 3 event bus, without touching the orchestrator.

Stack: `ratatui` + `crossterm`, `tokio` for the event loop, `tui-term` + `vt100` to show a VM's serial console in a pane with its ANSI sequences.

Views:

- **Dashboard**: one card per target (btop style) with colored state, boot time, duration, CPU and memory gauges, load sparkline over the last seconds.
- **Detail**: live serial logs of the selected target, with scrolling and search.
- **Report**: the target's `envprobe` report and its diff against another target, with collapsible sections.
- **Console**: interactive serial console of a VM in a pane, for manual debugging.
- **Layout**: resizable tiled panes, several workspaces (one matrix per workspace, for example).

Metrics: CPU and memory read from `/proc/<QEMU pid>/stat` and `/proc/<pid>/status` (1 s refresh), VM state and guest memory stats through QMP (`query-status`, balloon), disk and network throughput through `query-blockstats`.

Interactions:

- Vim-style modal navigation (`hjkl`, `/` to search), command palette on `:` or `Ctrl+P`, mouse support.
- Actions on a target: restart, kill, pause and resume (QMP `stop` / `cont`), open the console, start a diff.
- Filter targets by state (failures only, running…).

Rendering: themes configurable in `$XDG_CONFIG_HOME/distroproof/theme.toml`, true color with a 256-color fallback, Braille characters for graphs like btop. Enabled only if stdout is a TTY or with `--tui`; otherwise automatic fallback to text output, which keeps CI intact.

Exit criteria:

- [x] A 7-target matrix is shown in real time, with states, gauges and logs, without flicker or UI freezes during boots.
- [x] Pause, resume, restart and kill work from the TUI.
- [x] A VM's interactive serial console is usable in a pane.
- [x] Without a TTY, the same command produces the usual text output.
- [x] The TUI stays smooth on an 80 × 24 terminal and adapts to resizing.

## Future directions

Outside the POC, to consider once phase 5 is validated:

- **Fast boot**: direct kernel + initramfs boot and a rootfs shared over virtiofs, or even microVMs (Firecracker, Cloud Hypervisor, libkrun) to aim for one second.
- **Kernels on demand**: besides distro kernels, upstream LTS kernels (5.10, 5.15, 6.1, 6.6, 6.12) or a user-built kernel.
- **Arm64**: the same targets through QEMU TCG or an arm64 runner.
- **Standalone `envprobe`**: ship the probe on its own, as a `doctor` that any perf/BPF tool can embed.
- **Profile database**: publish `envprobe` reports for common distros, to compare without even starting a VM.
