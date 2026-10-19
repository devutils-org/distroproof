<p align="center"><code>devutils / VERIFY</code></p>

<h1 align="center">distroproof</h1>

<p align="center">Check that your program holds on every distro.</p>

<p align="center">
  <a href="https://devutils-org.github.io/tools/distroproof/">Website</a> ·
  <a href="#documentation">Documentation</a> ·
  <a href="https://github.com/devutils-org">devutils</a>
</p>

## Overview

- Describe Linux environments with `envprobe`: kernel, libc, privileges and features.
- Run the same program in QEMU VMs on each target distro's own kernel.
- Compare failures with passing targets; export JSON, JUnit or Markdown reports.

The probe, VM matrix, reporting and TUI are implemented as a proof of concept.
The image catalog includes Debian 10–13 and sid, Ubuntu 18.04–24.04, Rocky
8/9, Fedora latest/previous, Arch, Gentoo and Alpine on x86_64, and Debian 13,
Ubuntu 24.04 and Fedora latest on aarch64 (emulated on an x86_64 host): 19
images in total.

## In practice: Omniscient

[Omniscient](https://github.com/moha-bekh/omniscient), a kernel-facing
observability tool (eBPF, perf, BTF, file capabilities), was ported with
distroproof across 16 distributions and 5 variants. It found 16 defects that
its development machine never showed -- a release that would not start on a
third of the images, Debian's `perf_event_paranoid` 3 refusing what
CAP_PERFMON should allow, eBPF maps charged to a 64 KiB `RLIMIT_MEMLOCK` on
5.10, lockdown, a kernel without ftrace, a renamed kernel struct member,
version assumptions RHEL's backports disproved -- each tied to the envprobe
fact that explained it. The case study, finding by finding:
[What distroproof found in Omniscient](https://github.com/moha-bekh/omniscient/blob/main/docs/distroproof.md).

The latest campaign ran all 21 targets in about 12 minutes with four VMs
at a time. After the fixes, no source run listed as usable refused at start;
refusal reasons agreed with envprobe on every target. Reference comparisons
on six distributions still have open differences; the campaign does not
cover aarch64, containers or NixOS.

Use `[run].file_caps` to test capabilities on an installed binary,
`[run].collect` to retrieve result files per target, and `DP_TARGET` / `DP_IMAGE`
in guest scripts. envprobe now reports the function tracer configuration and
soft/hard memlock limits. See the [matrix reference](REFERENCE.md#targets).

## Install

From a source checkout with Rust:

```sh
rustup target add x86_64-unknown-linux-musl
cargo build --release --target x86_64-unknown-linux-musl
```

Add `target/x86_64-unknown-linux-musl/release` to your `PATH`;
keep `envprobe` beside `distroproof`. VMs need `qemu-system-x86_64`, `qemu-img`
and `curl`; KVM accelerates them, and some images require OVMF.
See [build and host requirements](REFERENCE.md).

## Use

```sh
envprobe --json                              # inspect this environment
envprobe diff before.json after.json         # compare two reports
distroproof run --target rocky-8 -- envprobe --json
distroproof check                           # validate distroproof.toml
distroproof up                              # run the configured VM matrix
```

Start with the [example matrix](distroproof.toml). `up` opens a TUI in a terminal;
`--no-tui` selects text output.

## Documentation

[Probe, VM and TUI reference](REFERENCE.md) ·
[Walkthrough](docs/how-to-use/demo.md) · [Roadmap](docs/ROADMAP.md) ·
[Report schema](schema/envprobe-report.schema.json)

## Development

```sh
cargo test --workspace
```

Schema updates and VM demo instructions are in the [reference](REFERENCE.md#build).

## License

[MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.
