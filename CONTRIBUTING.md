# Contributing to distroproof

Issues and pull requests are welcome at
[devutils-org/distroproof](https://github.com/devutils-org/distroproof).

## Before a pull request

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --release --target x86_64-unknown-linux-musl   # envprobe must stay static
```

A pull request also runs the VM matrix in [distroproof.toml](distroproof.toml) on
GitHub Actions and posts its summary.

## Rules the code keeps

- **The probe never crashes and never writes.** A missing fact is `Unknown` or
  `Unavailable` with its reason (permission, missing file, refused syscall); no
  sysctl writes, no persistent BPF program.
- **Probe features, never infer from a version number.** RHEL backports a lot.
- **Say whether a fact depends on privileges.**
- **No `unsafe` outside the dedicated, commented `sys` module.**
- **Dependencies are few** and justified in the commit message.
- A change to the JSON report updates its [schema](schema/envprobe-report.schema.json)
  (see [REFERENCE.md](REFERENCE.md#build)).

The plan the project followed is in [docs/ROADMAP.md](docs/ROADMAP.md).

## License

Contributions are dual-licensed under [MIT](LICENSE-MIT) or
[Apache-2.0](LICENSE-APACHE), at your option, unless stated otherwise.
