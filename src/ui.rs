use std::sync::OnceLock;

static INDENT_CACHE: OnceLock<Vec<&'static str>> = OnceLock::new();

fn get_indent(depth: usize) -> &'static str {
    let cache = INDENT_CACHE.get_or_init(|| {
        (0..64usize)
            .map(|n| -> &'static str { Box::leak("  ".repeat(n).into_boxed_str()) })
            .collect()
    });
    cache.get(depth.min(63)).copied().unwrap_or("")
}

use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph},
};

use crate::{
    app::{App, Mode, PromptKind},
    search::SearchMatch,
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
    let menu = match app.menu.as_ref() {
        Some(m) => m,
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
                Style::default()
                    .bg(SELECTED_BG)
                    .add_modifier(Modifier::BOLD)
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
    let mode_style = Style::default()
        .bg(ACCENT)
        .fg(Color::Black)
        .add_modifier(Modifier::BOLD);
    let root_cow = app
        .tree
        .root
        .to_str()
        .map(std::borrow::Cow::Borrowed)
        .unwrap_or_else(|| std::borrow::Cow::Owned(app.tree.root.display().to_string()));
    let spans = vec![
        Span::styled(mode_text, mode_style),
        Span::raw(" "),
        Span::styled(
            "kudzu",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::raw(" · "),
        Span::raw(root_cow),
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

    let selected = app.search.selected;
    let scroll = selected.saturating_sub(height / 2);
    let total = app.search.matches.len();
    let end = (scroll + height).min(total);

    let items: Vec<ListItem> = app.search.matches[scroll..end]
        .iter()
        .enumerate()
        .map(|(offset, m)| render_search_row(m, scroll + offset == selected))
        .collect();

    let indexed = app.search.nucleo_item_count();
    let status = if app.search.indexing {
        format!(" matches · {} / indexing\u{2026} ", total)
    } else {
        format!(" matches · {} / {} ", total, indexed)
    };
    let list = List::new(items).block(Block::default().borders(Borders::ALL).title(status));
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

fn render_tree_row<'a>(node: &'a Node, selected: bool, highlight: &[u32]) -> ListItem<'a> {
    let indent: &'static str = get_indent(node.depth);
    let icon: &'static str = if node.is_dir {
        if node.expanded { "▼ " } else { "▶ " }
    } else {
        "  "
    };
    let bs = base_style(node.is_dir, node.is_hidden, node.is_symlink);
    let mut spans: Vec<Span<'a>> = vec![Span::raw(indent), Span::raw(icon)];
    spans.extend(highlighted_name(&node.name, highlight, bs));
    if node.is_dir {
        spans.push(Span::styled("/", bs));
    }
    if node.is_symlink {
        spans.push(Span::styled(" →", Style::default().fg(SYMLINK_FG)));
    }
    finish_row(spans, selected)
}

fn render_search_row<'a>(m: &'a SearchMatch, selected: bool) -> ListItem<'a> {
    let icon: &'static str = if m.is_dir { "▶ " } else { "  " };
    let bs = base_style(m.is_dir, m.is_hidden, m.is_symlink);
    let mut spans: Vec<Span<'a>> = vec![Span::raw(icon)];
    spans.extend(highlighted_name(&m.name, &m.indices, bs));
    if m.is_dir {
        spans.push(Span::styled("/", bs));
    }
    if !m.parent_rel.is_empty() {
        spans.push(Span::raw("  "));
        spans.push(Span::raw("in "));
        spans.push(Span::styled(
            m.parent_rel.as_str(),
            Style::default().fg(HIDDEN_FG),
        ));
    }
    finish_row(spans, selected)
}

fn base_style(is_dir: bool, is_hidden: bool, is_symlink: bool) -> Style {
    let mut s = Style::default();
    if is_symlink {
        s = s.fg(SYMLINK_FG);
    } else if is_dir {
        s = s.fg(DIR_FG).add_modifier(Modifier::BOLD);
    } else if is_hidden {
        s = s.fg(HIDDEN_FG);
    }
    s
}

fn finish_row(spans: Vec<Span<'_>>, selected: bool) -> ListItem<'_> {
    let item_style = if selected {
        Style::default()
            .bg(SELECTED_BG)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    ListItem::new(Line::from(spans)).style(item_style)
}

fn highlighted_name<'a>(name: &'a str, indices: &[u32], base: Style) -> Vec<Span<'a>> {
    if indices.is_empty() {
        return vec![Span::styled(name, base)];
    }
    // indices are sorted ascending (nucleo guarantee); use byte offsets from
    // char_indices to avoid String allocation per segment.
    let hl = base.fg(MATCH_FG).add_modifier(Modifier::BOLD);
    let mut spans: Vec<Span<'a>> = Vec::new();
    let mut seg_start = 0usize;
    let mut cur_hl = false;
    let mut idx_pos = 0usize;
    for (char_i, (byte_i, _)) in name.char_indices().enumerate() {
        let is_hl = idx_pos < indices.len() && indices[idx_pos] as usize == char_i;
        if is_hl {
            idx_pos += 1;
        }
        if is_hl != cur_hl {
            if byte_i > seg_start {
                spans.push(Span::styled(
                    &name[seg_start..byte_i],
                    if cur_hl { hl } else { base },
                ));
            }
            seg_start = byte_i;
            cur_hl = is_hl;
        }
    }
    if seg_start < name.len() {
        spans.push(Span::styled(
            &name[seg_start..],
            if cur_hl { hl } else { base },
        ));
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
                    format!("move {} to trash? ", name),
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
            Span::styled(
                label,
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
            ),
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
                Span::styled(
                    "/ ",
                    Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
                ),
                Span::raw(app.search.query.as_str()),
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
            (
                "Enter / double-click",
                "expand dir or open file (editor or GUI, per config)",
            ),
            ("o", "open file in editor"),
            ("M", "open in file manager"),
        ],
    },
    HelpPage {
        title: "File ops",
        rows: &[
            ("n", "new file in selected dir"),
            ("N", "new folder in selected dir"),
            ("R", "rename selected"),
            ("D", "move selected to trash (confirm y)"),
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
    let key_col = rows
        .iter()
        .map(|(k, _)| k.chars().count())
        .max()
        .unwrap_or(0);
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
        let content_area = Rect::new(inner.x, inner.y + 1, inner.width, inner.height - 1);
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
