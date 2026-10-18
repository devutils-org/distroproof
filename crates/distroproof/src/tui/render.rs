//! Rendering: bars, tiled panes, dashboard cards, logs, report, console,
//! command palette and help.

use std::time::Duration;

use envprobe_core::diff::{self, Change, SectionDiff};
use envprobe_core::Value;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph};
use ratatui::Frame;
use tui_term::widget::PseudoTerminal;

use super::input::palette_matches;
use super::state::{App, LogKind, Mode, Phase, Target, View};
use crate::model::Outcome;
use crate::progress::fmt_bytes;

pub fn draw(f: &mut Frame, app: &mut App) {
    let [top, body, bottom] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .areas(f.area());
    top_bar(f, app, top);
    bottom_bar(f, app, bottom);

    let mut rects = Vec::new();
    app.workspace().root.layout(body, &mut rects);
    app.card_rects.clear();
    let focus = app.focused_pane();
    for (pane, area) in &rects {
        if area.width < 4 || area.height < 3 {
            continue;
        }
        let focused = *pane == focus;
        match app.panes[*pane].view {
            View::Dashboard => dashboard(f, app, *area, focused),
            View::Logs => logs(f, app, *pane, *area, focused),
            View::Report => report(f, app, *pane, *area, focused),
            View::Console => console(f, app, *pane, *area, focused),
        }
    }
    app.pane_rects = rects;

    match app.mode.clone() {
        Mode::Palette { input, selected } => palette(f, app, &input, selected),
        Mode::Help => help(f, app),
        _ => {}
    }
}

fn pane_block<'a>(app: &App, title: Line<'a>, focused: bool) -> Block<'a> {
    let color = if focused {
        app.theme.border_focus
    } else {
        app.theme.border
    };
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(color))
        .title(title)
}

fn top_bar(f: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;
    let mut spans = vec![Span::styled(
        " distroproof ",
        Style::new()
            .fg(Color::Black)
            .bg(t.accent)
            .add_modifier(Modifier::BOLD),
    )];
    for (i, ws) in app.workspaces.iter().enumerate() {
        let style = if i == app.ws {
            Style::new()
                .fg(t.accent)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
        } else {
            Style::new().fg(t.muted)
        };
        spans.push(Span::raw(" "));
        spans.push(Span::styled(format!("{}:{}", i + 1, ws.name), style));
    }
    let ok = app
        .targets
        .iter()
        .filter(|x| x.phase == Phase::Done(Outcome::Success))
        .count();
    let ko = app
        .targets
        .iter()
        .filter(|x| matches!(x.phase, Phase::Done(o) if o != Outcome::Success))
        .count();
    let running = app.targets.iter().filter(|x| x.is_running()).count();
    spans.extend([
        Span::styled("  │ ", Style::new().fg(t.border)),
        Span::styled(format!("✔ {ok}"), Style::new().fg(t.success)),
        Span::raw("  "),
        Span::styled(format!("✘ {ko}"), Style::new().fg(t.failure)),
        Span::raw("  "),
        Span::styled(format!("● {running}"), Style::new().fg(t.running)),
        Span::styled(
            format!(
                "  / {}  │ filter: {}",
                app.targets.len(),
                app.filter.label()
            ),
            Style::new().fg(t.muted),
        ),
    ]);
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn bottom_bar(f: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;
    let (label, text) = match &app.mode {
        Mode::Normal | Mode::Palette { .. } | Mode::Help => {
            let msg = app
                .message
                .as_ref()
                .filter(|(_, at)| at.elapsed() < Duration::from_secs(4))
                .map(|(m, _)| m.clone());
            (
                " NORMAL ",
                msg.unwrap_or_else(|| {
                    "hjkl select · space pause · r restart · x kill · c console · / search · : commands · ? help · q quit".into()
                }),
            )
        }
        Mode::Window => (
            " WINDOW ",
            "s/v split · q close · hjkl/w focus · < > + - resize · Esc cancel".into(),
        ),
        Mode::Insert => (
            " CONSOLE ",
            "input sent to the serial console · Ctrl+] to leave".into(),
        ),
        Mode::Search(s) => (" SEARCH ", format!("/{s}")),
    };
    let line = Line::from(vec![
        Span::styled(
            label,
            Style::new()
                .fg(Color::Black)
                .bg(match app.mode {
                    Mode::Insert => t.warning,
                    Mode::Search(_) => t.search,
                    Mode::Window => t.cpu,
                    _ => t.accent,
                })
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(text, Style::new().fg(t.muted)),
    ]);
    f.render_widget(Paragraph::new(line), area);
}

pub fn status(app: &App, t: &Target) -> (String, Color) {
    let th = &app.theme;
    if t.paused {
        return ("⏸ paused".into(), th.warning);
    }
    match &t.phase {
        Phase::Queued => ("queued".into(), th.muted),
        Phase::Downloading { done, total } => (
            match total {
                Some(total) if *total > 0 => format!("downloading {}%", done * 100 / total),
                _ => format!("downloading {}", fmt_bytes(*done)),
            },
            th.warning,
        ),
        Phase::Verifying => ("verifying".into(), th.warning),
        Phase::Preparing => ("extracting kernel".into(), th.running),
        Phase::Booting => ("boot…".into(), th.running),
        Phase::Running(stage) => (
            match stage {
                Some(s) => format!("● {}", s.label()),
                None => "● agent ready".into(),
            },
            th.running,
        ),
        Phase::Done(o) => {
            let mut label = match (o, t.result.as_ref().and_then(|r| r.exit_code())) {
                (Outcome::Failure, Some(c)) => format!("✘ failed ({c})"),
                (Outcome::Success, _) => "✔ success".into(),
                (o, _) => format!("✘ {}", o.label()),
            };
            if t.held && t.alive {
                label.push_str(" · console");
            }
            let color = match o {
                Outcome::Success => th.success,
                Outcome::TimedOut | Outcome::Killed => th.warning,
                _ => th.failure,
            };
            (label, color)
        }
    }
}

fn fmt_clock(d: Duration) -> String {
    let s = d.as_secs();
    format!("{:02}:{:02}", s / 60, s % 60)
}

/// Horizontal gauge with eighth blocks.
fn gauge(ratio: f64, width: usize, color: Color, bg: Color) -> Vec<Span<'static>> {
    let ratio = ratio.clamp(0.0, 1.0);
    let eighths = (ratio * width as f64 * 8.0).round() as usize;
    let full = eighths / 8;
    let part = eighths % 8;
    let partials = ["", "▏", "▎", "▍", "▌", "▋", "▊", "▉"];
    let mut filled = "█".repeat(full);
    let mut used = full;
    if part > 0 && used < width {
        filled.push_str(partials[part]);
        used += 1;
    }
    vec![
        Span::styled(filled, Style::new().fg(color)),
        Span::styled("·".repeat(width.saturating_sub(used)), Style::new().fg(bg)),
    ]
}

/// Graph in Braille characters (2 samples × 4 levels per cell), right-aligned
/// on the most recent samples.
pub fn braille(samples: &[f64], max: f64, width: usize, height: usize) -> Vec<String> {
    let cols = width * 2;
    let start = samples.len().saturating_sub(cols);
    let data = &samples[start..];
    let offset = cols - data.len();
    let levels = height * 4;
    let mut cells = vec![vec![0u8; width]; height];
    for (i, v) in data.iter().enumerate() {
        let x = offset + i;
        let level = if max > 0.0 {
            ((v / max).clamp(0.0, 1.0) * levels as f64).round() as usize
        } else {
            0
        };
        let level = if *v > 0.0 { level.max(1) } else { level };
        for (row, line) in cells.iter_mut().enumerate() {
            let base = (height - 1 - row) * 4;
            let dots = level.saturating_sub(base).min(4);
            for d in 0..dots {
                // Dots from bottom to top: rows 3, 2, 1, 0 of the cell.
                let bit = match (x % 2, 3 - d) {
                    (0, 0) => 0x01,
                    (0, 1) => 0x02,
                    (0, 2) => 0x04,
                    (0, _) => 0x40,
                    (_, 0) => 0x08,
                    (_, 1) => 0x10,
                    (_, 2) => 0x20,
                    _ => 0x80,
                };
                line[x / 2] |= bit;
            }
        }
    }
    cells
        .into_iter()
        .map(|row| {
            row.into_iter()
                .map(|b| char::from_u32(0x2800 + u32::from(b)).unwrap_or(' '))
                .collect()
        })
        .collect()
}

fn dashboard(f: &mut Frame, app: &mut App, area: Rect, focused: bool) {
    let visible = app.visible();
    let title = Line::from(vec![
        Span::styled(
            " dashboard ",
            Style::new()
                .fg(app.theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("{} target(s) ", visible.len()),
            Style::new().fg(app.theme.muted),
        ),
    ]);
    let block = pane_block(app, title, focused);
    let inner = block.inner(area);
    f.render_widget(block, area);
    if visible.is_empty() {
        f.render_widget(
            Paragraph::new("no targets for this filter (f to change it)")
                .style(Style::new().fg(app.theme.muted)),
            inner,
        );
        return;
    }
    let cols = (inner.width / 34).max(1) as usize;
    let card_h: u16 = if inner.height >= 16 { 8 } else { 5 };
    let card_w = inner.width / cols as u16;
    app.dashboard_cols = cols;
    let rows_fit = (inner.height / card_h).max(1) as usize;
    let sel_pos = visible.iter().position(|&i| i == app.selected).unwrap_or(0);
    let first_row = (sel_pos / cols).saturating_sub(rows_fit - 1);
    for (pos, &idx) in visible.iter().enumerate().skip(first_row * cols) {
        let row = pos / cols - first_row;
        if row >= rows_fit {
            break;
        }
        let col = pos % cols;
        let w = if col == cols - 1 {
            inner.width - card_w * (cols as u16 - 1)
        } else {
            card_w
        };
        let rect = Rect::new(
            inner.x + card_w * col as u16,
            inner.y + card_h * row as u16,
            w,
            card_h.min(inner.y + inner.height - (inner.y + card_h * row as u16)),
        );
        card(f, app, idx, rect);
        app.card_rects.push((rect, idx));
    }
}

fn card(f: &mut Frame, app: &App, idx: usize, rect: Rect) {
    let t = &app.targets[idx];
    let th = &app.theme;
    let selected = idx == app.selected;
    let (status_text, color) = status(app, t);
    let block = Block::bordered()
        .border_type(if selected {
            BorderType::Thick
        } else {
            BorderType::Rounded
        })
        .border_style(Style::new().fg(if selected { color } else { th.border }))
        .title(Line::from(Span::styled(
            format!(" {} ", t.name),
            Style::new().fg(th.fg).add_modifier(Modifier::BOLD),
        )))
        .title(
            Line::from(Span::styled(
                format!(" {status_text} "),
                Style::new().fg(color),
            ))
            .right_aligned(),
        );
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    if inner.height == 0 || inner.width < 10 {
        return;
    }
    let w = inner.width as usize;
    let muted = Style::new().fg(th.muted);
    let mut lines = Vec::new();
    lines.push(Line::from(vec![
        Span::styled(t.image.clone(), Style::new().fg(th.accent)),
        Span::styled(" · ", muted),
        Span::styled(
            t.kernel().unwrap_or("kernel ?").to_string(),
            Style::new().fg(th.fg),
        ),
    ]));
    let boot = t
        .boot_ms
        .map(|b| format!("boot {:.1} s", b as f64 / 1000.0))
        .unwrap_or_else(|| "boot -".into());
    let clock = t
        .duration()
        .map(fmt_clock)
        .unwrap_or_else(|| "--:--".into());
    lines.push(Line::from(vec![
        Span::styled(format!("⏱ {clock}"), Style::new().fg(th.fg)),
        Span::styled(format!("  {boot}"), muted),
        Span::styled(if t.direct_kernel { "  direct" } else { "" }, muted),
    ]));
    let gauge_w = w.saturating_sub(14).max(4);
    // 100% per vCPU: a saturated 2-CPU VM shows 200%.
    let cpu_max = 100.0 * f64::from(t.cpus.max(1));
    let alive = t.alive;
    let cpu = if alive { t.metrics.cpu_percent } else { 0.0 };
    let mut cpu_line = vec![Span::styled("CPU ", muted)];
    cpu_line.extend(gauge(cpu / cpu_max, gauge_w, th.cpu, th.border));
    cpu_line.push(Span::styled(
        format!(" {:>4.0} %", cpu),
        Style::new().fg(th.cpu),
    ));
    lines.push(Line::from(cpu_line));
    let rss = if alive { t.metrics.rss_bytes } else { 0 };
    let mut mem_line = vec![Span::styled("MEM ", muted)];
    mem_line.extend(gauge(
        rss as f64 / t.memory_bytes.max(1) as f64,
        gauge_w,
        th.mem,
        th.border,
    ));
    mem_line.push(Span::styled(
        format!(" {:>6}", short_bytes(rss)),
        Style::new().fg(th.mem),
    ));
    lines.push(Line::from(mem_line));
    let graph_h = (inner.height as usize).saturating_sub(lines.len());
    if graph_h > 0 {
        let samples: Vec<f64> = t.cpu.iter().copied().collect();
        let max = samples.iter().copied().fold(cpu_max, f64::max);
        for row in braille(&samples, max, w, graph_h) {
            lines.push(Line::from(Span::styled(row, Style::new().fg(th.cpu))));
        }
    }
    let mut p = Paragraph::new(lines);
    if selected {
        p = p.style(Style::new().bg(th.selection));
    }
    f.render_widget(p, inner);
}

fn short_bytes(n: u64) -> String {
    if n >= 1 << 30 {
        format!("{:.1}G", n as f64 / (1u64 << 30) as f64)
    } else {
        format!("{}M", n >> 20)
    }
}

fn target_title<'a>(app: &App, pane: usize, view: &str, extra: String) -> Line<'a> {
    let idx = app.pane_target(pane);
    let t = &app.targets[idx];
    let (st, color) = status(app, t);
    let pin = if app.panes[pane].pinned.is_some() {
        " 📌"
    } else {
        ""
    };
    Line::from(vec![
        Span::styled(
            format!(" {view} "),
            Style::new()
                .fg(app.theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!("{}{pin} ", t.name), Style::new().fg(app.theme.fg)),
        Span::styled(format!("{st} "), Style::new().fg(color)),
        Span::styled(extra, Style::new().fg(app.theme.muted)),
    ])
}

fn highlight<'a>(text: &'a str, needle: Option<&str>, base: Style, hl: Style) -> Line<'a> {
    let Some(needle) = needle.filter(|n| !n.is_empty()) else {
        return Line::from(Span::styled(text, base));
    };
    let lower = text.to_lowercase();
    let n = needle.to_lowercase();
    // Match on the lowercase version and slice the original if the lengths
    // agree (ASCII); otherwise highlight the whole line.
    if lower.len() != text.len() {
        return if lower.contains(&n) {
            Line::from(Span::styled(text, hl))
        } else {
            Line::from(Span::styled(text, base))
        };
    }
    let mut spans = Vec::new();
    let mut last = 0;
    for (i, _) in lower.match_indices(&n) {
        if i > last {
            spans.push(Span::styled(&text[last..i], base));
        }
        spans.push(Span::styled(&text[i..i + n.len()], hl));
        last = i + n.len();
    }
    spans.push(Span::styled(&text[last..], base));
    Line::from(spans)
}

fn logs(f: &mut Frame, app: &mut App, pane: usize, area: Rect, focused: bool) {
    let idx = app.pane_target(pane);
    let height = area.height.saturating_sub(2) as usize;
    let total = app.targets[idx].logs.len();
    let max_scroll = total.saturating_sub(height);
    let p = &mut app.panes[pane];
    p.scroll = p.scroll.min(max_scroll);
    let scroll = p.scroll;
    let follow = if scroll == 0 { "following" } else { "scrolled" };
    let search = app.search.clone();
    let matches = search
        .as_ref()
        .map(|s| {
            let s = s.to_lowercase();
            app.targets[idx]
                .logs
                .iter()
                .filter(|l| l.text.to_lowercase().contains(&s))
                .count()
        })
        .map(|n| format!(" · \"{}\": {n}", search.as_deref().unwrap_or("")))
        .unwrap_or_default();
    let title = target_title(
        app,
        pane,
        "logs",
        format!("{total} lines · {follow}{matches} "),
    );
    let block = pane_block(app, title, focused);
    let th = app.theme;
    let t = &app.targets[idx];
    let end = total - scroll;
    let start = end.saturating_sub(height);
    let hl = Style::new().fg(Color::Black).bg(th.search);
    let lines: Vec<Line> = t.logs[start..end]
        .iter()
        .map(|l| {
            let style = match l.kind {
                LogKind::Info => Style::new().fg(th.accent),
                LogKind::Stage => Style::new().fg(th.cpu).add_modifier(Modifier::BOLD),
                LogKind::Stdout => Style::new().fg(th.fg),
                LogKind::Stderr => Style::new().fg(th.failure),
                LogKind::Console => Style::new().fg(th.muted),
            };
            highlight(&l.text, search.as_deref(), style, hl)
        })
        .collect();
    f.render_widget(Paragraph::new(lines).block(block), area);
}

/// Logical line of the report pane.
pub enum ReportRow {
    Section {
        name: String,
        count: usize,
        collapsed: bool,
    },
    Entry(Line<'static>),
}

pub fn report_rows(app: &App, pane: usize) -> Vec<ReportRow> {
    let idx = app.pane_target(pane);
    let th = app.theme;
    let collapsed = &app.panes[pane].collapsed;
    let Some(rep) = app.targets[idx]
        .result
        .as_ref()
        .and_then(|r| r.envprobe.as_ref())
    else {
        return Vec::new();
    };
    let value_style = |v: &Value| match v {
        Value::Ok(_) => Style::new().fg(th.fg),
        Value::Unavailable(_) => Style::new().fg(th.warning),
        Value::Denied(_) => Style::new().fg(th.failure),
    };
    let mut rows = Vec::new();
    let other = app
        .diff_with
        .filter(|&o| o != idx)
        .and_then(|o| app.targets[o].result.as_ref()?.envprobe.as_ref());
    match other {
        Some(other) => {
            for d in diff::diff(other, rep) {
                let name = d.name().to_string();
                let is_collapsed = collapsed.contains(&name);
                let changes = match &d {
                    SectionDiff::Changed { changes, .. } => changes.len(),
                    _ => 1,
                };
                rows.push(ReportRow::Section {
                    name: name.clone(),
                    count: changes,
                    collapsed: is_collapsed,
                });
                if is_collapsed {
                    continue;
                }
                match d {
                    SectionDiff::OnlyInA(_) => rows.push(ReportRow::Entry(Line::styled(
                        "    - section missing here",
                        Style::new().fg(th.failure),
                    ))),
                    SectionDiff::OnlyInB(_) => rows.push(ReportRow::Entry(Line::styled(
                        "    + section missing from the reference",
                        Style::new().fg(th.success),
                    ))),
                    SectionDiff::Changed { changes, .. } => {
                        let w = changes.iter().map(|c| c.key().len()).max().unwrap_or(0);
                        for c in changes {
                            let (minus, plus) = match &c {
                                Change::Changed { a, b, .. } => (Some(a.clone()), Some(b.clone())),
                                Change::Added { value, .. } => (None, Some(value.clone())),
                                Change::Removed { value, .. } => (Some(value.clone()), None),
                            };
                            if let Some(a) = minus {
                                rows.push(ReportRow::Entry(Line::styled(
                                    format!("    - {:<w$}  {a}", c.key()),
                                    Style::new().fg(th.failure),
                                )));
                            }
                            if let Some(b) = plus {
                                rows.push(ReportRow::Entry(Line::styled(
                                    format!("    + {:<w$}  {b}", c.key()),
                                    Style::new().fg(th.success),
                                )));
                            }
                        }
                    }
                }
            }
        }
        None => {
            for s in &rep.sections {
                let is_collapsed = collapsed.contains(&s.name);
                rows.push(ReportRow::Section {
                    name: s.name.clone(),
                    count: s.entries.len(),
                    collapsed: is_collapsed,
                });
                if is_collapsed {
                    continue;
                }
                let w = s.entries.iter().map(|e| e.key.len()).max().unwrap_or(0);
                for e in &s.entries {
                    let mark = if e.privileged { " *" } else { "" };
                    rows.push(ReportRow::Entry(Line::from(vec![
                        Span::styled(format!("    {:<w$}  ", e.key), Style::new().fg(th.muted)),
                        Span::styled(format!("{}{mark}", e.value), value_style(&e.value)),
                    ])));
                }
            }
        }
    }
    rows
}

fn report(f: &mut Frame, app: &mut App, pane: usize, area: Rect, focused: bool) {
    let idx = app.pane_target(pane);
    let rows = report_rows(app, pane);
    let diff_label = app
        .diff_with
        .filter(|&o| o != idx)
        .map(|o| format!("diff against {} (D to leave) ", app.targets[o].name))
        .unwrap_or_else(|| "Enter fold · d diff ".into());
    let title = target_title(app, pane, "report", diff_label);
    let block = pane_block(app, title, focused);
    let height = area.height.saturating_sub(2) as usize;
    let th = app.theme;
    let p = &mut app.panes[pane];
    p.cursor = p.cursor.min(rows.len().saturating_sub(1));
    if p.cursor < p.scroll {
        p.scroll = p.cursor;
    } else if height > 0 && p.cursor >= p.scroll + height {
        p.scroll = p.cursor + 1 - height;
    }
    let (cursor, scroll) = (p.cursor, p.scroll);
    if rows.is_empty() {
        let msg = match &app.targets[idx].result {
            Some(_) => "no envprobe report for this target",
            None => "envprobe report not available yet",
        };
        f.render_widget(
            Paragraph::new(msg)
                .style(Style::new().fg(th.muted))
                .block(block),
            area,
        );
        return;
    }
    let lines: Vec<Line> = rows
        .into_iter()
        .enumerate()
        .skip(scroll)
        .take(height)
        .map(|(i, row)| {
            let line = match row {
                ReportRow::Section {
                    name,
                    count,
                    collapsed,
                } => Line::from(vec![
                    Span::styled(
                        format!("{} {name}", if collapsed { "▸" } else { "▾" }),
                        Style::new().fg(th.accent).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(format!(" ({count})"), Style::new().fg(th.muted)),
                ]),
                ReportRow::Entry(l) => l,
            };
            if i == cursor && focused {
                line.style(Style::new().bg(th.selection))
            } else {
                line
            }
        })
        .collect();
    f.render_widget(Paragraph::new(lines).block(block), area);
}

fn console(f: &mut Frame, app: &mut App, pane: usize, area: Rect, focused: bool) {
    let idx = app.pane_target(pane);
    let insert = focused && app.mode == Mode::Insert;
    let extra = if insert {
        "typing · Ctrl+] to leave ".to_string()
    } else if app.targets[idx].alive {
        "i to type ".to_string()
    } else {
        "VM stopped ".to_string()
    };
    let title = target_title(app, pane, "console", extra);
    let mut block = pane_block(app, title, focused);
    if insert {
        block = block.border_style(Style::new().fg(app.theme.warning));
    }
    let inner = block.inner(area);
    let t = &mut app.targets[idx];
    let (rows, cols) = t.term.screen().size();
    if (rows, cols) != (inner.height, inner.width) && inner.height > 0 && inner.width > 0 {
        t.term.screen_mut().set_size(inner.height, inner.width);
    }
    let term = PseudoTerminal::new(t.term.screen()).block(block);
    f.render_widget(term, area);
}

fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width.saturating_sub(2));
    let h = h.min(area.height.saturating_sub(2));
    Rect::new(
        area.x + (area.width - w) / 2,
        area.y + (area.height - h) / 2,
        w,
        h,
    )
}

fn palette(f: &mut Frame, app: &App, input: &str, selected: usize) {
    let th = &app.theme;
    let rect = centered(f.area(), 64, 16);
    f.render_widget(Clear, rect);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(th.accent))
        .title(" commands ");
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    let mut lines = vec![Line::from(vec![
        Span::styled(": ", Style::new().fg(th.accent)),
        Span::styled(input.to_string(), Style::new().fg(th.fg)),
        Span::styled("█", Style::new().fg(th.accent)),
    ])];
    let matches = palette_matches(app, input);
    let room = inner.height.saturating_sub(1) as usize;
    let first = selected.saturating_sub(room.saturating_sub(1));
    for (i, (cmd, help)) in matches.iter().enumerate().skip(first).take(room) {
        let style = if i == selected {
            Style::new()
                .fg(th.fg)
                .bg(th.selection)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(th.fg)
        };
        lines.push(Line::from(vec![
            Span::styled(format!(" {cmd:<24}"), style),
            Span::styled(format!(" {help}"), Style::new().fg(th.muted)),
        ]));
    }
    f.render_widget(Paragraph::new(lines), inner);
}

const HELP: &[(&str, &str)] = &[
    ("h j k l / arrows", "change target (dashboard) or scroll"),
    ("Tab, Ctrl+W w", "next pane"),
    ("Ctrl+W h/j/k/l", "focus in a direction"),
    ("Ctrl+W s / v", "split the pane (below / side by side)"),
    ("Ctrl+W q", "close the pane"),
    ("Ctrl+W < > + -", "resize"),
    ("1…9", "switch workspace"),
    ("D L R C", "view: dashboard, logs, report, console"),
    ("space", "pause / resume (QMP stop / cont)"),
    ("r / x", "restart / kill the target"),
    ("c, then i", "open the serial console / type in it"),
    ("Ctrl+]", "leave console input"),
    ("/  n  N", "search the logs, next, previous"),
    ("g / G", "top / bottom (follow)"),
    ("Enter", "fold a report section"),
    ("d / D", "diff against another target / leave the diff"),
    ("p", "pin the pane to the target"),
    ("f", "filter: all, running, failed, finished"),
    (": or Ctrl+P", "command palette"),
    ("mouse", "click: select and focus, wheel: scroll"),
    ("q", "quit (twice if VMs are running)"),
];

fn help(f: &mut Frame, app: &App) {
    let th = &app.theme;
    let rect = centered(f.area(), 70, HELP.len() as u16 + 4);
    f.render_widget(Clear, rect);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(th.accent))
        .title(" help · Esc to close ");
    let lines: Vec<Line> = HELP
        .iter()
        .map(|(k, v)| {
            Line::from(vec![
                Span::styled(format!(" {k:<20}"), Style::new().fg(th.accent)),
                Span::styled(v.to_string(), Style::new().fg(th.fg)),
            ])
        })
        .collect();
    f.render_widget(Paragraph::new(lines).block(block), rect);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn braille_graph() {
        let rows = braille(&[0.0, 100.0], 100.0, 1, 1);
        // Left column empty, right column full.
        assert_eq!(rows, vec!["⢸".to_string()]);
        let rows = braille(&[100.0, 100.0, 50.0, 50.0], 100.0, 2, 2);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0], "⣿⠀");
        assert_eq!(rows[1], "⣿⣿");
        // Fewer samples than room: right-aligned.
        assert_eq!(braille(&[100.0], 100.0, 2, 1), vec!["⠀⢸".to_string()]);
    }

    #[test]
    fn gauge_width_is_constant() {
        for r in [0.0, 0.13, 0.5, 0.99, 1.0, 2.0] {
            let w: usize = gauge(r, 10, Color::Red, Color::Gray)
                .iter()
                .map(|s| s.content.chars().count())
                .sum();
            assert_eq!(w, 10, "ratio {r}");
        }
    }
}
