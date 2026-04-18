
use std::sync::OnceLock;

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph},
    Frame,
};

use crate::{
    app::{App, ContextMenu, Mode, PromptKind},
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
            Constraint::Length(1), // footer: info/prompt + help hint
        ])
        .split(f.area());

    draw_header(f, app, chunks[0]);
    match app.mode {
        Mode::Normal => draw_tree(f, app, chunks[1]),
        Mode::Search => draw_search(f, app, chunks[1]),
    }
    draw_info(f, app, chunks[2]);
    if app.show_help {
        draw_help_overlay(f, app, f.area());
    }
    if app.menu.is_some() {
        draw_context_menu(f, app, f.area());
    } else {
        app.menu_rect = None;
    }
}

fn draw_context_menu(f: &mut Frame, app: &mut App, area: Rect) {
    let menu: ContextMenu = match &app.menu {
        Some(m) => m.clone(),
        None => return,
    };
    let max_label = menu
        .items
        .iter()
        .map(|i| i.label().chars().count())
        .max()
        .unwrap_or(10);
    let width = (max_label as u16 + 4).min(area.width);
    let height = (menu.items.len() as u16 + 2).min(area.height);
    let mut x = menu.anchor.0;
    let mut y = menu.anchor.1;
    if x + width > area.x + area.width {
        x = (area.x + area.width).saturating_sub(width);
    }
    if y + height > area.y + area.height {
        y = (area.y + area.height).saturating_sub(height);
    }
    let rect = Rect::new(x, y, width, height);

    f.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(ACCENT))
        .title(" menu ");
    let lines: Vec<Line> = menu
        .items
        .iter()
        .enumerate()
        .map(|(i, item)| {
            let style = if i == menu.selected {
                Style::default().bg(SELECTED_BG).add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            let pad = max_label.saturating_sub(item.label().chars().count());
            let text = format!(" {}{} ", item.label(), " ".repeat(pad));
            Line::from(Span::styled(text, style))
        })
        .collect();
    f.render_widget(Paragraph::new(lines).block(block), rect);
    app.menu_rect = Some(rect);
}

fn draw_header(f: &mut Frame, app: &App, area: Rect) {
    let mode_text = match app.mode {
        Mode::Normal => " NORMAL ",
        Mode::Search => " SEARCH ",
    };
    let mode_style = Style::default().bg(ACCENT).fg(Color::Black).add_modifier(Modifier::BOLD);
    let root = app.tree.root.display().to_string();
    let spans = vec![
        Span::styled(mode_text, mode_style),
        Span::raw(" "),
        Span::styled("kudzu", Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
        Span::raw(" · "),
        Span::raw(root),
    ];
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
            render_search_row(node, &m.parent_rel, row == selected, &m.indices)
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
    parent_rel: &str,
    selected: bool,
    highlight: &[u32],
) -> ListItem<'static> {
    let icon = if node.is_dir { "▶ " } else { "  " };
    let base_style = base_style_for(node);
    let mut spans = vec![Span::raw(icon)];
    spans.extend(highlighted_name(&node.name, highlight, base_style));
    if node.is_dir {
        spans.push(Span::styled("/", base_style));
    }
    if !parent_rel.is_empty() {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            format!("in {}", parent_rel),
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
    // indices are sorted ascending (nucleo guarantee); walk with a pointer
    // instead of building a HashSet.
    let hl = base.fg(MATCH_FG).add_modifier(Modifier::BOLD);
    let mut spans = Vec::new();
    let mut cur = String::new();
    let mut cur_hl = false;
    let mut idx_pos = 0usize;
    for (i, c) in name.chars().enumerate() {
        let is_hl = idx_pos < indices.len() && indices[idx_pos] as usize == i;
        if is_hl {
            idx_pos += 1;
        }
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
    if let Some(prompt) = &app.input {
        if prompt.kind == PromptKind::Delete {
            let name = prompt
                .target
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| prompt.target.display().to_string());
            let line = Line::from(vec![
                Span::styled(
                    format!("delete {}? ", name),
                    Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                ),
                Span::styled("(y/N)", Style::default().fg(HIDDEN_FG)),
            ]);
            f.render_widget(Paragraph::new(line), area);
            return;
        }
        let label = match prompt.kind {
            PromptKind::NewFile => "new file: ",
            PromptKind::NewFolder => "new folder: ",
            PromptKind::Rename => "rename: ",
            PromptKind::Delete => unreachable!(),
        };
        let before: String = prompt.buffer.chars().take(prompt.cursor).collect();
        let after: String = prompt.buffer.chars().skip(prompt.cursor).collect();
        let line = Line::from(vec![
            Span::styled(label, Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
            Span::raw(before),
            Span::styled("▏", Style::default().fg(ACCENT)),
            Span::raw(after),
        ]);
        f.render_widget(Paragraph::new(line), area);
        return;
    }
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
                format!(" {}", size)
            } else {
                String::new()
            };
            let left = Paragraph::new(Span::styled(info, Style::default().fg(Color::Gray)));

            let mut right_spans: Vec<Span> =
                vec![Span::styled("h help", Style::default().fg(HIDDEN_FG))];
            if !app.tree.opts.respect_gitignore {
                right_spans.push(Span::raw("  "));
                right_spans.push(Span::styled(
                    "[ignore off]",
                    Style::default().fg(Color::Yellow),
                ));
            }
            if app.tree.opts.show_hidden {
                right_spans.push(Span::raw("  "));
                right_spans.push(Span::styled("[hidden]", Style::default().fg(Color::Yellow)));
            }
            if !app.status.is_empty() {
                right_spans.push(Span::raw("  "));
                right_spans.push(Span::styled(
                    app.status.clone(),
                    Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
                ));
            }
            let right_line = Line::from(right_spans);
            let right_width = (right_line.width() as u16 + 2).min(area.width);

            let horiz = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Min(1), Constraint::Length(right_width)])
                .split(area);
            f.render_widget(left, horiz[0]);
            f.render_widget(
                Paragraph::new(right_line).alignment(ratatui::layout::Alignment::Right),
                horiz[1],
            );
        }
    }
}

struct HelpPage {
    title: &'static str,
    rows: &'static [(&'static str, &'static str)],
}

const HELP_PAGES: &[HelpPage] = &[
    HelpPage {
        title: "Navigate",
        rows: &[
            ("s / ↓", "move down"),
            ("w / ↑", "move up"),
            ("u / ←", "collapse · parent · ascend root"),
            ("l / → / Space", "expand"),
            ("f", "focus: descend root into selected dir"),
            ("g / Home", "top"),
            ("G / End", "bottom"),
            ("Ctrl-d / PgDn", "down 10"),
            ("Ctrl-u / PgUp", "up 10"),
        ],
    },
    HelpPage {
        title: "Open",
        rows: &[
            ("Enter", "expand dir or open file"),
            ("o", "open file in editor"),
            ("double-click", "open (editor or GUI, per config)"),
            ("M", "open in file manager"),
        ],
    },
    HelpPage {
        title: "File ops",
        rows: &[
            ("n", "new file in selected dir"),
            ("N", "new folder in selected dir"),
            ("R", "rename selected"),
            ("D", "delete selected (confirm y)"),
            ("right-click", "context menu"),
        ],
    },
    HelpPage {
        title: "View",
        rows: &[
            (".", "toggle hidden files"),
            ("i", "toggle .gitignore"),
            ("r", "rescan"),
            ("h", "toggle this help"),
            ("q / Ctrl-c", "quit"),
        ],
    },
    HelpPage {
        title: "Search",
        rows: &[
            ("/", "enter search"),
            ("type", "filter"),
            ("↑ / ↓", "select match"),
            ("Enter", "jump to (open if file)"),
            ("Backspace", "delete char"),
            ("Ctrl-w", "delete word"),
            ("Esc / Ctrl-c", "exit search"),
        ],
    },
];

fn draw_help_overlay(f: &mut Frame, app: &App, area: Rect) {
    let tab = app.help_tab.min(HELP_PAGES.len().saturating_sub(1));
    let page = &HELP_PAGES[tab];
    let rows = page.rows;

    // Build content lines
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
        " Tab: next page · any key: close ",
        Style::default().fg(HIDDEN_FG),
    )));

    // Build tab bar line
    let tab_bar: Line = {
        let mut spans = vec![Span::raw(" ")];
        for (i, p) in HELP_PAGES.iter().enumerate() {
            if i == tab {
                spans.push(Span::styled(
                    format!("[{}]", p.title),
                    Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
                ));
            } else {
                spans.push(Span::styled(
                    p.title.to_string(),
                    Style::default().fg(HIDDEN_FG),
                ));
            }
            if i + 1 < HELP_PAGES.len() {
                spans.push(Span::styled(" · ", Style::default().fg(HIDDEN_FG)));
            }
        }
        spans.push(Span::raw(" "));
        Line::from(spans)
    };

    // Compute popup dimensions once; HELP_PAGES is 'static so the result never changes.
    static MAX_CONTENT_WIDTH: OnceLock<u16> = OnceLock::new();
    let max_content_width = *MAX_CONTENT_WIDTH.get_or_init(|| {
        let key_w = HELP_PAGES
            .iter()
            .flat_map(|p| p.rows.iter())
            .map(|(k, _)| k.chars().count())
            .max()
            .unwrap_or(0);
        HELP_PAGES
            .iter()
            .flat_map(|p| p.rows.iter())
            .map(|(k, desc)| {
                let pad = key_w.saturating_sub(k.chars().count());
                (1 + k.chars().count() + pad + 2 + desc.chars().count()) as u16
            })
            .max()
            .unwrap_or(40)
    });

    let tab_bar_width = tab_bar.width() as u16;
    let inner_width = max_content_width.max(tab_bar_width) + 2;
    let inner_height = (1 + lines.len()) as u16 + 2; // tab bar + content lines + borders
    let width = inner_width.min(area.width.saturating_sub(2)).max(20);
    let height = inner_height.min(area.height.saturating_sub(2)).max(6);
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = area.y + area.height.saturating_sub(height) / 2;
    let popup = Rect::new(x, y, width, height);

    f.render_widget(Clear, popup);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(ACCENT))
        .title(" help ");

    // Render tab bar + content inside the block
    let inner = block.inner(popup);
    f.render_widget(block, popup);

    if inner.height == 0 {
        return;
    }

    // First line: tab bar
    let tab_area = Rect::new(inner.x, inner.y, inner.width, 1);
    f.render_widget(Paragraph::new(tab_bar), tab_area);

    // Remaining lines: key bindings
    if inner.height > 1 {
        let content_area = Rect::new(
            inner.x,
            inner.y + 1,
            inner.width,
            inner.height - 1,
        );
        f.render_widget(Paragraph::new(lines), content_area);
    }
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

