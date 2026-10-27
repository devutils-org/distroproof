//! Running one target on a machine already up, over ssh: the same payload and
//! agent as a VM's, extracted into a fresh directory of the host, the agent's
//! messages read from ssh's standard output, its output archive read back,
//! and the directory removed.
//!
//! Nothing is booted, so none of a VM's settings apply: the machine's kernel,
//! memory, PMU and sysctls are its own, and the config refuses them.

use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, Instant};

use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc::UnboundedReceiver;

use crate::event::{Control, Emitter, Stage};
use crate::guest::{shell_quote, Payload};
use crate::images::Arch;
use crate::model::{Outcome, SshHost, TargetResult, VmSpec};
use crate::qemu::RunDir;
use crate::vm::{collect, handle_agent_line, read_output, AgentMsg, BootEnd, Runner, Stop};

/// How long a connection may take before the host counts as unreachable.
const CONNECT_TIMEOUT_S: u32 = 20;

/// The program run as ssh: `$DISTROPROOF_SSH`, else `ssh`.
fn ssh_program() -> String {
    std::env::var("DISTROPROOF_SSH").unwrap_or_else(|_| "ssh".to_string())
}

/// An ssh command running `remote` on the host, never prompting.
fn ssh(host: &SshHost, remote: &str) -> Command {
    let mut cmd = Command::new(ssh_program());
    cmd.args([
        "-o",
        "BatchMode=yes",
        "-o",
        &format!("ConnectTimeout={CONNECT_TIMEOUT_S}"),
        "-o",
        "ServerAliveInterval=15",
    ]);
    if let Some(port) = host.port {
        cmd.args(["-p", &port.to_string()]);
    }
    if let Some(identity) = &host.identity {
        cmd.arg("-i").arg(identity);
    }
    cmd.arg("--").arg(&host.host).arg(remote);
    cmd.stdin(Stdio::null()).kill_on_drop(true);
    cmd
}

/// Runs `remote` and returns its standard output, or why it failed.
async fn output(host: &SshHost, remote: &str, stdin: Option<&Path>) -> Result<Vec<u8>, String> {
    let mut cmd = ssh(host, remote);
    if let Some(path) = stdin {
        let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        cmd.stdin(Stdio::from(file));
    }
    let out = cmd
        .output()
        .await
        .map_err(|e| format!("cannot run {}: {e}", ssh_program()))?;
    if out.status.success() {
        Ok(out.stdout)
    } else {
        Err(format!(
            "ssh {} ({}): {}",
            host.host,
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

/// What the run needs to know of the host before sending anything.
struct Host {
    arch: Arch,
    /// `sudo -n ` when the login is not root, else empty.
    root: &'static str,
    kernel: String,
}

async fn describe(host: &SshHost) -> Result<Host, String> {
    let text = output(
        host,
        "uname -m; uname -r; id -u; if sudo -n true 2>/dev/null; then echo sudo; else echo nosudo; fi",
        None,
    )
    .await
    .map_err(|e| format!("host unreachable: {e}"))?;
    let text = String::from_utf8_lossy(&text);
    let lines: Vec<&str> = text.lines().map(str::trim).collect();
    let [machine, kernel, uid, sudo] = lines[..] else {
        return Err(format!("unexpected answer from {}: {text:?}", host.host));
    };
    let arch = match machine {
        "x86_64" => Arch::X86_64,
        "aarch64" => Arch::Aarch64,
        other => return Err(format!("{}: unsupported architecture {other}", host.host)),
    };
    let root = match (uid, sudo) {
        ("0", _) => "",
        (_, "sudo") => "sudo -n ",
        _ => {
            return Err(format!(
                "{}: the agent runs as root (it creates the test user, grants file \
                 capabilities); log in as root, or allow this login passwordless sudo",
                host.host
            ))
        }
    };
    Ok(Host {
        arch,
        root,
        kernel: kernel.to_string(),
    })
}

impl Runner {
    pub(crate) async fn run_ssh(
        &self,
        spec: &VmSpec,
        host: &SshHost,
        ev: &Emitter,
        ctrl: &mut UnboundedReceiver<Control>,
        result: &mut TargetResult,
    ) -> Result<BootEnd, String> {
        let remote = describe(host).await?;
        ev.info(format!(
            "{}: {} {}, {}",
            host.host,
            remote.arch.uname(),
            remote.kernel,
            if remote.root.is_empty() {
                "as root"
            } else {
                "through sudo"
            }
        ));
        if spec.hold {
            ev.info("--hold has no console to keep on an ssh target: ignored");
        }
        let mut rd = RunDir::create(&spec.name)?;
        rd.keep = self.keep;
        let payload = rd.path("payload.tar");
        let output_path = rd.path("output.raw");
        {
            let spec = spec.clone();
            let envprobe = self.envprobe_for(remote.arch)?;
            let payload = payload.clone();
            tokio::task::spawn_blocking(move || {
                Payload {
                    spec: &spec,
                    envprobe: &envprobe,
                    extract_kernel: false,
                    prepare_only: false,
                }
                .write(&payload)
            })
            .await
            .map_err(|e| e.to_string())??;
        }

        // A fresh directory the test user can traverse; its path comes back.
        // The archive's owners are the host's that wrote it, not this one's:
        // root's tar would try to restore them. A failed unpack leaves nothing.
        let unpack = "D=$(mktemp -d \"${TMPDIR:-/var/tmp}/distroproof.XXXXXX\") || exit 1; \
                      if chmod 755 \"$D\" && tar --no-same-owner -xf - -C \"$D\"; \
                      then echo \"$D\"; else rm -rf \"$D\"; exit 1; fi";
        let dir = output(
            host,
            &format!("{}sh -c {}", remote.root, shell_quote(unpack)),
            Some(&payload),
        )
        .await?;
        let dir = String::from_utf8_lossy(&dir).trim().to_string();
        if !dir.starts_with('/') || dir.contains(char::is_whitespace) {
            return Err(format!("{}: unexpected work directory {dir:?}", host.host));
        }
        let quoted = shell_quote(&dir);

        let ran = self
            .agent(spec, host, &remote, &quoted, &output_path, ev, ctrl, result)
            .await;
        // Whatever happened, the host keeps nothing of the run: the agent's
        // session is ended if it still runs -- the command's children with it
        // -- and its directory removed.
        let stop_group = format!(
            "if [ -f {quoted}/agent.pid ]; then g=$(cat {quoted}/agent.pid); \
             kill -TERM -- -$g 2>/dev/null && sleep 1; kill -KILL -- -$g 2>/dev/null; fi; true"
        );
        let cleanup = format!(
            "{root}sh -c {stop}; {root}rm -rf {quoted}",
            stop = shell_quote(&stop_group),
            root = remote.root,
        );
        if let Err(e) = output(host, &cleanup, None).await {
            ev.info(format!("{dir} may remain on {}: {e}", host.host));
            result
                .warnings
                .push(format!("{dir} may remain on {}: {e}", host.host));
        }
        ran
    }

    #[allow(clippy::too_many_arguments)]
    async fn agent(
        &self,
        spec: &VmSpec,
        host: &SshHost,
        remote: &Host,
        dir: &str,
        output_path: &Path,
        ev: &Emitter,
        ctrl: &mut UnboundedReceiver<Control>,
        result: &mut TargetResult,
    ) -> Result<BootEnd, String> {
        let mut child = ssh(
            host,
            // A session of its own, whose id is noted: the cleanup ends the
            // whole group, the command's own children included.
            &format!(
                "{}env DP_DIR={dir} setsid sh -c {script}",
                remote.root,
                script = shell_quote(&format!(
                    "echo $$ > {dir}/agent.pid; exec sh {dir}/agent.sh"
                )),
            ),
        )
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot run {}: {e}", ssh_program()))?;
        let stderr_task = {
            let mut stderr = child.stderr.take().expect("stderr captured");
            tokio::spawn(async move {
                let mut s = String::new();
                let _ = (&mut stderr).take(64 * 1024).read_to_string(&mut s).await;
                s
            })
        };
        let mut agent = BufReader::new(child.stdout.take().expect("stdout captured"));
        let mut line = Vec::new();
        let connected = Instant::now();
        let mut current: Option<Stage> = None;
        let mut stop: Option<Stop> = None;
        let mut timed_out = false;
        let mut fatal: Option<String> = None;
        let mut done = false;
        let deadline = tokio::time::sleep(spec.timeout);
        tokio::pin!(deadline);

        loop {
            tokio::select! {
                n = agent.read_until(b'\n', &mut line) => {
                    match n {
                        Ok(0) | Err(_) => break,
                        Ok(_) => {
                            let text = String::from_utf8_lossy(&line)
                                .trim_end_matches(['\n', '\r'])
                                .to_string();
                            line.clear();
                            match handle_agent_line(&text, ev, result, connected, &mut current) {
                                AgentMsg::Done => done = true,
                                AgentMsg::Fatal(msg) => fatal = Some(msg),
                                AgentMsg::Other => {}
                            }
                        }
                    }
                }
                cmd = ctrl.recv() => match cmd {
                    Some(Control::Kill) | None => {
                        stop = Some(Stop::Kill);
                        break;
                    }
                    Some(Control::Restart) => {
                        stop = Some(Stop::Restart);
                        break;
                    }
                    // A running machine is not paused, and has no console here.
                    _ => {}
                },
                _ = &mut deadline, if !done => {
                    timed_out = true;
                    break;
                }
            }
        }
        let _ = child.start_kill();
        let _ = child.wait().await;
        // What the agent started may still hold ssh's standard error open
        // (the cleanup that follows kills it): wait for it a moment only.
        let ssh_stderr = tokio::time::timeout(Duration::from_secs(5), stderr_task)
            .await
            .ok()
            .and_then(Result::ok)
            .unwrap_or_default();

        if done {
            let archive =
                output(host, &format!("{}cat {dir}/output.tar", remote.root), None).await?;
            std::fs::write(output_path, archive)
                .map_err(|e| format!("{}: {e}", output_path.display()))?;
            let out = read_output(output_path).await?;
            collect(&out, spec, false, result);
            return Ok(BootEnd::default());
        }
        if let Some(stop) = stop {
            result.outcome = Outcome::Killed;
            return Ok(BootEnd::stopped(stop));
        }
        if timed_out {
            result.outcome = Outcome::TimedOut;
            let phase = match current {
                Some(stage) => format!("during the {} stage", stage.label()),
                None => "on the host".to_string(),
            };
            result.error = Some(format!(
                "timed out after {} {phase}",
                crate::config::format_duration(spec.timeout)
            ));
            return Ok(BootEnd::default());
        }
        if let Some(msg) = fatal {
            return Err(format!("agent: {msg}"));
        }
        Err(format!(
            "the agent on {} stopped before it finished: {}",
            host.host,
            ssh_stderr.trim()
        ))
    }
}
