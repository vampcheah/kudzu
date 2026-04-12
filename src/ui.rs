use std::time::SystemTime;

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph},
    Frame,
};

use crate::{
    app::{App, Mode},
    tree::Node,
};

const DIR_FG: Color = Color::Cyan;
const HIDDEN_FG: Color = Color::DarkGray;
const SYMLINK_FG: Color = Color::Magenta;
const SELECTED_BG: Color = Color::Indexed(238);
const MATCH_FG: Color = Color::Yellow;
const ACCENT: Color = Color::Green;

pub fn draw(f: &mut Frame, app: &mut App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // header
            Constraint::Min(1),    // body (tree or search results)
            Constraint::Length(1), // input (search) or status
            Constraint::Length(1), // help hint
        ])
        .split(f.area());

    draw_header(f, app, chunks[0]);
    match app.mode {
        Mode::Normal => draw_tree(f, app, chunks[1]),
        Mode::Search => draw_search(f, app, chunks[1]),
    }
    draw_info(f, app, chunks[2]);
    let hint = Paragraph::new(Span::styled(
        "h help",
        Style::default().fg(HIDDEN_FG),
    ));
    f.render_widget(hint, chunks[3]);
    if app.show_help {
        draw_help_overlay(f, app, f.area());
    }
}

fn draw_header(f: &mut Frame, app: &App, area: Rect) {
    let mode_text = match app.mode {
        Mode::Normal => " NORMAL ",
        Mode::Search => " SEARCH ",
    };
    let mode_style = Style::default().bg(ACCENT).fg(Color::Black).add_modifier(Modifier::BOLD);
    let root = app.tree.root.display().to_string();
    let mut spans = vec![
        Span::styled(mode_text, mode_style),
        Span::raw(" "),
        Span::styled("kudzu", Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
        Span::raw(" · "),
        Span::raw(root),
    ];
    if !app.tree.opts.respect_gitignore {
        spans.push(Span::styled(
            "  [ignore off]",
            Style::default().fg(Color::Yellow),
        ));
    }
    if app.tree.opts.show_hidden {
        spans.push(Span::styled(
            "  [hidden]",
            Style::default().fg(Color::Yellow),
        ));
    }
    let p = Paragraph::new(Line::from(spans));
    f.render_widget(p, area);
}

fn draw_tree(f: &mut Frame, app: &mut App, area: Rect) {
    let inner_height = area.height.saturating_sub(2) as usize; // borders
    let height = inner_height.max(1);
    if app.selected < app.scroll {
        app.scroll = app.selected;
    } else if app.selected >= app.scroll + height {
        app.scroll = app.selected + 1 - height;
    }
    // Clamp scroll.
    let max_scroll = app.tree.visible.len().saturating_sub(height);
    if app.scroll > max_scroll {
        app.scroll = max_scroll;
    }

    let end = (app.scroll + height).min(app.tree.visible.len());
    let items: Vec<ListItem> = app.tree.visible[app.scroll..end]
        .iter()
        .enumerate()
        .map(|(offset, &idx)| {
            let row = app.scroll + offset;
            let node = &app.tree.nodes[idx];
            render_tree_row(node, row == app.selected, &[])
        })
        .collect();

    let title = format!(
        " tree · {} nodes · {} visible ",
        app.tree.nodes.len() - 1,
        app.tree.visible.len()
    );
    let list = List::new(items).block(Block::default().borders(Borders::ALL).title(title));
    let mut state = ListState::default();
    f.render_stateful_widget(list, area, &mut state);
    app.list_area = Some(inner_rect(area));
    app.list_scroll = app.scroll;
}

fn draw_search(f: &mut Frame, app: &mut App, area: Rect) {
    let inner_height = area.height.saturating_sub(2) as usize;
    let height = inner_height.max(1);

    // Scroll follows selection within matches.
    // We don't persist scroll across mode changes; fine for search.
    let selected = app.search.selected;
    let scroll = selected.saturating_sub(height / 2);
    let total = app.search.matches.len();
    let end = (scroll + height).min(total);

    let items: Vec<ListItem> = app.search.matches[scroll..end]
        .iter()
        .enumerate()
        .map(|(offset, m)| {
            let row = scroll + offset;
            let node = &app.tree.nodes[m.node];
            render_search_row(node, &app.tree.root, row == selected, &m.indices)
        })
        .collect();

    let title = format!(
        " matches · {} / {} ",
        total,
        app.tree.nodes.len().saturating_sub(1)
    );
    let list = List::new(items).block(Block::default().borders(Borders::ALL).title(title));
    let mut state = ListState::default();
    f.render_stateful_widget(list, area, &mut state);
    app.list_area = Some(inner_rect(area));
    app.list_scroll = scroll;
}

fn inner_rect(area: Rect) -> Rect {
    // Borders::ALL trims 1 cell on each side.
    if area.width < 2 || area.height < 2 {
        return Rect::new(area.x, area.y, 0, 0);
    }
    Rect::new(area.x + 1, area.y + 1, area.width - 2, area.height - 2)
}

fn render_tree_row(node: &Node, selected: bool, highlight: &[u32]) -> ListItem<'static> {
    let indent = "  ".repeat(node.depth);
    let icon = if node.is_dir {
        if node.expanded {
            "▼ "
        } else {
            "▶ "
        }
    } else {
        "  "
    };
    let base_style = base_style_for(node);
    let name_spans = highlighted_name(&node.name, highlight, base_style);
    let mut spans = vec![Span::raw(indent), Span::raw(icon)];
    spans.extend(name_spans);
    if node.is_dir {
        spans.push(Span::styled("/", base_style));
    }
    if node.is_symlink {
        spans.push(Span::styled(" →", Style::default().fg(SYMLINK_FG)));
    }
    let line = Line::from(spans);
    let item_style = if selected {
        Style::default().bg(SELECTED_BG).add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    ListItem::new(line).style(item_style)
}

fn render_search_row(
    node: &Node,
    root: &std::path::Path,
    selected: bool,
    highlight: &[u32],
) -> ListItem<'static> {
    let icon = if node.is_dir { "▶ " } else { "  " };
    let base_style = base_style_for(node);
    let rel = node
        .path
        .strip_prefix(root)
        .ok()
        .and_then(|p| p.parent())
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut spans = vec![Span::raw(icon)];
    spans.extend(highlighted_name(&node.name, highlight, base_style));
    if node.is_dir {
        spans.push(Span::styled("/", base_style));
    }
    if !rel.is_empty() {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            format!("in {}", rel),
            Style::default().fg(HIDDEN_FG),
        ));
    }
    let item_style = if selected {
        Style::default().bg(SELECTED_BG).add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    ListItem::new(Line::from(spans)).style(item_style)
}

fn base_style_for(node: &Node) -> Style {
    let mut s = Style::default();
    if node.is_symlink {
        s = s.fg(SYMLINK_FG);
    } else if node.is_dir {
        s = s.fg(DIR_FG).add_modifier(Modifier::BOLD);
    } else if node.is_hidden {
        s = s.fg(HIDDEN_FG);
    }
    s
}

fn highlighted_name(name: &str, indices: &[u32], base: Style) -> Vec<Span<'static>> {
    if indices.is_empty() {
        return vec![Span::styled(name.to_string(), base)];
    }
    let mut set = std::collections::HashSet::new();
    for &i in indices {
        set.insert(i as usize);
    }
    let hl = base.fg(MATCH_FG).add_modifier(Modifier::BOLD);
    let mut spans = Vec::new();
    let mut cur = String::new();
    let mut cur_hl = false;
    for (i, c) in name.chars().enumerate() {
        let is_hl = set.contains(&i);
        if is_hl != cur_hl && !cur.is_empty() {
            spans.push(Span::styled(
                std::mem::take(&mut cur),
                if cur_hl { hl } else { base },
            ));
        }
        cur_hl = is_hl;
        cur.push(c);
    }
    if !cur.is_empty() {
        spans.push(Span::styled(cur, if cur_hl { hl } else { base }));
    }
    spans
}

fn draw_info(f: &mut Frame, app: &App, area: Rect) {
    match app.mode {
        Mode::Search => {
            let line = Line::from(vec![
                Span::styled("/ ", Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
                Span::raw(app.search.query.clone()),
                Span::styled("▏", Style::default().fg(ACCENT)),
            ]);
            f.render_widget(Paragraph::new(line), area);
        }
        Mode::Normal => {
            let info = if let Some(&idx) = app.tree.visible.get(app.selected) {
                let n = &app.tree.nodes[idx];
                let size = if n.is_dir {
                    "-".to_string()
                } else {
                    human_size(n.size)
                };
                let mtime = n.mtime.map(format_time).unwrap_or_default();
                format!(
                    " {}/{}  {}  {}  {}",
                    app.selected + 1,
                    app.tree.visible.len(),
                    size,
                    mtime,
                    n.path.display()
                )
            } else {
                String::new()
            };
            let left = Paragraph::new(Span::styled(info, Style::default().fg(Color::Gray)));
            let flash = if !app.status.is_empty() {
                Span::styled(
                    format!("  {}", app.status),
                    Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
                )
            } else {
                Span::raw("")
            };
            let horiz = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Min(1), Constraint::Length(40)])
                .split(area);
            f.render_widget(left, horiz[0]);
            f.render_widget(Paragraph::new(Line::from(flash)), horiz[1]);
        }
    }
}

fn draw_help_overlay(f: &mut Frame, app: &App, area: Rect) {
    let normal_rows: &[(&str, &str)] = &[
        ("s / ↓", "move down"),
        ("w / ↑", "move up"),
        ("u / ←", "collapse · parent · ascend root"),
        ("l / → / Space", "expand"),
        ("f", "focus: descend root into selected dir"),
        ("Enter", "expand dir or open file in editor"),
        ("o", "open file in editor"),
        ("double-click", "open (editor or GUI, per config)"),
        ("g / Home", "top"),
        ("G / End", "bottom"),
        ("Ctrl-d / PgDn", "down 10"),
        ("Ctrl-u / PgUp", "up 10"),
        ("/", "search"),
        (".", "toggle hidden files"),
        ("i", "toggle .gitignore"),
        ("r", "rescan"),
        ("h", "toggle this help"),
        ("q / Ctrl-c", "quit"),
    ];
    let search_rows: &[(&str, &str)] = &[
        ("type", "filter"),
        ("↑ / ↓", "select match"),
        ("Enter", "jump to (open if file)"),
        ("Backspace", "delete char"),
        ("Ctrl-w", "delete word"),
        ("Esc / Ctrl-c", "exit search"),
    ];

    let rows = if app.mode == Mode::Search {
        search_rows
    } else {
        normal_rows
    };

    let key_col = rows.iter().map(|(k, _)| k.chars().count()).max().unwrap_or(0);
    let mut lines: Vec<Line> = rows
        .iter()
        .map(|(k, desc)| {
            let pad = " ".repeat(key_col.saturating_sub(k.chars().count()));
            Line::from(vec![
                Span::styled(
                    format!(" {}{}", k, pad),
                    Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
                ),
                Span::raw("  "),
                Span::raw(desc.to_string()),
            ])
        })
        .collect();
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        " press any key to close ",
        Style::default().fg(HIDDEN_FG),
    )));

    let inner_width = lines
        .iter()
        .map(|l| l.width() as u16)
        .max()
        .unwrap_or(40)
        + 2;
    let inner_height = lines.len() as u16 + 2;
    let width = inner_width.min(area.width.saturating_sub(2)).max(20);
    let height = inner_height.min(area.height.saturating_sub(2)).max(6);
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = area.y + area.height.saturating_sub(height) / 2;
    let popup = Rect::new(x, y, width, height);

    f.render_widget(Clear, popup);
    let title = if app.mode == Mode::Search {
        " help · search mode "
    } else {
        " help "
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(ACCENT))
        .title(title);
    f.render_widget(Paragraph::new(lines).block(block), popup);
}

fn human_size(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "K", "M", "G", "T"];
    let mut v = bytes as f64;
    let mut unit = 0;
    while v >= 1024.0 && unit + 1 < UNITS.len() {
        v /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{}{}", bytes, UNITS[0])
    } else {
        format!("{:.1}{}", v, UNITS[unit])
    }
}

fn format_time(t: SystemTime) -> String {
    match t.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(d) => {
            let secs = d.as_secs();
            let (y, mo, da, h, mi) = epoch_to_ymdhm(secs);
            format!("{:04}-{:02}-{:02} {:02}:{:02}", y, mo, da, h, mi)
        }
        Err(_) => String::new(),
    }
}

/// Minimal UTC epoch → (year, month, day, hour, minute). Local-tz
/// conversion isn't worth a dep here; times display as UTC.
fn epoch_to_ymdhm(secs: u64) -> (i32, u32, u32, u32, u32) {
    let days = (secs / 86400) as i64;
    let sod = secs % 86400;
    let h = (sod / 3600) as u32;
    let mi = ((sod % 3600) / 60) as u32;
    // Howard Hinnant's civil_from_days algorithm.
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = if m <= 2 { y + 1 } else { y } as i32;
    (y, m, d, h, mi)
}
