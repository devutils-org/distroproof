# Changelog

## Unreleased

- **ssh targets**: `[[target]] ssh = "user@host"` runs the same agent on a
  machine already running -- bare metal, a cloud instance -- for what a VM
  hides (PT, topdown, uncore, `resctrl`, GPUs). The payload goes into a fresh
  directory, the agent runs in its own session, and both are removed after
  the run, a timeout or a Ctrl+C included; settings that only describe a VM
  are refused.
- **envprobe**: `hardware.cpufreq_driver` (the driver and its mode),
  `hardware.pmu_name` (Intel's core PMU name), `hardware.thermal_throttle`,
  and `hardware.resctrl` (mounted with its monitoring features, supported,
  or absent).

- `distroproof up --pin-cache`: each VM gets a last-level cache of its own
  (QEMU pinned to one L3 domain), so VMs measuring performance can run in
  parallel; `--jobs` is capped at the number of domains.

## 0.1.0 — 2026-10-21

First release: static x86_64 and aarch64 binaries of `distroproof` and `envprobe`,
each archive signed by a build provenance attestation.

- **envprobe**: describes a Linux environment (identity, kernel and its config,
  sysctls, security modules, privileges and capabilities, BTF, BPF and perf
  support, tracing, libc, libraries, FHS layout, shell, hardware and toolchain),
  as text or versioned JSON ([schema](schema/envprobe-report.schema.json)); never
  crashes, says why a fact is missing. `envprobe diff` compares two reports.
- **VMs**: each target boots the distribution's own kernel and rootfs under QEMU
  (KVM when present). 20 images: Debian 10–13 and sid, Ubuntu 18.04–24.04, Rocky
  8/9, Fedora latest/previous, Arch, Gentoo, Alpine, NixOS, and three aarch64
  images under emulation.
- **Matrix**: `distroproof.toml` declares targets, setup, command, users, kernel
  command lines, file capabilities and files to collect; `distroproof up` runs it
  in parallel and compares failing targets with passing ones on envprobe facts.
- **Reports**: JSON, JUnit and Markdown; a TUI with live logs, report diffs and a
  serial console.
