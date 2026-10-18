# distroproof demo

Six acts, from the simplest to the most complete. The first five are scripted
by [`scripts/demo.sh`](../../scripts/demo.sh); the sixth, the TUI, is played
by hand following the walkthrough below. Everything runs on real VMs.

## Setup

```sh
# Arch: sudo pacman -S qemu-base edk2-ovmf ; Debian/Ubuntu: qemu-system-x86 qemu-utils ovmf
rustup target add x86_64-unknown-linux-musl
cargo build --release --target x86_64-unknown-linux-musl
export PATH="$PWD/target/x86_64-unknown-linux-musl/release:$PATH"
distroproof images pull debian-12 ubuntu-18.04 ubuntu-22.04 rocky-8 rocky-9 alpine
```

The first `pull` downloads about 3 GiB. Run each target once before the demo:
the first boot extracts the kernel and initramfs, later ones boot directly in
a few seconds.

```sh
scripts/demo.sh --list   # the acts
scripts/demo.sh          # acts 1 to 5 (~1 min with a warm cache)
scripts/demo.sh 5        # a single act
```

## Scripted acts

1. **The catalog and the cache**: `distroproof images` lists the targets,
   their source and what is already cached (verified image, extracted kernel).
2. **Each distro's real kernel**: `uname -r` on the host, then in Rocky 8
   (4.18), Rocky 9 (5.14) and Alpine (`virt`). These are not containers: each
   VM runs on its distro's kernel.
3. **Two distros, one diff**: an `envprobe` report in Debian 12 and in Alpine,
   then `envprobe diff`: libc (glibc → musl), `/bin/sh`, package manager, and
   this time the kernel too.
4. **A declarative matrix**: `distroproof check` validates
   `distroproof.toml`; a file targeting a nonexistent image is rejected with
   line and column, without starting any VM; then `distroproof up` runs the
   dogfooding matrix with its variants (paranoid, lockdown, hidden PMU).
5. **Why it breaks**: [`examples/require-btf`](../../examples/require-btf)
   requires `/sys/kernel/btf/vmlinux`. It breaks on Ubuntu 18.04 and
   distroproof names BTF as the probable cause, then writes the Markdown
   summary meant for the PR.

## Act 6 — The TUI

```sh
distroproof up --hold
```

`--hold` keeps each VM up after the command, with a root shell on the serial
console: that is what makes the interactive console usable once the matrix is
done. Without `--hold`, the console shows the boot and then stops with the VM.

Suggested walkthrough:

| Step | Keys | What to show |
| --- | --- | --- |
| Overview | — | one card per target: state, boot time, CPU and memory gauges, Braille graph; the UI stays smooth during boots |
| Navigation | `hjkl`, `f` | select a target; `f` filters: all, running, failed, finished |
| Logs | `L`, `/` `kernel`, `n` `N` | the target's live serial logs, search and jump between matches |
| Pause, resume | `space` | QMP `stop` / `cont`: the CPU gauge drops to zero, then picks up again |
| Kill, restart | `x`, `r` | the card turns to failed, then the VM restarts on a fresh overlay |
| Report | `R`, `Enter`, `h` `l` | `envprobe` report folded by section, expand and collapse |
| Diff | `d`, `d`, `D` | diff against the next target that has a report; `D` goes back to the plain report |
| Console | `c`, `i`, `Ctrl+]` | serial console in a pane; `i` to type (`uname -a`, `dmesg \| tail`), `Ctrl+]` to leave |
| Tiling | `Ctrl+W v`, `Ctrl+W s`, `Ctrl+W hjkl`, `Ctrl+W <>+-`, `Ctrl+W q` | split, switch pane, resize, close |
| Pin | `p` | pin the pane to the selected target, to follow two targets side by side |
| Palette | `:` or `Ctrl+P` | every action by name |
| Help, quit | `?`, `q` | press `q` twice if VMs are still running: everything is stopped and cleaned up |

Several matrices side by side, one workspace per file (`1`, `2` to switch
between them):

```sh
distroproof up -f distroproof.toml -f examples/require-btf/distroproof.toml
```

To check the fallback: `distroproof up | cat` gives the usual text output,
without the TUI, just like in CI.
