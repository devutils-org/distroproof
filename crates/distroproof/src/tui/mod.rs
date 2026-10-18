//! Matrix monitoring TUI, plugged only into the event bus: it reads
//! `Event`s and sends `Control`s.

mod input;
mod layout;
mod render;
mod state;
mod theme;

use std::collections::HashMap;
use std::io;
use std::time::{Duration, Instant};

use ratatui::crossterm::event::{self, DisableMouseCapture, EnableMouseCapture};
use ratatui::crossterm::execute;
use tokio::sync::mpsc::{self, UnboundedSender};

use crate::event::{Control, EventRx};
use crate::model::VmSpec;

pub use state::App;

/// Minimum interval between two renders: at most 30 frames per second,
/// whatever the throughput of the serial consoles.
const FRAME: Duration = Duration::from_millis(33);

/// Runs the TUI until the user quits.
pub async fn run(
    specs: Vec<(VmSpec, usize)>,
    groups: Vec<String>,
    controls: HashMap<String, UnboundedSender<Control>>,
    mut bus: EventRx,
) -> io::Result<()> {
    let (theme, warning) = theme::Theme::load();
    let mut app = App::new(&specs, groups, controls, theme);
    if let Some(w) = warning {
        app.notify(w);
    }

    let mut terminal = ratatui::try_init()?;
    execute!(io::stdout(), EnableMouseCapture)?;

    // The terminal is read in a dedicated thread: the tokio loop never
    // blocks, even during boots.
    let (input_tx, mut input_rx) = mpsc::unbounded_channel();
    std::thread::spawn(move || {
        while let Ok(ev) = event::read() {
            if input_tx.send(ev).is_err() {
                break;
            }
        }
    });

    let mut tick = tokio::time::interval(Duration::from_millis(250));
    let mut bus_open = true;
    let mut dirty = true;
    let mut last_draw = Instant::now() - FRAME;
    let result = loop {
        if dirty && last_draw.elapsed() >= FRAME {
            if let Err(e) = terminal.draw(|f| render::draw(f, &mut app)) {
                break Err(e);
            }
            last_draw = Instant::now();
            dirty = false;
        }
        let wait = FRAME.saturating_sub(last_draw.elapsed());
        tokio::select! {
            ev = bus.recv(), if bus_open => match ev {
                Some(ev) => {
                    app.apply(ev);
                    // Drain the rest of the stream before redrawing.
                    while let Ok(ev) = bus.try_recv() {
                        app.apply(ev);
                    }
                    dirty = true;
                }
                // No senders left: the display stays until `q`.
                None => bus_open = false,
            },
            ev = input_rx.recv() => {
                if let Some(ev) = ev {
                    input::handle(&mut app, ev);
                    while let Ok(ev) = input_rx.try_recv() {
                        input::handle(&mut app, ev);
                    }
                    dirty = true;
                }
                if app.quit {
                    break Ok(());
                }
            }
            _ = tick.tick() => dirty = true,
            _ = tokio::time::sleep(wait), if dirty => {}
        }
    };

    let _ = execute!(io::stdout(), DisableMouseCapture);
    ratatui::restore();
    result
}

#[cfg(test)]
mod tests {
    use super::state::{Mode, View};
    use super::*;
    use crate::event::{Event, Metrics, Stage, Stream, VmEvent};
    use crate::model::{Outcome, StageResult, TargetResult};
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::{Event as TermEvent, KeyCode, KeyEvent, KeyModifiers};
    use ratatui::Terminal;
    use std::collections::BTreeMap;

    fn spec(name: &str, image: &str) -> VmSpec {
        VmSpec {
            name: name.into(),
            image: image.into(),
            memory_mib: 1024,
            cpus: 2,
            timeout: Duration::from_secs(60),
            sysctl: BTreeMap::new(),
            kernel_cmdline: None,
            user: "root".into(),
            caps: Vec::new(),
            pmu: true,
            setup: None,
            command: Some("true".into()),
            env: BTreeMap::new(),
            workdir: None,
            exclude: Vec::new(),
            include: Vec::new(),
            probe: true,
            hold: false,
            file_caps: BTreeMap::new(),
            collect: Vec::new(),
        }
    }

    fn app() -> (App, Vec<mpsc::UnboundedReceiver<Control>>) {
        let names = [
            ("debian-12", "debian-12"),
            ("ubuntu-22.04", "ubuntu-22.04"),
            ("rocky-9", "rocky-9"),
            ("rocky-8", "rocky-8"),
            ("fedora", "fedora-latest"),
            ("arch", "arch"),
            ("alpine", "alpine"),
        ];
        let specs: Vec<(VmSpec, usize)> = names.iter().map(|(n, i)| (spec(n, i), 0)).collect();
        let mut controls = HashMap::new();
        let mut rxs = Vec::new();
        for (s, _) in &specs {
            let (tx, rx) = mpsc::unbounded_channel();
            controls.insert(s.name.clone(), tx);
            rxs.push(rx);
        }
        let (theme, _) = theme::Theme::load();
        let mut app = App::new(&specs, vec!["distroproof.toml".into()], controls, theme);
        let ev = |t: &str, kind| Event {
            target: t.into(),
            kind,
        };
        app.apply(ev(
            "debian-12",
            VmEvent::Booting {
                pid: 1,
                qmp: "/q".into(),
                console: "/c".into(),
                direct_kernel: true,
            },
        ));
        app.apply(ev("debian-12", VmEvent::Booted { boot_ms: 7000 }));
        app.apply(ev(
            "debian-12",
            VmEvent::Stage {
                stage: Stage::Command,
            },
        ));
        for i in 0..50 {
            app.apply(ev(
                "debian-12",
                VmEvent::Metrics(Metrics {
                    cpu_percent: f64::from(i * 4 % 200),
                    rss_bytes: 400 << 20,
                    ..Metrics::default()
                }),
            ));
            app.apply(ev(
                "debian-12",
                VmEvent::Output {
                    stream: Stream::Stdout,
                    line: format!("program line {i}"),
                },
            ));
        }
        app.apply(ev(
            "debian-12",
            VmEvent::Console(b"\x1b[0;32m  OK  \x1b[0m] Started cloud-final\r\nlogin: ".to_vec()),
        ));
        app.apply(ev(
            "rocky-8",
            VmEvent::Downloading {
                done: 500 << 20,
                total: Some(2000 << 20),
            },
        ));
        let mut done = TargetResult::new(&spec("alpine", "alpine"));
        done.outcome = Outcome::Failure;
        done.command = Some(StageResult {
            exit_code: Some(1),
            stdout: String::new(),
            stderr: "boom".into(),
        });
        done.envprobe = Some(envprobe_core::probe(&["kernel", "libc"]));
        app.apply(ev("alpine", VmEvent::Finished(Box::new(done.clone()))));
        let mut ok = done;
        ok.name = "arch".into();
        ok.outcome = Outcome::Success;
        ok.envprobe = Some(envprobe_core::probe(&["kernel"]));
        app.apply(ev("arch", VmEvent::Finished(Box::new(ok))));
        (app, rxs)
    }

    fn render(app: &mut App, w: u16, h: u16) -> String {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| render::draw(f, app)).unwrap();
        let buf = term.backend().buffer().clone();
        let mut out = String::new();
        for y in 0..h {
            for x in 0..w {
                out.push_str(buf[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    fn key(app: &mut App, code: KeyCode, mods: KeyModifiers) {
        input::handle(app, TermEvent::Key(KeyEvent::new(code, mods)));
    }

    fn chars(app: &mut App, s: &str) {
        for c in s.chars() {
            key(app, KeyCode::Char(c), KeyModifiers::NONE);
        }
    }

    #[test]
    fn renders_at_80x24_and_resizes() {
        let (mut app, _rx) = app();
        let small = render(&mut app, 80, 24);
        assert!(small.contains("distroproof"));
        assert!(small.contains("debian-12"));
        assert!(small.contains("command"), "{small}");
        assert!(small.contains("program line 49"), "{small}");
        let big = render(&mut app, 200, 60);
        for name in ["rocky-9", "fedora", "alpine", "downloading 25%"] {
            assert!(big.contains(name), "{name} missing:\n{big}");
        }
        // Tiny terminal: no panic.
        render(&mut app, 20, 5);
    }

    #[test]
    fn keys_drive_views_and_controls() {
        let (mut app, mut rxs) = app();
        render(&mut app, 120, 40);
        // Select and pause the target.
        assert_eq!(app.selected, 0);
        key(&mut app, KeyCode::Char(' '), KeyModifiers::NONE);
        assert!(matches!(rxs[0].try_recv(), Ok(Control::Pause)));
        key(&mut app, KeyCode::Char('l'), KeyModifiers::NONE);
        assert_eq!(app.selected, 1);
        key(&mut app, KeyCode::Char('r'), KeyModifiers::NONE);
        assert!(matches!(rxs[1].try_recv(), Ok(Control::Restart)));
        key(&mut app, KeyCode::Char('x'), KeyModifiers::NONE);
        assert!(matches!(rxs[1].try_recv(), Ok(Control::Kill)));

        // Palette: diff alpine's report against arch.
        app.selected = 6;
        chars(&mut app, ":diff arch");
        key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(app.mode, Mode::Normal);
        assert_eq!(app.diff_with, Some(5));
        assert_eq!(app.panes[app.focused_pane()].view, View::Report);
        let screen = render(&mut app, 120, 40);
        assert!(screen.contains("diff against arch"), "{screen}");

        // Tiling: split, resize, close.
        let before = app.workspace().root.leaves().len();
        key(&mut app, KeyCode::Char('w'), KeyModifiers::CONTROL);
        chars(&mut app, "v");
        assert_eq!(app.workspace().root.leaves().len(), before + 1);
        key(&mut app, KeyCode::Char('w'), KeyModifiers::CONTROL);
        chars(&mut app, ">");
        key(&mut app, KeyCode::Char('w'), KeyModifiers::CONTROL);
        chars(&mut app, "q");
        assert_eq!(app.workspace().root.leaves().len(), before);

        // Interactive console: input goes to the VM.
        app.selected = 0;
        key(&mut app, KeyCode::Char('c'), KeyModifiers::NONE);
        assert_eq!(app.panes[app.focused_pane()].view, View::Console);
        key(&mut app, KeyCode::Char('i'), KeyModifiers::NONE);
        assert_eq!(app.mode, Mode::Insert);
        chars(&mut app, "ls");
        key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        let mut sent = Vec::new();
        while let Ok(Control::ConsoleInput(b)) = rxs[0].try_recv() {
            sent.extend(b);
        }
        assert_eq!(sent, b"ls\r");
        key(&mut app, KeyCode::Char(']'), KeyModifiers::CONTROL);
        assert_eq!(app.mode, Mode::Normal);
        let screen = render(&mut app, 120, 40);
        assert!(screen.contains("login:"), "{screen}");

        // Search the logs.
        key(&mut app, KeyCode::Char('L'), KeyModifiers::NONE);
        chars(&mut app, "/line 3");
        key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(app.search.as_deref(), Some("line 3"));
        let screen = render(&mut app, 120, 40);
        assert!(screen.contains("\"line 3\": 11"), "{screen}");

        // Failed filter: only alpine is left.
        key(&mut app, KeyCode::Char('1'), KeyModifiers::NONE);
        chars(&mut app, ":filter failed");
        key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(app.visible(), vec![6]);

        // Quit: confirmation while VMs are running.
        key(&mut app, KeyCode::Char('q'), KeyModifiers::NONE);
        assert!(!app.quit);
        key(&mut app, KeyCode::Char('q'), KeyModifiers::NONE);
        assert!(app.quit);
    }
}
