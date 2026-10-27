//! Running one target: image, disks, QEMU boot, agent, result.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc::UnboundedReceiver;

use crate::event::{Control, Emitter, Metrics, Stage, Stream, VmEvent};
use crate::guest::{self, GuestOutput, Payload};
use crate::images::{self, Arch, DirectKernel, Image, Store};
use crate::metrics::Sampler;
use crate::model::{Outcome, StageResult, TargetResult, VmSpec};
use crate::qemu::{self, KernelBoot, Machine, Qmp, RunDir};

/// Time given to QEMU to stop on its own after the agent finishes.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(15);
/// Size of the console tail kept in the result.
const CONSOLE_TAIL: usize = 16 * 1024;

#[derive(Clone)]
pub struct Runner {
    pub store: Store,
    /// Static envprobe binary copied into each guest.
    pub envprobe: PathBuf,
    /// Direct boot on the kernel extracted from the image, when available.
    pub direct_boot: bool,
    /// Keep the run directories for debugging.
    pub keep: bool,
    /// One L3 cache domain per VM (`up --pin-cache`).
    pub cache_pool: Option<crate::cache::Pool>,
}

/// End of run requested from outside.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stop {
    Kill,
    Restart,
}

impl Runner {
    /// The static envprobe for a guest of `arch`: the one found for the host,
    /// or for another architecture `$DISTROPROOF_ENVPROBE_AARCH64`, else the
    /// same build for that target beside it
    /// (`target/aarch64-unknown-linux-musl/release/envprobe`).
    pub(crate) fn envprobe_for(&self, arch: Arch) -> Result<PathBuf, String> {
        if arch == Arch::host() {
            return Ok(self.envprobe.clone());
        }
        if let Some(path) = std::env::var_os("DISTROPROOF_ENVPROBE_AARCH64") {
            return Ok(path.into());
        }
        let host = format!("{}-unknown-linux-musl", Arch::host().uname());
        let other = format!("{}-unknown-linux-musl", arch.uname());
        let sibling = PathBuf::from(self.envprobe.to_string_lossy().replace(&host, &other));
        if sibling != self.envprobe && sibling.is_file() {
            return Ok(sibling);
        }
        Err(format!(
            "no static envprobe for {}: `cargo zigbuild --release --target {other} -p envprobe` \
             or set DISTROPROOF_ENVPROBE_AARCH64",
            arch.uname()
        ))
    }

    /// Runs a target and returns its result; never panics on an environment
    /// error (missing image, QEMU failing to start…).
    pub async fn run(
        &self,
        spec: &VmSpec,
        ev: &Emitter,
        ctrl: &mut UnboundedReceiver<Control>,
    ) -> (TargetResult, Option<Stop>) {
        let start = Instant::now();
        let mut result = TargetResult::new(spec);
        // Held until the VM is gone, then free for the next one.
        let lease = self.cache_pool.as_ref().and_then(crate::cache::Pool::take);
        let pin = lease.as_ref().map(|l| l.cpus.as_slice());
        let ran = match &spec.ssh {
            Some(host) => self.run_ssh(spec, host, ev, ctrl, &mut result).await,
            None => {
                self.run_inner(spec, ev, ctrl, &mut result, start, pin)
                    .await
            }
        };
        let end = match ran {
            Ok(end) => end,
            Err(err) => {
                result.outcome = Outcome::Error;
                result.error = Some(err);
                BootEnd::default()
            }
        };
        if !end.emitted {
            result.duration_ms = start.elapsed().as_millis() as u64;
            ev.emit(VmEvent::Finished(Box::new(result.clone())));
        }
        ev.emit(VmEvent::Stopped);
        (result, end.stop)
    }

    async fn run_inner(
        &self,
        spec: &VmSpec,
        ev: &Emitter,
        ctrl: &mut UnboundedReceiver<Control>,
        result: &mut TargetResult,
        started: Instant,
        pin: Option<&[usize]>,
    ) -> Result<BootEnd, String> {
        let image = self.store.ensure(&spec.image, ev).await?;
        result.image_file = Some(image.file.clone());
        if image.arch != Arch::host() {
            // Another architecture boots from its disk through its firmware:
            // its kernel is not extracted for a direct boot (an arm64
            // `vmlinuz` is often an EFI zboot image QEMU cannot load), so its
            // command line cannot be changed.
            if spec.kernel_cmdline.is_some() {
                return Err(format!(
                    "{}: kernel_cmdline needs a direct boot, which an {} image does not have",
                    image.name,
                    image.arch.uname()
                ));
            }
            return self
                .boot(
                    spec, &image, None, false, false, ev, ctrl, result, started, pin,
                )
                .await;
        }

        let mut kernel = image.direct_kernel();
        if spec.kernel_cmdline.is_some() && kernel.is_none() {
            // The kernel must be extracted once before its command line can be
            // changed: preparation boot with no user stage.
            let _guard = self
                .store
                .exclusive(&format!("{}#kernel", image.name))
                .await;
            kernel = image.direct_kernel();
            if kernel.is_none() {
                ev.emit(VmEvent::Preparing);
                let mut prep = TargetResult::new(spec);
                let end = self
                    .boot(
                        spec, &image, None, true, true, ev, ctrl, &mut prep, started, pin,
                    )
                    .await?;
                if let Some(stop) = end.stop {
                    result.outcome = Outcome::Killed;
                    return Ok(BootEnd::stopped(stop));
                }
                kernel = image.direct_kernel();
                if kernel.is_none() {
                    return Err(format!(
                        "kernel not extracted from {}: kernel_cmdline impossible ({})",
                        image.file,
                        prep.warnings.join("; ")
                    ));
                }
            }
        }
        let kernel = kernel.filter(|_| self.direct_boot || spec.kernel_cmdline.is_some());
        let extract = image.direct_kernel().is_none();
        self.boot(
            spec, &image, kernel, extract, false, ev, ctrl, result, started, pin,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn boot(
        &self,
        spec: &VmSpec,
        image: &Image,
        kernel: Option<DirectKernel>,
        extract_kernel: bool,
        prepare_only: bool,
        ev: &Emitter,
        ctrl: &mut UnboundedReceiver<Control>,
        result: &mut TargetResult,
        started: Instant,
        pin: Option<&[usize]>,
    ) -> Result<BootEnd, String> {
        let mut rd = RunDir::create(&spec.name)?;
        rd.keep = self.keep;
        let overlay = rd.path("root.qcow2");
        let seed = rd.path("seed.iso");
        let payload = rd.path("payload.tar");
        let output = rd.path("output.raw");
        let (qmp_path, console_path, agent_path) = (
            rd.socket("qmp.sock"),
            rd.socket("console.sock"),
            rd.socket("agent.sock"),
        );

        qemu::create_overlay(&image.path, &overlay).await?;
        let instance = rd
            .dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| spec.name.clone());
        fs::write(&seed, guest::seed_iso(&instance)).map_err(|e| e.to_string())?;
        {
            let spec = spec.clone();
            let envprobe = self.envprobe_for(image.arch)?;
            let payload = payload.clone();
            tokio::task::spawn_blocking(move || {
                Payload {
                    spec: &spec,
                    envprobe: &envprobe,
                    extract_kernel,
                    prepare_only,
                }
                .write(&payload)
            })
            .await
            .map_err(|e| e.to_string())??;
        }
        fs::File::create(&output)
            .and_then(|f| f.set_len(guest::OUTPUT_DISK_BYTES))
            .map_err(|e| format!("{}: {e}", output.display()))?;

        let cmdline;
        let kernel_boot = match &kernel {
            Some(k) => {
                // Waiting for NTP sync (Arch) blocks cloud-final for ~40 s for
                // nothing: the VM clock already comes from the host.
                cmdline = format!(
                    "{} console=ttyS0,115200 panic=-1 \
                     systemd.mask=systemd-time-wait-sync.service {}",
                    k.cmdline,
                    spec.kernel_cmdline.as_deref().unwrap_or("")
                );
                Some(KernelBoot {
                    kernel: &k.kernel,
                    initrd: k.initrd.as_deref(),
                    cmdline: cmdline.trim().to_string(),
                })
            }
            None => None,
        };
        result.direct_kernel = kernel_boot.is_some();
        // Direct boot needs no firmware; booting UEFI images from disk needs
        // OVMF and a copy of its variables.
        let vars = rd.path("efivars.fd");
        let ovmf = if image.uefi && kernel_boot.is_none() {
            let ovmf = qemu::find_ovmf(image.arch).ok_or_else(|| match image.arch {
                Arch::X86_64 => format!(
                    "{} boots with UEFI: install OVMF (edk2-ovmf) or set \
                     DISTROPROOF_OVMF_CODE and DISTROPROOF_OVMF_VARS",
                    image.name
                ),
                Arch::Aarch64 => format!(
                    "{} boots with UEFI: install AAVMF (edk2-aarch64, qemu-efi-aarch64) \
                     or set DISTROPROOF_AAVMF_CODE and DISTROPROOF_AAVMF_VARS",
                    image.name
                ),
            })?;
            fs::copy(&ovmf.vars, &vars).map_err(|e| format!("{}: {e}", ovmf.vars.display()))?;
            Some(ovmf)
        } else {
            None
        };
        let machine = Machine {
            name: &spec.name,
            arch: image.arch,
            memory_mib: spec.memory_mib,
            cpus: spec.cpus,
            pmu: spec.pmu,
            overlay: &overlay,
            seed: &seed,
            payload: &payload,
            output: &output,
            qmp: &qmp_path,
            console: &console_path,
            agent: &agent_path,
            kernel: kernel_boot,
            uefi: ovmf.as_ref().map(|o| (o.code.as_path(), vars.as_path())),
        };
        let mut cmd = machine.command();
        crate::sys::kill_with_parent(&mut cmd);
        if let Some(cpus) = pin {
            crate::sys::set_affinity(&mut cmd, cpus);
            ev.info(format!(
                "pinned to CPUs {} (one L3 cache of its own)",
                crate::cache::format_list(cpus)
            ));
        }
        let mut child = cmd
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| format!("cannot start QEMU: {e}"))?;
        let pid = child.id().unwrap_or(0);
        let stderr_task = {
            let mut stderr = child.stderr.take().expect("stderr captured");
            tokio::spawn(async move {
                let mut s = String::new();
                let _ = (&mut stderr).take(64 * 1024).read_to_string(&mut s).await;
                s
            })
        };

        // Connect to the sockets while the VM is paused (-S).
        let sockets = async {
            let qmp = Qmp::connect(&qmp_path).await?;
            let console = qemu::connect(&console_path, Duration::from_secs(5)).await?;
            let agent = qemu::connect(&agent_path, Duration::from_secs(5)).await?;
            Ok::<_, String>((qmp, console, agent))
        };
        let (mut qmp, console, agent) = tokio::select! {
            r = sockets => r?,
            status = child.wait() => {
                let err = stderr_task.await.unwrap_or_default();
                return Err(format!(
                    "QEMU stopped during startup ({}): {}",
                    status.map(|s| s.to_string()).unwrap_or_default(),
                    err.trim()
                ));
            }
        };
        ev.emit(VmEvent::Booting {
            pid,
            qmp: qmp_path.clone(),
            console: console_path.clone(),
            direct_kernel: result.direct_kernel,
        });
        qmp.execute("cont", None).await?;
        let boot_start = Instant::now();

        let (mut con_r, mut con_w) = console.into_split();
        let mut agent = BufReader::new(agent);
        let mut agent_line = Vec::new();
        let mut con_buf = vec![0u8; 8192];
        let mut console_tail: Vec<u8> = Vec::new();
        let mut sampler = Sampler::new(pid);
        let mut tick = tokio::time::interval(Duration::from_secs(1));

        let deadline = tokio::time::sleep(spec.timeout);
        tokio::pin!(deadline);
        let mut paused_at: Option<Instant> = None;
        let mut done = false;
        let mut grace: Option<tokio::time::Instant> = None;
        let mut agent_open = true;
        let mut console_open = true;
        let mut stop: Option<Stop> = None;
        let mut timed_out = false;
        let mut fatal: Option<String> = None;
        let mut current_stage: Option<Stage> = None;
        let mut collected: Option<Result<(), String>> = None;
        let mut held = false;

        let exit_status = loop {
            let grace_at = grace;
            let grace_sleep = async move {
                match grace_at {
                    Some(at) => tokio::time::sleep_until(at).await,
                    None => std::future::pending().await,
                }
            };
            tokio::select! {
                status = child.wait() => break status.ok(),
                n = agent.read_until(b'\n', &mut agent_line), if agent_open => {
                    match n {
                        Ok(0) | Err(_) => agent_open = false,
                        Ok(_) => {
                            let line = String::from_utf8_lossy(&agent_line)
                                .trim_end_matches(['\n', '\r'])
                                .to_string();
                            agent_line.clear();
                            match handle_agent_line(&line, ev, result, boot_start, &mut current_stage) {
                                AgentMsg::Done => {
                                    done = true;
                                    // The output disk is complete (sync): read it
                                    // without waiting for the VM to stop.
                                    let res = read_output(&output).await;
                                    if let Ok(out) = &res {
                                        if extract_kernel {
                                            save_kernel(out, &image.kernel_dir(), ev);
                                        }
                                        collect(out, spec, prepare_only, result);
                                    }
                                    let ok = res.is_ok();
                                    collected = Some(res.map(|_| ()));
                                    if spec.hold && !prepare_only && ok {
                                        result.console_tail = tail_text(&console_tail, 60);
                                        result.duration_ms = started.elapsed().as_millis() as u64;
                                        ev.emit(VmEvent::Finished(Box::new(result.clone())));
                                        ev.emit(VmEvent::Holding);
                                        held = true;
                                    } else {
                                        grace = Some(tokio::time::Instant::now() + SHUTDOWN_GRACE);
                                    }
                                }
                                AgentMsg::Fatal(msg) => {
                                    fatal = Some(msg);
                                    let _ = qmp.execute("quit", None).await;
                                }
                                AgentMsg::Other => {}
                            }
                        }
                    }
                }
                n = con_r.read(&mut con_buf), if console_open => {
                    match n {
                        Ok(0) | Err(_) => console_open = false,
                        Ok(n) => {
                            let bytes = con_buf[..n].to_vec();
                            console_tail.extend_from_slice(&bytes);
                            if console_tail.len() > 2 * CONSOLE_TAIL {
                                console_tail.drain(..console_tail.len() - CONSOLE_TAIL);
                            }
                            ev.emit(VmEvent::Console(bytes));
                        }
                    }
                }
                _ = tick.tick() => {
                    if let Some((cpu, rss)) = sampler.sample() {
                        let (rd, wr) = qmp.block_stats().await.unwrap_or_default();
                        let balloon = qmp.balloon().await;
                        ev.emit(VmEvent::Metrics(Metrics {
                            cpu_percent: cpu,
                            rss_bytes: rss,
                            disk_read_bytes: rd,
                            disk_write_bytes: wr,
                            guest_memory_bytes: balloon,
                        }));
                    }
                }
                cmd = ctrl.recv() => match cmd {
                    Some(Control::Pause) if paused_at.is_none() => {
                        if qmp.execute("stop", None).await.is_ok() {
                            paused_at = Some(Instant::now());
                            ev.emit(VmEvent::Paused);
                        }
                    }
                    Some(Control::Resume) => {
                        if let Some(at) = paused_at {
                            if qmp.execute("cont", None).await.is_ok() {
                                paused_at = None;
                                // Time spent paused does not count toward the timeout.
                                let next = deadline.deadline() + at.elapsed();
                                deadline.as_mut().reset(next);
                                ev.emit(VmEvent::Resumed);
                            }
                        }
                    }
                    Some(Control::Kill) => {
                        stop = Some(Stop::Kill);
                        let _ = child.start_kill();
                    }
                    Some(Control::Restart) => {
                        stop = Some(Stop::Restart);
                        let _ = child.start_kill();
                    }
                    Some(Control::ConsoleInput(bytes)) => {
                        let _ = con_w.write_all(&bytes).await;
                    }
                    // No consumers left: Ctrl+C or the TUI exited.
                    None => {
                        stop = Some(Stop::Kill);
                        let _ = child.start_kill();
                    }
                    _ => {}
                },
                _ = &mut deadline, if paused_at.is_none() && !done => {
                    timed_out = true;
                    let _ = child.start_kill();
                }
                _ = grace_sleep => {
                    let _ = qmp.execute("quit", None).await;
                    grace = None;
                    let _ = child.start_kill();
                }
            }
        };
        let qemu_stderr = stderr_task.await.unwrap_or_default();
        result.console_tail = tail_text(&console_tail, 60);

        if let Some(res) = collected {
            // The agent finished: the result stands, even if the VM was killed
            // afterwards (VM kept up with --hold).
            res?;
            return Ok(BootEnd {
                stop: stop.filter(|s| *s == Stop::Restart),
                emitted: held,
            });
        }
        if let Some(stop) = stop {
            result.outcome = Outcome::Killed;
            return Ok(BootEnd::stopped(stop));
        }
        if timed_out {
            result.outcome = Outcome::TimedOut;
            let phase = match (result.boot_ms, current_stage) {
                (None, _) => "during boot".to_string(),
                (Some(_), Some(stage)) => format!("during the {} stage", stage.label()),
                (Some(_), None) => "after boot".to_string(),
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
        let status = exit_status.map(|s| s.to_string()).unwrap_or_default();
        let detail = qemu_stderr.trim();
        Err(if result.boot_ms.is_none() {
            format!("the VM stopped before the agent answered ({status}) {detail}")
        } else {
            format!("the VM stopped before the agent finished ({status}) {detail}")
        })
    }
}

/// End of a boot: requested stop, and result already published (VM kept up).
#[derive(Debug, Default)]
pub(crate) struct BootEnd {
    stop: Option<Stop>,
    emitted: bool,
}

impl BootEnd {
    pub(crate) fn stopped(stop: Stop) -> Self {
        BootEnd {
            stop: Some(stop),
            emitted: false,
        }
    }
}

pub(crate) async fn read_output(path: &Path) -> Result<GuestOutput, String> {
    let path = path.to_path_buf();
    let out = tokio::task::spawn_blocking(move || GuestOutput::read(&path))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("output disk unreadable: {e}"))?;
    if out.is_empty() {
        return Err("output disk empty".into());
    }
    Ok(out)
}

pub(crate) enum AgentMsg {
    Done,
    Fatal(String),
    Other,
}

pub(crate) fn handle_agent_line(
    line: &str,
    ev: &Emitter,
    result: &mut TargetResult,
    boot_start: Instant,
    current: &mut Option<Stage>,
) -> AgentMsg {
    // The firmware (OVMF) also writes to COM2 before the agent: a control
    // message can arrive preceded by stray bytes.
    let line = match line.find("@ ") {
        Some(i) if i > 0 && !line.starts_with("> ") && !line.starts_with("! ") => &line[i..],
        _ => line,
    };
    if let Some(text) = line.strip_prefix("> ") {
        ev.emit(VmEvent::Output {
            stream: Stream::Stdout,
            line: text.to_string(),
        });
    } else if let Some(text) = line.strip_prefix("! ") {
        ev.emit(VmEvent::Output {
            stream: Stream::Stderr,
            line: text.to_string(),
        });
    } else if line == ">" || line == "!" {
        let stream = if line == ">" {
            Stream::Stdout
        } else {
            Stream::Stderr
        };
        ev.emit(VmEvent::Output {
            stream,
            line: String::new(),
        });
    } else if let Some(msg) = line.strip_prefix("@ ") {
        let (kind, rest) = msg.split_once(' ').unwrap_or((msg, ""));
        match kind {
            "READY" if result.boot_ms.is_none() => {
                let boot_ms = boot_start.elapsed().as_millis() as u64;
                result.boot_ms = Some(boot_ms);
                ev.emit(VmEvent::Booted { boot_ms });
            }
            "STAGE" => {
                if let Some(stage) = Stage::parse(rest.trim()) {
                    *current = Some(stage);
                    ev.emit(VmEvent::Stage { stage });
                }
            }
            "EXIT" => {
                let mut parts = rest.split_whitespace();
                if let (Some(stage), Some(code)) = (
                    parts.next().and_then(Stage::parse),
                    parts.next().and_then(|c| c.parse().ok()),
                ) {
                    *current = None;
                    ev.emit(VmEvent::StageExited { stage, code });
                }
            }
            "WARN" => {
                result.warnings.push(rest.to_string());
                ev.info(format!("guest: {rest}"));
            }
            "DONE" => return AgentMsg::Done,
            "FATAL" => return AgentMsg::Fatal(rest.to_string()),
            _ => {}
        }
    }
    AgentMsg::Other
}

pub(crate) fn collect(
    out: &GuestOutput,
    spec: &VmSpec,
    prepare_only: bool,
    result: &mut TargetResult,
) {
    let stage = |name: &str| StageResult {
        exit_code: out.code(name),
        stdout: out.text(&format!("{name}.out")).unwrap_or_default(),
        stderr: out.text(&format!("{name}.err")).unwrap_or_default(),
    };
    if spec.setup.is_some() && !prepare_only {
        result.setup = Some(stage("setup"));
    }
    if spec.command.is_some() && !prepare_only {
        result.command = Some(stage("command"));
    }
    if !spec.collect.is_empty() && !prepare_only {
        let base = spec
            .workdir
            .clone()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".distroproof/collect")
            .join(&spec.name);
        for (name, data) in out.files() {
            let Some(rel) = name.strip_prefix("collect/") else {
                continue;
            };
            let path = base.join(rel);
            let written = path
                .parent()
                .map_or(Ok(()), std::fs::create_dir_all)
                .and_then(|()| std::fs::write(&path, data));
            match written {
                Ok(()) => result.collected.push(path.display().to_string()),
                Err(e) => result
                    .warnings
                    .push(format!("collect: {}: {e}", path.display())),
            }
        }
    }
    if let Some(json) = out.get("envprobe.json") {
        match serde_json::from_slice(json) {
            Ok(report) => result.envprobe = Some(report),
            Err(e) => result.warnings.push(format!(
                "envprobe report unreadable: {e} ({})",
                out.text("envprobe.err").unwrap_or_default().trim()
            )),
        }
    }
    let setup_failed = result
        .setup
        .as_ref()
        .is_some_and(|s| s.exit_code != Some(0));
    result.outcome = match (&result.command, setup_failed) {
        (_, true) => {
            result.error = Some(format!(
                "setup failed (code {})",
                result
                    .setup
                    .as_ref()
                    .and_then(|s| s.exit_code)
                    .map_or("?".into(), |c| c.to_string())
            ));
            Outcome::Failure
        }
        (Some(c), false) if c.exit_code == Some(0) => Outcome::Success,
        (Some(_), false) => Outcome::Failure,
        (None, false) => Outcome::Success,
    };
}

/// Stores the kernel and initramfs extracted by the agent, atomically.
fn save_kernel(out: &GuestOutput, dir: &Path, ev: &Emitter) {
    let (Some(kernel), Some(cmdline)) = (out.get("kernel/vmlinuz"), out.get("kernel/cmdline"))
    else {
        return;
    };
    if dir.join("vmlinuz").is_file() {
        return;
    }
    let tmp = dir.with_extension(format!("kernel.tmp{}", std::process::id()));
    let res = (|| {
        fs::create_dir_all(&tmp)?;
        fs::write(tmp.join("vmlinuz"), kernel)?;
        if let Some(initrd) = out.get("kernel/initrd") {
            fs::write(tmp.join("initrd"), initrd)?;
        }
        fs::write(tmp.join("cmdline"), cmdline)?;
        if let Some(release) = out.get("kernel/release") {
            fs::write(tmp.join("release"), release)?;
        }
        fs::rename(&tmp, dir)
    })();
    match res {
        Ok(()) => ev.info(format!(
            "kernel extracted ({}), direct boot on the next runs",
            images::clean_cmdline(&String::from_utf8_lossy(cmdline))
        )),
        Err(e) => {
            let _ = fs::remove_dir_all(&tmp);
            ev.info(format!("extracted kernel not saved: {e}"));
        }
    }
}

/// Last lines of the console, without escape sequences.
pub fn tail_text(bytes: &[u8], lines: usize) -> String {
    let text = strip_ansi(&String::from_utf8_lossy(bytes));
    let all: Vec<&str> = text
        .lines()
        .map(|l| l.trim_end_matches('\r'))
        .filter(|l| !l.trim().is_empty())
        .collect();
    all[all.len().saturating_sub(lines)..].join("\n")
}

pub fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            match chars.peek() {
                Some('[') => {
                    chars.next();
                    for c in chars.by_ref() {
                        if ('@'..='~').contains(&c) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    for c in chars.by_ref() {
                        if c == '\x07' {
                            break;
                        }
                    }
                }
                _ => {
                    chars.next();
                }
            }
        } else if c == '\r' || c == '\n' || c == '\t' || !c.is_control() {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_escape_sequences() {
        assert_eq!(
            strip_ansi("\x1b[0;32m  OK  \x1b[0m Started"),
            "  OK   Started"
        );
        assert_eq!(tail_text(b"a\r\n\r\nb\nc\n", 2), "b\nc");
    }
}
