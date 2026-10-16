//! Clavier et souris : navigation modale façon vim, palette de commandes.

use ratatui::crossterm::event::{
    Event as TermEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent,
    MouseEventKind,
};
use ratatui::layout::Position;

use super::layout::{neighbor, Dir, Node};
use super::render::{report_rows, ReportRow};
use super::state::{App, Filter, Mode, Phase, View, Workspace};
use crate::event::Control;

/// Commandes de la palette : (commande, description).
pub fn palette_commands(app: &App) -> Vec<(String, String)> {
    let mut cmds: Vec<(String, String)> = [
        ("pause", "met la cible en pause (QMP stop)"),
        ("resume", "reprend la cible (QMP cont)"),
        ("restart", "relance la cible"),
        ("kill", "tue la cible"),
        ("restart failed", "relance toutes les cibles en échec"),
        ("kill all", "tue toutes les VM"),
        ("console", "panneau courant : console série"),
        ("logs", "panneau courant : logs"),
        ("report", "panneau courant : rapport envprobe"),
        ("dashboard", "panneau courant : dashboard"),
        ("split", "découpe le panneau (dessous)"),
        ("vsplit", "découpe le panneau (côte à côte)"),
        ("close", "ferme le panneau"),
        ("pin", "épingle le panneau sur la cible"),
        ("nodiff", "quitte le mode diff"),
        ("filter all", "toutes les cibles"),
        ("filter running", "cibles en cours"),
        ("filter failed", "cibles en échec"),
        ("filter done", "cibles terminées"),
        ("ws new", "nouveau workspace"),
        ("help", "aide"),
        ("quit", "quitter"),
    ]
    .iter()
    .map(|(c, h)| (c.to_string(), h.to_string()))
    .collect();
    for t in &app.targets {
        cmds.push((
            format!("diff {}", t.name),
            "diff du rapport contre cette cible".into(),
        ));
        cmds.push((format!("select {}", t.name), "sélectionne la cible".into()));
    }
    for (i, ws) in app.workspaces.iter().enumerate() {
        cmds.push((
            format!("ws {}", i + 1),
            format!("workspace « {} »", ws.name),
        ));
    }
    cmds
}

/// Correspondance floue : les caractères de `query` apparaissent dans l'ordre.
fn fuzzy(query: &str, text: &str) -> Option<usize> {
    let mut score = 0;
    let mut last = 0;
    let mut chars = text.char_indices();
    for q in query.chars() {
        let (i, _) = chars.by_ref().find(|(_, c)| c.eq_ignore_ascii_case(&q))?;
        score += i - last;
        last = i;
    }
    Some(score)
}

pub fn palette_matches(app: &App, input: &str) -> Vec<(String, String)> {
    let mut m: Vec<(usize, (String, String))> = palette_commands(app)
        .into_iter()
        .filter_map(|c| fuzzy(input.trim(), &c.0).map(|s| (s, c)))
        .collect();
    m.sort_by_key(|(s, c)| (*s, c.0.len()));
    m.into_iter().map(|(_, c)| c).collect()
}

fn target_by_name(app: &App, name: &str) -> Option<usize> {
    app.targets.iter().position(|t| t.name == name)
}

/// Exécute une commande textuelle (palette ou `:` libre).
pub fn execute(app: &mut App, cmd: &str) {
    let words: Vec<&str> = cmd.split_whitespace().collect();
    let sel = app.selected;
    match words.as_slice() {
        [] => {}
        ["pause"] => app.send(sel, Control::Pause),
        ["resume" | "cont"] => app.send(sel, Control::Resume),
        ["restart"] => app.send(sel, Control::Restart),
        ["kill"] => app.send(sel, Control::Kill),
        ["kill", "all"] => {
            for i in 0..app.targets.len() {
                if app.targets[i].alive || app.targets[i].phase == Phase::Queued {
                    app.send(i, Control::Kill);
                }
            }
        }
        ["restart", "failed"] => {
            for i in 0..app.targets.len() {
                if Filter::Failed.accepts(&app.targets[i]) {
                    app.send(i, Control::Restart);
                }
            }
        }
        ["console"] => set_view(app, View::Console),
        ["logs"] => set_view(app, View::Logs),
        ["report" | "rapport"] => set_view(app, View::Report),
        ["dashboard"] => set_view(app, View::Dashboard),
        ["split"] => split(app, false),
        ["vsplit"] => split(app, true),
        ["close"] => close(app),
        ["pin"] => toggle_pin(app),
        ["nodiff"] => app.diff_with = None,
        ["diff", name] => match target_by_name(app, name) {
            Some(i) => {
                app.diff_with = Some(i);
                if app.panes[app.focused_pane()].view != View::Report {
                    set_view(app, View::Report);
                }
            }
            None => app.notify(format!("cible inconnue : {name}")),
        },
        ["select", name] => match target_by_name(app, name) {
            Some(i) => app.selected = i,
            None => app.notify(format!("cible inconnue : {name}")),
        },
        ["filter", f] => match Filter::parse(f) {
            Some(f) => {
                app.filter = f;
                app.fix_selection();
            }
            None => app.notify(format!("filtre inconnu : {f}")),
        },
        ["ws", "new"] => {
            let pane = app.add_pane(View::Dashboard);
            app.workspaces.push(Workspace {
                name: format!("ws{}", app.workspaces.len() + 1),
                group: None,
                root: Node::Leaf(pane),
                focus: pane,
            });
            app.ws = app.workspaces.len() - 1;
        }
        ["ws", n] => match n.parse::<usize>() {
            Ok(n) if n >= 1 && n <= app.workspaces.len() => app.ws = n - 1,
            _ => app.notify(format!("workspace inconnu : {n}")),
        },
        ["help"] => app.mode = Mode::Help,
        ["q" | "quit"] => request_quit(app),
        ["q!" | "quit!"] => app.quit = true,
        _ => app.notify(format!("commande inconnue : {cmd}")),
    }
}

fn set_view(app: &mut App, view: View) {
    let pane = app.focused_pane();
    app.panes[pane].view = view;
    app.panes[pane].scroll = 0;
    app.panes[pane].cursor = 0;
}

fn split(app: &mut App, side_by_side: bool) {
    let focus = app.focused_pane();
    let view = app.panes[focus].view;
    let new = app.add_pane(if view == View::Dashboard {
        View::Logs
    } else {
        view
    });
    app.workspace_mut()
        .root
        .split_leaf(focus, new, side_by_side);
    app.workspace_mut().focus = new;
}

fn close(app: &mut App) {
    let focus = app.focused_pane();
    let ws = app.workspace_mut();
    if ws.root.close(focus) {
        ws.focus = ws.root.leaves()[0];
    } else {
        app.notify("dernier panneau du workspace");
    }
}

fn toggle_pin(app: &mut App) {
    let pane = app.focused_pane();
    let p = &mut app.panes[pane];
    p.pinned = match p.pinned {
        Some(_) => None,
        None => Some(app.selected),
    };
}

fn request_quit(app: &mut App) {
    if app.running_count() == 0 || app.quit_armed {
        app.quit = true;
    } else {
        app.quit_armed = true;
        app.notify(format!(
            "{} VM en cours : q encore pour tout arrêter et quitter",
            app.running_count()
        ));
    }
}

fn cycle_focus(app: &mut App, forward: bool) {
    let leaves = app.workspace().root.leaves();
    let pos = leaves
        .iter()
        .position(|&p| p == app.focused_pane())
        .unwrap_or(0);
    let next = if forward {
        (pos + 1) % leaves.len()
    } else {
        (pos + leaves.len() - 1) % leaves.len()
    };
    app.workspace_mut().focus = leaves[next];
}

fn focus_dir(app: &mut App, dir: Dir) {
    if let Some(p) = neighbor(&app.pane_rects, app.focused_pane(), dir) {
        app.workspace_mut().focus = p;
    }
}

/// Octets envoyés à la console série pour une touche.
pub fn key_bytes(key: &KeyEvent) -> Vec<u8> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Char(c) if ctrl && c.is_ascii_alphabetic() => {
            vec![(c.to_ascii_lowercase() as u8) & 0x1f]
        }
        KeyCode::Char(c) => c.to_string().into_bytes(),
        KeyCode::Enter => vec![b'\r'],
        KeyCode::Backspace => vec![0x7f],
        KeyCode::Tab => vec![b'\t'],
        KeyCode::Esc => vec![0x1b],
        KeyCode::Up => b"\x1b[A".to_vec(),
        KeyCode::Down => b"\x1b[B".to_vec(),
        KeyCode::Right => b"\x1b[C".to_vec(),
        KeyCode::Left => b"\x1b[D".to_vec(),
        KeyCode::Home => b"\x1b[H".to_vec(),
        KeyCode::End => b"\x1b[F".to_vec(),
        KeyCode::Delete => b"\x1b[3~".to_vec(),
        KeyCode::PageUp => b"\x1b[5~".to_vec(),
        KeyCode::PageDown => b"\x1b[6~".to_vec(),
        _ => Vec::new(),
    }
}

pub fn handle(app: &mut App, ev: TermEvent) {
    match ev {
        TermEvent::Key(key) if key.kind != KeyEventKind::Release => handle_key(app, key),
        TermEvent::Mouse(m) => handle_mouse(app, m),
        _ => {}
    }
}

fn handle_key(app: &mut App, key: KeyEvent) {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    if !matches!(key.code, KeyCode::Char('q')) {
        app.quit_armed = false;
    }
    match app.mode.clone() {
        Mode::Insert => {
            // Ctrl+] arrive en 0x1D, que crossterm décode en Ctrl+5.
            if ctrl && matches!(key.code, KeyCode::Char(']') | KeyCode::Char('5')) {
                app.mode = Mode::Normal;
                return;
            }
            let pane = app.focused_pane();
            let target = app.pane_target(pane);
            let bytes = key_bytes(&key);
            if !bytes.is_empty() {
                app.send(target, Control::ConsoleInput(bytes));
            }
        }
        Mode::Help => {
            if matches!(
                key.code,
                KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('?')
            ) {
                app.mode = Mode::Normal;
            }
        }
        Mode::Search(mut s) => match key.code {
            KeyCode::Esc => {
                app.mode = Mode::Normal;
                app.search = None;
            }
            KeyCode::Enter => {
                app.mode = Mode::Normal;
                app.search = Some(s).filter(|s| !s.is_empty());
                jump_match(app, true);
            }
            KeyCode::Backspace => {
                s.pop();
                app.search = Some(s.clone());
                app.mode = Mode::Search(s);
            }
            KeyCode::Char(c) => {
                s.push(c);
                app.search = Some(s.clone());
                app.mode = Mode::Search(s);
            }
            _ => {}
        },
        Mode::Palette {
            mut input,
            mut selected,
        } => match key.code {
            KeyCode::Esc => app.mode = Mode::Normal,
            KeyCode::Enter => {
                let matches = palette_matches(app, &input);
                app.mode = Mode::Normal;
                // Une commande avec arguments tapée en entier passe telle quelle.
                let exact = palette_commands(app)
                    .iter()
                    .any(|(c, _)| *c == input.trim());
                let cmd = if exact || matches.is_empty() {
                    input
                } else {
                    matches[selected.min(matches.len() - 1)].0.clone()
                };
                execute(app, &cmd);
            }
            KeyCode::Up => {
                selected = selected.saturating_sub(1);
                app.mode = Mode::Palette { input, selected };
            }
            KeyCode::Down | KeyCode::Tab => {
                selected += 1;
                let n = palette_matches(app, &input).len();
                app.mode = Mode::Palette {
                    input,
                    selected: selected.min(n.saturating_sub(1)),
                };
            }
            KeyCode::Char('n') if ctrl => {
                let n = palette_matches(app, &input).len();
                app.mode = Mode::Palette {
                    input,
                    selected: (selected + 1).min(n.saturating_sub(1)),
                };
            }
            KeyCode::Char('p') if ctrl => {
                app.mode = Mode::Palette {
                    input,
                    selected: selected.saturating_sub(1),
                };
            }
            KeyCode::Backspace => {
                input.pop();
                app.mode = Mode::Palette { input, selected: 0 };
            }
            KeyCode::Char(c) => {
                input.push(c);
                app.mode = Mode::Palette { input, selected: 0 };
            }
            _ => {}
        },
        Mode::Window => {
            app.mode = Mode::Normal;
            match key.code {
                KeyCode::Char('s') => split(app, false),
                KeyCode::Char('v') => split(app, true),
                KeyCode::Char('q') | KeyCode::Char('c') => close(app),
                KeyCode::Char('w') => cycle_focus(app, true),
                KeyCode::Char('h') | KeyCode::Left => focus_dir(app, Dir::Left),
                KeyCode::Char('l') | KeyCode::Right => focus_dir(app, Dir::Right),
                KeyCode::Char('k') | KeyCode::Up => focus_dir(app, Dir::Up),
                KeyCode::Char('j') | KeyCode::Down => focus_dir(app, Dir::Down),
                KeyCode::Char(c @ ('<' | '>' | '+' | '-')) => {
                    let focus = app.focused_pane();
                    let (delta, horizontal) = match c {
                        '>' => (5, true),
                        '<' => (-5, true),
                        '+' => (5, false),
                        _ => (-5, false),
                    };
                    app.workspace_mut().root.resize(focus, delta, horizontal);
                }
                _ => {}
            }
        }
        Mode::Normal => normal_key(app, key),
    }
}

fn normal_key(app: &mut App, key: KeyEvent) {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let pane = app.focused_pane();
    let view = app.panes[pane].view;
    match key.code {
        KeyCode::Char('w') if ctrl => app.mode = Mode::Window,
        KeyCode::Char('p') if ctrl => {
            app.mode = Mode::Palette {
                input: String::new(),
                selected: 0,
            }
        }
        KeyCode::Char('c') if ctrl => request_quit(app),
        KeyCode::Char('d') if ctrl => scroll(app, pane, -10),
        KeyCode::Char('u') if ctrl => scroll(app, pane, 10),
        KeyCode::Char(':') => {
            app.mode = Mode::Palette {
                input: String::new(),
                selected: 0,
            }
        }
        KeyCode::Char('?') => app.mode = Mode::Help,
        KeyCode::Char('q') => request_quit(app),
        KeyCode::Tab => cycle_focus(app, true),
        KeyCode::BackTab => cycle_focus(app, false),
        KeyCode::Char(c @ '1'..='9') => {
            let n = c as usize - '1' as usize;
            if n < app.workspaces.len() {
                app.ws = n;
                app.fix_selection();
            }
        }
        KeyCode::Char('D') if view == View::Report && app.diff_with.is_some() => {
            app.diff_with = None
        }
        KeyCode::Char('D') => set_view(app, View::Dashboard),
        KeyCode::Char('L') => set_view(app, View::Logs),
        KeyCode::Char('R') => set_view(app, View::Report),
        KeyCode::Char('C') => set_view(app, View::Console),
        KeyCode::Char('f') => {
            app.filter = app.filter.next();
            app.fix_selection();
        }
        KeyCode::Char(' ') => {
            let t = app.selected;
            let ctrl = if app.targets[t].paused {
                Control::Resume
            } else {
                Control::Pause
            };
            app.send(t, ctrl);
        }
        KeyCode::Char('r') => app.send(app.selected, Control::Restart),
        KeyCode::Char('x') => app.send(app.selected, Control::Kill),
        KeyCode::Char('p') => toggle_pin(app),
        KeyCode::Char('c') => {
            // Ouvre la console de la cible dans le panneau courant (ou dans
            // un panneau console déjà visible).
            let existing = app
                .workspace()
                .root
                .leaves()
                .into_iter()
                .find(|&p| app.panes[p].view == View::Console);
            match existing {
                Some(p) => app.workspace_mut().focus = p,
                None if view == View::Dashboard => split(app, true),
                None => {}
            }
            set_view(app, View::Console);
        }
        KeyCode::Char('i') if view == View::Console => {
            if app.targets[app.pane_target(pane)].alive {
                app.mode = Mode::Insert;
            } else {
                app.notify("VM arrêtée : relancez-la (r), avec --hold pour garder la console");
            }
        }
        KeyCode::Char('/') => {
            app.mode = Mode::Search(String::new());
            app.search = Some(String::new());
        }
        KeyCode::Char('n') => jump_match(app, true),
        KeyCode::Char('N') => jump_match(app, false),
        KeyCode::Esc => app.search = None,
        KeyCode::Char('g') => match view {
            View::Logs => app.panes[pane].scroll = usize::MAX,
            View::Report => app.panes[pane].cursor = 0,
            _ => {}
        },
        KeyCode::Char('G') => match view {
            View::Logs => app.panes[pane].scroll = 0,
            View::Report => app.panes[pane].cursor = usize::MAX,
            _ => {}
        },
        KeyCode::Char('d') if view == View::Report => {
            // Diff contre la cible suivante qui a un rapport.
            let me = app.pane_target(pane);
            let n = app.targets.len();
            let start = app.diff_with.unwrap_or(me);
            app.diff_with = (1..=n).map(|k| (start + k) % n).find(|&i| {
                i != me
                    && app.targets[i]
                        .result
                        .as_ref()
                        .is_some_and(|r| r.envprobe.is_some())
            });
            if app.diff_with.is_none() {
                app.notify("aucune autre cible n'a de rapport envprobe");
            }
        }
        KeyCode::Enter if view == View::Report => toggle_section(app, pane),
        KeyCode::Char('h') | KeyCode::Left => match view {
            View::Dashboard => app.move_selection(-1),
            View::Report => collapse_at_cursor(app, pane, true),
            _ => app.move_selection(-1),
        },
        KeyCode::Char('l') | KeyCode::Right => match view {
            View::Dashboard => app.move_selection(1),
            View::Report => collapse_at_cursor(app, pane, false),
            _ => app.move_selection(1),
        },
        KeyCode::Char('j') | KeyCode::Down => match view {
            View::Dashboard => app.move_selection(app.dashboard_cols as isize),
            View::Logs => scroll(app, pane, -1),
            View::Report => app.panes[pane].cursor = app.panes[pane].cursor.saturating_add(1),
            View::Console => app.move_selection(1),
        },
        KeyCode::Char('k') | KeyCode::Up => match view {
            View::Dashboard => app.move_selection(-(app.dashboard_cols as isize)),
            View::Logs => scroll(app, pane, 1),
            View::Report => app.panes[pane].cursor = app.panes[pane].cursor.saturating_sub(1),
            View::Console => app.move_selection(-1),
        },
        KeyCode::PageUp => scroll(app, pane, 20),
        KeyCode::PageDown => scroll(app, pane, -20),
        _ => {}
    }
}

/// Défilement : positif vers le haut (logs : depuis le bas).
fn scroll(app: &mut App, pane: usize, delta: isize) {
    let p = &mut app.panes[pane];
    match p.view {
        View::Logs => {
            p.scroll = if delta >= 0 {
                p.scroll.saturating_add(delta as usize)
            } else {
                p.scroll.saturating_sub(delta.unsigned_abs())
            }
        }
        View::Report => {
            p.cursor = if delta >= 0 {
                p.cursor.saturating_sub(delta as usize)
            } else {
                p.cursor.saturating_add(delta.unsigned_abs())
            }
        }
        _ => {}
    }
}

fn section_at_cursor(app: &App, pane: usize) -> Option<String> {
    let rows = report_rows(app, pane);
    let cursor = app.panes[pane].cursor.min(rows.len().saturating_sub(1));
    rows[..=cursor.min(rows.len().saturating_sub(1))]
        .iter()
        .rev()
        .find_map(|r| match r {
            ReportRow::Section { name, .. } => Some(name.clone()),
            ReportRow::Entry(_) => None,
        })
}

fn toggle_section(app: &mut App, pane: usize) {
    if let Some(name) = section_at_cursor(app, pane) {
        let set = &mut app.panes[pane].collapsed;
        if !set.remove(&name) {
            set.insert(name);
        }
    }
}

fn collapse_at_cursor(app: &mut App, pane: usize, collapse: bool) {
    if let Some(name) = section_at_cursor(app, pane) {
        if collapse {
            app.panes[pane].collapsed.insert(name.clone());
            // Le curseur revient sur l'en-tête replié.
            let rows = report_rows(app, pane);
            if let Some(i) = rows
                .iter()
                .position(|r| matches!(r, ReportRow::Section { name: n, .. } if *n == name))
            {
                app.panes[pane].cursor = i;
            }
        } else {
            app.panes[pane].collapsed.remove(&name);
        }
    }
}

/// Fait défiler le panneau logs jusqu'à la correspondance suivante.
fn jump_match(app: &mut App, forward: bool) {
    let Some(needle) = app.search.clone().filter(|s| !s.is_empty()) else {
        return;
    };
    let needle = needle.to_lowercase();
    let pane = app
        .workspace()
        .root
        .leaves()
        .into_iter()
        .find(|&p| p == app.focused_pane() && app.panes[p].view == View::Logs)
        .or_else(|| {
            app.workspace()
                .root
                .leaves()
                .into_iter()
                .find(|&p| app.panes[p].view == View::Logs)
        });
    let Some(pane) = pane else {
        return;
    };
    let target = app.pane_target(pane);
    let logs = &app.targets[target].logs;
    let total = logs.len();
    let height = app
        .pane_rects
        .iter()
        .find(|(p, _)| *p == pane)
        .map_or(10, |(_, r)| r.height.saturating_sub(2) as usize);
    // Ligne actuellement en bas du panneau.
    let bottom = total.saturating_sub(app.panes[pane].scroll + 1);
    let found = if forward {
        (bottom + 1..total)
            .chain(0..=bottom.min(total.saturating_sub(1)))
            .find(|&i| logs[i].text.to_lowercase().contains(&needle))
    } else {
        (0..bottom)
            .rev()
            .chain((bottom..total).rev())
            .find(|&i| logs[i].text.to_lowercase().contains(&needle))
    };
    match found {
        Some(i) => {
            // Correspondance au milieu du panneau.
            let bottom_line = (i + height / 2).min(total.saturating_sub(1));
            app.panes[pane].scroll = total.saturating_sub(bottom_line + 1);
        }
        None => app.notify(format!("« {needle} » introuvable")),
    }
}

fn handle_mouse(app: &mut App, m: MouseEvent) {
    let pos = Position::new(m.column, m.row);
    let pane = app
        .pane_rects
        .iter()
        .find(|(_, r)| r.contains(pos))
        .map(|(p, _)| *p);
    match m.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            if let Some(p) = pane {
                app.workspace_mut().focus = p;
            }
            if let Some((_, idx)) = app.card_rects.iter().find(|(r, _)| r.contains(pos)) {
                app.selected = *idx;
            }
        }
        MouseEventKind::ScrollUp => {
            if let Some(p) = pane {
                match app.panes[p].view {
                    View::Dashboard => app.move_selection(-(app.dashboard_cols as isize)),
                    _ => scroll(app, p, 3),
                }
            }
        }
        MouseEventKind::ScrollDown => {
            if let Some(p) = pane {
                match app.panes[p].view {
                    View::Dashboard => app.move_selection(app.dashboard_cols as isize),
                    _ => scroll(app, p, -3),
                }
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzzy_matching() {
        assert_eq!(fuzzy("", "pause"), Some(0));
        assert!(fuzzy("rst", "restart").is_some());
        assert!(fuzzy("xyz", "restart").is_none());
        assert!(fuzzy("rs", "restart").unwrap() < fuzzy("rs", "report status").unwrap_or(99));
    }

    #[test]
    fn console_key_bytes() {
        let k = |code, m| KeyEvent::new(code, m);
        assert_eq!(
            key_bytes(&k(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            vec![3]
        );
        assert_eq!(key_bytes(&k(KeyCode::Enter, KeyModifiers::NONE)), b"\r");
        assert_eq!(key_bytes(&k(KeyCode::Up, KeyModifiers::NONE)), b"\x1b[A");
        assert_eq!(
            key_bytes(&k(KeyCode::Char('é'), KeyModifiers::NONE)),
            "é".as_bytes()
        );
    }
}
