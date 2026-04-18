use anyhow::Result;
use crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use std::time::{Duration, Instant};

use crate::config::DoubleClick;
use super::{Action, App, Mode, HELP_PAGES_LEN};

impl App {
    pub(super) fn on_key_normal(&mut self, key: KeyEvent) -> Result<Action> {
        match key.code {
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.should_quit = true
            }
            KeyCode::Down | KeyCode::Char('s') => self.tree_move(1),
            KeyCode::Up | KeyCode::Char('w') => self.tree_move(-1),
            KeyCode::PageDown | KeyCode::Char('d')
                if key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                self.tree_move(10)
            }
            KeyCode::PageUp | KeyCode::Char('u')
                if key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                self.tree_move(-10)
            }
            KeyCode::PageDown => self.tree_move(10),
            KeyCode::PageUp => self.tree_move(-10),
            KeyCode::Home | KeyCode::Char('g') => self.selected = 0,
            KeyCode::End | KeyCode::Char('G') => {
                if !self.tree.visible.is_empty() {
                    self.selected = self.tree.visible.len() - 1;
                }
            }
            KeyCode::Enter | KeyCode::Char(' ') | KeyCode::Right | KeyCode::Char('l') => {
                if let Some(idx) = self.selected_node() {
                    if self.tree.nodes[idx].is_dir {
                        self.toggle_and_reselect(idx)?;
                    } else if key.code == KeyCode::Enter {
                        return Ok(Action::OpenInEditor(self.tree.nodes[idx].path.clone()));
                    }
                }
            }
            KeyCode::Left | KeyCode::Char('u') => {
                if let Some(idx) = self.selected_node() {
                    if idx == 0 {
                        match self.ascend_root()? {
                            Some(new_root) => {
                                self.flash(format!("root: {}", new_root.display()));
                                return Ok(Action::RootChanged);
                            }
                            None => self.flash("already at filesystem root"),
                        }
                    } else if self.tree.nodes[idx].is_dir && self.tree.nodes[idx].expanded {
                        self.toggle_and_reselect(idx)?;
                    } else if let Some(parent) = self.tree.nodes[idx].parent {
                        if let Some(pos) = self.tree.visible.iter().position(|&i| i == parent) {
                            self.selected = pos;
                        }
                    }
                }
            }
            KeyCode::Char('f') => {
                match self.descend_root()? {
                    Some(new_root) => {
                        self.flash(format!("root: {}", new_root.display()));
                        return Ok(Action::RootChanged);
                    }
                    None => self.flash("select a subdirectory to focus"),
                }
            }
            KeyCode::Char('o') => {
                if let Some(idx) = self.selected_node() {
                    if !self.tree.nodes[idx].is_dir {
                        return Ok(Action::OpenInEditor(self.tree.nodes[idx].path.clone()));
                    }
                }
            }
            KeyCode::Char('n') => self.start_new_file(),
            KeyCode::Char('N') => self.start_new_folder(),
            KeyCode::Char('R') => self.start_rename(),
            KeyCode::Char('D') => self.start_delete(),
            KeyCode::Char('M') => return Ok(self.open_selected_in_filemanager()),
            KeyCode::Char('/') => self.enter_search()?,
            KeyCode::Char('h') => self.show_help = true,
            KeyCode::Char('.') => {
                self.tree.opts.show_hidden = !self.tree.opts.show_hidden;
                self.tree.rescan()?;
                self.selected = self.selected.min(self.tree.visible.len().saturating_sub(1));
                self.flash(if self.tree.opts.show_hidden {
                    "hidden files: shown"
                } else {
                    "hidden files: hidden"
                });
            }
            KeyCode::Char('i') => {
                self.tree.opts.respect_gitignore = !self.tree.opts.respect_gitignore;
                self.tree.rescan()?;
                self.selected = self.selected.min(self.tree.visible.len().saturating_sub(1));
                self.flash(if self.tree.opts.respect_gitignore {
                    "gitignore: respected"
                } else {
                    "gitignore: disabled"
                });
            }
            KeyCode::Char('r') => {
                self.tree.rescan()?;
                self.selected = self.selected.min(self.tree.visible.len().saturating_sub(1));
                self.flash("rescanned");
            }
            _ => {}
        }
        Ok(Action::None)
    }

    pub(super) fn on_key_search(&mut self, key: KeyEvent) -> Result<Action> {
        match key.code {
            KeyCode::Esc => self.exit_search(),
            KeyCode::Enter => {
                if let Some(m) = self.search.selected_match() {
                    let is_dir = m.is_dir;
                    let path = m.path.clone();
                    self.exit_search();
                    if is_dir {
                        if let Some(node_idx) = self.tree.find_by_path(&path) {
                            let _ = self.tree.expand(node_idx);
                            self.tree.rebuild_visible();
                            if let Some(pos) =
                                self.tree.visible.iter().position(|&i| i == node_idx)
                            {
                                self.selected = pos;
                            }
                        }
                    } else {
                        return Ok(Action::OpenInEditor(path));
                    }
                }
            }
            KeyCode::Down => self.search.move_selection(1),
            KeyCode::Up => self.search.move_selection(-1),
            KeyCode::PageDown => self.search.move_selection(10),
            KeyCode::PageUp => self.search.move_selection(-10),
            KeyCode::Backspace => self.search.mutate_query(|q| { q.pop(); }),
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.exit_search()
            }
            KeyCode::Char('w') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.search.mutate_query(|q| {
                    while matches!(q.chars().last(), Some(c) if c.is_whitespace()) { q.pop(); }
                    while matches!(q.chars().last(), Some(c) if !c.is_whitespace()) { q.pop(); }
                });
            }
            KeyCode::Char(c) => self.search.mutate_query(|q| q.push(c)),
            _ => {}
        }
        Ok(Action::None)
    }

    pub(super) fn on_key(&mut self, key: KeyEvent) -> Result<Action> {
        if key.kind != KeyEventKind::Press {
            return Ok(Action::None);
        }
        if self.show_help {
            match key.code {
                KeyCode::Tab => {
                    self.help_tab = (self.help_tab + 1) % HELP_PAGES_LEN;
                }
                KeyCode::BackTab => {
                    self.help_tab = (self.help_tab + HELP_PAGES_LEN - 1) % HELP_PAGES_LEN;
                }
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.should_quit = true;
                }
                _ => {
                    self.show_help = false;
                }
            }
            return Ok(Action::None);
        }
        if self.input.is_some() {
            return self.on_key_prompt(key);
        }
        if self.menu.is_some() {
            return self.on_key_menu(key);
        }
        match self.mode {
            Mode::Normal => self.on_key_normal(key),
            Mode::Search => self.on_key_search(key),
        }
    }

    pub(super) fn on_mouse(&mut self, m: MouseEvent) -> Result<Action> {
        if self.menu.is_some() {
            return self.on_mouse_menu(m);
        }
        let area = match self.list_area {
            Some(a) => a,
            None => return Ok(Action::None),
        };
        let inside = m.column >= area.x
            && m.column < area.x + area.width
            && m.row >= area.y
            && m.row < area.y + area.height;

        match m.kind {
            MouseEventKind::ScrollDown => {
                if inside {
                    self.move_in_current_mode(3);
                }
            }
            MouseEventKind::ScrollUp => {
                if inside {
                    self.move_in_current_mode(-3);
                }
            }
            MouseEventKind::Down(MouseButton::Right) if inside && self.mode == Mode::Normal => {
                let row_offset = (m.row - area.y) as usize;
                let target_row = self.list_scroll + row_offset;
                let target = if target_row < self.tree.visible.len() {
                    self.selected = target_row;
                    Some(self.tree.visible[target_row])
                } else {
                    None
                };
                self.open_context_menu((m.column, m.row), target);
            }
            MouseEventKind::Down(MouseButton::Left) if inside => {
                let row_offset = (m.row - area.y) as usize;
                let target_row = self.list_scroll + row_offset;

                let now = Instant::now();
                let is_double = matches!(self.last_click, Some((t, col, row))
                    if row == m.row
                        && col.abs_diff(m.column) <= 1
                        && now.duration_since(t) < Duration::from_millis(400));
                self.last_click = Some((now, m.column, m.row));

                match self.mode {
                    Mode::Normal => {
                        if target_row < self.tree.visible.len() {
                            self.selected = target_row;
                            if is_double {
                                return self.activate_selected();
                            }
                        }
                    }
                    Mode::Search => {
                        if target_row < self.search.matches.len() {
                            self.search.selected = target_row;
                            if is_double {
                                return self.activate_selected();
                            }
                        }
                    }
                }
            }
            _ => {}
        }
        Ok(Action::None)
    }

    pub(super) fn move_in_current_mode(&mut self, delta: isize) {
        match self.mode {
            Mode::Normal => self.tree_move(delta),
            Mode::Search => self.search.move_selection(delta),
        }
    }

    fn toggle_and_reselect(&mut self, idx: usize) -> Result<()> {
        self.tree.toggle_expand(idx)?;
        if let Some(pos) = self.tree.visible.iter().position(|&i| i == idx) {
            self.selected = pos;
        }
        Ok(())
    }

    /// Activate handler for double-click. Files honor `cfg.double_click`;
    /// directories always toggle expansion (matching the normal Enter flow).
    pub(super) fn activate_selected(&mut self) -> Result<Action> {
        let file_action = |path: std::path::PathBuf, dc: &DoubleClick| -> Action {
            match dc {
                DoubleClick::Editor => Action::OpenInEditor(path),
                DoubleClick::Gui => Action::OpenInGui(path),
            }
        };
        match self.mode {
            Mode::Normal => {
                if let Some(idx) = self.selected_node() {
                    if self.tree.nodes[idx].is_dir {
                        self.toggle_and_reselect(idx)?;
                    } else {
                        return Ok(file_action(
                            self.tree.nodes[idx].path.clone(),
                            &self.cfg.double_click,
                        ));
                    }
                }
            }
            Mode::Search => {
                if let Some(m) = self.search.selected_match() {
                    let is_dir = m.is_dir;
                    let path = m.path.clone();
                    self.exit_search();
                    if is_dir {
                        if let Some(node_idx) = self.tree.find_by_path(&path) {
                            let _ = self.tree.expand(node_idx);
                            self.tree.rebuild_visible();
                            if let Some(pos) =
                                self.tree.visible.iter().position(|&i| i == node_idx)
                            {
                                self.selected = pos;
                            }
                        }
                    } else {
                        return Ok(file_action(path, &self.cfg.double_click));
                    }
                }
            }
        }
        Ok(Action::None)
    }
}
