//! TUI state, fed only by the event bus.

use std::collections::{HashMap, HashSet, VecDeque};
use std::time::{Duration, Instant};

use ratatui::layout::Rect;
use tokio::sync::mpsc::UnboundedSender;

use super::layout::Node;
use super::theme::Theme;
use crate::event::{Control, Event, Metrics, Stage, Stream, VmEvent};
use crate::model::{Outcome, TargetResult, VmSpec};
use crate::vm::strip_ansi;

/// Samples kept for the graphs (one second each).
pub const HISTORY: usize = 120;
const MAX_LOGS: usize = 20_000;

#[derive(Debug, Clone, PartialEq)]
pub enum Phase {
    Queued,
    Downloading { done: u64, total: Option<u64> },
    Verifying,
    Preparing,
    Booting,
    Running(Option<Stage>),
    Done(Outcome),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogKind {
    Info,
    Stage,
    Stdout,
    Stderr,
    Console,
}

#[derive(Debug, Clone)]
pub struct LogLine {
    pub kind: LogKind,
    pub text: String,
}

pub struct Target {
    pub name: String,
    pub image: String,
    pub group: usize,
    pub memory_bytes: u64,
    pub cpus: u32,
    pub phase: Phase,
    pub paused: bool,
    /// QEMU process alive.
    pub alive: bool,
    pub held: bool,
    pub started: Option<Instant>,
    pub elapsed: Option<Duration>,
    pub boot_ms: Option<u64>,
    pub pid: Option<u32>,
    pub direct_kernel: bool,
    pub cpu: VecDeque<f64>,
    pub mem: VecDeque<u64>,
    pub metrics: Metrics,
    pub logs: Vec<LogLine>,
    console_partial: Vec<u8>,
    pub term: vt100::Parser,
    pub result: Option<TargetResult>,
    pub runs: u32,
}

impl Target {
    fn new(spec: &VmSpec, group: usize) -> Target {
        Target {
            name: spec.name.clone(),
            image: spec.image.clone(),
            group,
            memory_bytes: spec.memory_mib * 1024 * 1024,
            cpus: spec.cpus,
            phase: Phase::Queued,
            paused: false,
            alive: false,
            held: false,
            started: None,
            elapsed: None,
            boot_ms: None,
            pid: None,
            direct_kernel: false,
            cpu: VecDeque::with_capacity(HISTORY),
            mem: VecDeque::with_capacity(HISTORY),
            metrics: Metrics::default(),
            logs: Vec::new(),
            console_partial: Vec::new(),
            term: vt100::Parser::new(24, 80, 2000),
            result: None,
            runs: 0,
        }
    }

    pub fn kernel(&self) -> Option<&str> {
        self.result.as_ref()?.kernel_release()
    }

    pub fn duration(&self) -> Option<Duration> {
        self.elapsed.or_else(|| self.started.map(|s| s.elapsed()))
    }

    pub fn is_running(&self) -> bool {
        !matches!(self.phase, Phase::Queued | Phase::Done(_))
    }

    fn log(&mut self, kind: LogKind, text: impl Into<String>) {
        if self.logs.len() >= MAX_LOGS {
            self.logs.drain(..MAX_LOGS / 10);
        }
        self.logs.push(LogLine {
            kind,
            text: text.into(),
        });
    }

    fn console(&mut self, bytes: &[u8]) {
        self.term.process(bytes);
        for line in complete_lines(&mut self.console_partial, bytes) {
            self.log(LogKind::Console, line);
        }
    }

    fn reset(&mut self) {
        self.phase = Phase::Queued;
        self.paused = false;
        self.held = false;
        self.started = None;
        self.elapsed = None;
        self.boot_ms = None;
        self.cpu.clear();
        self.mem.clear();
        self.metrics = Metrics::default();
        self.result = None;
        self.term = vt100::Parser::new(24, 80, 2000);
    }

    fn apply(&mut self, ev: VmEvent) {
        match ev {
            VmEvent::Queued => {
                if self.result.is_some() || self.started.is_some() {
                    self.reset();
                    self.log(LogKind::Info, "── relance ──");
                }
            }
            VmEvent::Info { message } => self.log(LogKind::Info, message),
            VmEvent::Downloading { done, total } => {
                self.started.get_or_insert_with(Instant::now);
                self.phase = Phase::Downloading { done, total };
            }
            VmEvent::Verifying => self.phase = Phase::Verifying,
            VmEvent::ImageReady { file, cached } => {
                self.started.get_or_insert_with(Instant::now);
                self.log(
                    LogKind::Info,
                    format!("image {file}{}", if cached { " (cache)" } else { "" }),
                );
            }
            VmEvent::Preparing => {
                self.phase = Phase::Preparing;
                self.log(LogKind::Info, "preparation boot: extracting the kernel");
            }
            VmEvent::Booting {
                pid, direct_kernel, ..
            } => {
                self.started.get_or_insert_with(Instant::now);
                if self.phase != Phase::Preparing {
                    self.phase = Phase::Booting;
                }
                self.pid = Some(pid);
                self.alive = true;
                self.direct_kernel = direct_kernel;
                self.runs += 1;
                self.log(
                    LogKind::Info,
                    format!(
                        "QEMU pid {pid}, {}",
                        if direct_kernel {
                            "boot direct"
                        } else {
                            "booting from disk"
                        }
                    ),
                );
            }
            VmEvent::Booted { boot_ms } => {
                if self.phase != Phase::Preparing {
                    self.boot_ms = Some(boot_ms);
                    self.phase = Phase::Running(None);
                }
                self.log(LogKind::Info, format!("agent ready in {} ms", boot_ms));
            }
            VmEvent::Stage { stage } => {
                if self.phase != Phase::Preparing {
                    self.phase = Phase::Running(Some(stage));
                }
                self.log(LogKind::Stage, format!("▶ {}", stage.label()));
            }
            VmEvent::StageExited { stage, code } => {
                self.log(LogKind::Stage, format!("■ {}: code {code}", stage.label()));
            }
            VmEvent::Output { stream, line } => {
                let kind = match stream {
                    Stream::Stdout => LogKind::Stdout,
                    Stream::Stderr => LogKind::Stderr,
                };
                self.log(kind, line);
            }
            VmEvent::Console(bytes) => self.console(&bytes),
            VmEvent::Metrics(m) => {
                if self.cpu.len() >= HISTORY {
                    self.cpu.pop_front();
                    self.mem.pop_front();
                }
                self.cpu.push_back(m.cpu_percent);
                self.mem.push_back(m.rss_bytes);
                self.metrics = m;
            }
            VmEvent::Paused => {
                self.paused = true;
                self.log(LogKind::Info, "pause");
            }
            VmEvent::Resumed => {
                self.paused = false;
                self.log(LogKind::Info, "resumed");
            }
            VmEvent::Finished(result) => {
                self.phase = Phase::Done(result.outcome);
                self.elapsed = Some(Duration::from_millis(result.duration_ms));
                if result.boot_ms.is_some() {
                    self.boot_ms = result.boot_ms;
                }
                let msg = match (&result.error, result.exit_code()) {
                    (Some(e), _) => format!("{}: {e}", result.outcome.label()),
                    (None, Some(c)) => format!("{} (code {c})", result.outcome.label()),
                    (None, None) => result.outcome.label().to_string(),
                };
                self.log(LogKind::Info, msg);
                self.result = Some(*result);
            }
            VmEvent::Holding => {
                self.held = true;
                self.log(
                    LogKind::Info,
                    "VM kept up: root shell on the console (c to open it)",
                );
            }
            VmEvent::Stopped => {
                self.alive = false;
                self.held = false;
                self.paused = false;
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Dashboard,
    Logs,
    Report,
    Console,
}

pub struct Pane {
    pub view: View,
    /// Lines scrolled from the bottom (logs) or from the top (report).
    pub scroll: usize,
    /// Target shown when the pane is pinned; otherwise the selection.
    pub pinned: Option<usize>,
    pub cursor: usize,
    pub collapsed: HashSet<String>,
}

impl Pane {
    pub fn new(view: View) -> Pane {
        Pane {
            view,
            scroll: 0,
            pinned: None,
            cursor: 0,
            collapsed: HashSet::new(),
        }
    }
}

pub struct Workspace {
    pub name: String,
    /// Group of targets (matrix file) shown by the dashboard.
    pub group: Option<usize>,
    pub root: Node,
    pub focus: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Filter {
    All,
    Running,
    Failed,
    Done,
}

impl Filter {
    pub fn label(self) -> &'static str {
        match self {
            Filter::All => "all",
            Filter::Running => "running",
            Filter::Failed => "failed",
            Filter::Done => "finished",
        }
    }

    pub fn next(self) -> Filter {
        match self {
            Filter::All => Filter::Running,
            Filter::Running => Filter::Failed,
            Filter::Failed => Filter::Done,
            Filter::Done => Filter::All,
        }
    }

    pub fn parse(s: &str) -> Option<Filter> {
        Some(match s {
            "all" => Filter::All,
            "running" | "run" => Filter::Running,
            "failed" | "fail" => Filter::Failed,
            "done" | "finished" => Filter::Done,
            _ => return None,
        })
    }

    pub fn accepts(self, t: &Target) -> bool {
        match self {
            Filter::All => true,
            Filter::Running => t.is_running() || t.held,
            Filter::Failed => matches!(
                t.phase,
                Phase::Done(Outcome::Failure | Outcome::Error | Outcome::TimedOut)
            ),
            Filter::Done => matches!(t.phase, Phase::Done(_)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    Normal,
    /// Ctrl+W prefix: window command pending.
    Window,
    /// Input sent to the target's serial console.
    Insert,
    Search(String),
    Palette {
        input: String,
        selected: usize,
    },
    Help,
}

pub struct App {
    pub targets: Vec<Target>,
    pub selected: usize,
    pub filter: Filter,
    pub panes: Vec<Pane>,
    pub workspaces: Vec<Workspace>,
    pub ws: usize,
    pub mode: Mode,
    pub search: Option<String>,
    pub diff_with: Option<usize>,
    pub message: Option<(String, Instant)>,
    pub theme: Theme,
    pub controls: HashMap<String, UnboundedSender<Control>>,
    pub quit_armed: bool,
    pub quit: bool,
    /// Geometry of the last render, for the mouse and navigation.
    pub pane_rects: Vec<(usize, Rect)>,
    pub card_rects: Vec<(Rect, usize)>,
    pub dashboard_cols: usize,
}

impl App {
    pub fn new(
        specs: &[(VmSpec, usize)],
        groups: Vec<String>,
        controls: HashMap<String, UnboundedSender<Control>>,
        theme: Theme,
    ) -> App {
        let targets = specs.iter().map(|(s, g)| Target::new(s, *g)).collect();
        let mut app = App {
            targets,
            selected: 0,
            filter: Filter::All,
            panes: Vec::new(),
            workspaces: Vec::new(),
            ws: 0,
            mode: Mode::Normal,
            search: None,
            diff_with: None,
            message: None,
            theme,
            controls,
            quit_armed: false,
            quit: false,
            pane_rects: Vec::new(),
            card_rects: Vec::new(),
            dashboard_cols: 1,
        };
        // One workspace per matrix: dashboard above the logs.
        let multi = groups.len() > 1;
        for (i, name) in groups.iter().enumerate() {
            let dash = app.add_pane(View::Dashboard);
            let logs = app.add_pane(View::Logs);
            app.workspaces.push(Workspace {
                name: if multi { name.clone() } else { "matrix".into() },
                group: multi.then_some(i),
                root: Node::split(Node::Leaf(dash), Node::Leaf(logs), false, 58),
                focus: dash,
            });
        }
        // Inspection workspace: report and console side by side.
        let report = app.add_pane(View::Report);
        let console = app.add_pane(View::Console);
        app.workspaces.push(Workspace {
            name: "inspection".into(),
            group: None,
            root: Node::split(Node::Leaf(report), Node::Leaf(console), true, 50),
            focus: report,
        });
        app
    }

    pub fn add_pane(&mut self, view: View) -> usize {
        self.panes.push(Pane::new(view));
        self.panes.len() - 1
    }

    pub fn apply(&mut self, ev: Event) {
        if let Some(t) = self.targets.iter_mut().find(|t| t.name == ev.target) {
            t.apply(ev.kind);
        }
    }

    pub fn workspace(&self) -> &Workspace {
        &self.workspaces[self.ws]
    }

    pub fn workspace_mut(&mut self) -> &mut Workspace {
        &mut self.workspaces[self.ws]
    }

    pub fn focused_pane(&self) -> usize {
        self.workspace().focus
    }

    /// Target shown by a pane.
    pub fn pane_target(&self, pane: usize) -> usize {
        self.panes[pane]
            .pinned
            .unwrap_or(self.selected)
            .min(self.targets.len().saturating_sub(1))
    }

    /// Targets visible in the current workspace's dashboard.
    pub fn visible(&self) -> Vec<usize> {
        let group = self.workspace().group;
        self.targets
            .iter()
            .enumerate()
            .filter(|(_, t)| group.is_none_or(|g| t.group == g))
            .filter(|(_, t)| self.filter.accepts(t))
            .map(|(i, _)| i)
            .collect()
    }

    pub fn notify(&mut self, msg: impl Into<String>) {
        self.message = Some((msg.into(), Instant::now()));
    }

    pub fn send(&mut self, target: usize, ctrl: Control) {
        let name = self.targets[target].name.clone();
        let label = match &ctrl {
            Control::Pause => Some("pause"),
            Control::Resume => Some("resume"),
            Control::Kill => Some("kill"),
            Control::Restart => Some("restart"),
            Control::ConsoleInput(_) => None,
        };
        match self.controls.get(&name) {
            Some(tx) if tx.send(ctrl).is_ok() => {
                if let Some(label) = label {
                    self.notify(format!("{label}: {name}"));
                }
            }
            _ => self.notify(format!("{name}: can no longer be controlled")),
        }
    }

    pub fn running_count(&self) -> usize {
        self.targets
            .iter()
            .filter(|t| t.is_running() || t.alive)
            .count()
    }

    pub fn move_selection(&mut self, delta: isize) {
        let visible = self.visible();
        if visible.is_empty() {
            return;
        }
        let pos = visible
            .iter()
            .position(|&i| i == self.selected)
            .unwrap_or(0) as isize;
        let next = (pos + delta).clamp(0, visible.len() as isize - 1) as usize;
        self.selected = visible[next];
    }

    /// Keeps the selection among the visible targets when the filter changes.
    pub fn fix_selection(&mut self) {
        let visible = self.visible();
        if !visible.contains(&self.selected) {
            if let Some(&first) = visible.first() {
                self.selected = first;
            }
        }
    }
}

/// Appends `bytes` to the buffer and returns the complete, cleaned-up lines.
/// Decoding waits for the end of the line: a UTF-8 character or an escape
/// sequence split across two serial port reads stays intact.
fn complete_lines(partial: &mut Vec<u8>, bytes: &[u8]) -> Vec<String> {
    partial.extend_from_slice(bytes);
    let mut lines = Vec::new();
    while let Some(i) = partial.iter().position(|&b| b == b'\n') {
        let raw: Vec<u8> = partial.drain(..=i).collect();
        let line = strip_ansi(&String::from_utf8_lossy(&raw));
        let line = line.trim_end_matches(['\n', '\r']);
        if !line.trim().is_empty() {
            lines.push(line.to_string());
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::complete_lines;

    #[test]
    fn console_lines_survive_split_reads() {
        let mut partial = Vec::new();
        let text = "\x1b[0;32m OK \x1b[0m ttyS0…ice\r\ncommand finished — déjà vu\n\nrest";
        let mut lines = Vec::new();
        for b in text.as_bytes() {
            lines.extend(complete_lines(&mut partial, std::slice::from_ref(b)));
        }
        assert_eq!(lines, [" OK  ttyS0…ice", "command finished — déjà vu"]);
        assert_eq!(partial, b"rest");
    }
}
