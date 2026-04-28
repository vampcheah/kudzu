use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

use super::prompt::{Prompt, PromptKind};
use super::{Action, App};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuItem {
    NewFolder,
    NewFile,
    Rename,
    Delete,
    OpenFile,
    OpenFolder,
}

impl MenuItem {
    pub fn label(self) -> &'static str {
        match self {
            MenuItem::NewFolder => "New Folder",
            MenuItem::NewFile => "New File",
            MenuItem::Rename => "Rename",
            MenuItem::Delete => "Move to Trash",
            MenuItem::OpenFile => "Open File",
            MenuItem::OpenFolder => "Open Folder",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ContextMenu {
    /// Tree node index the menu targets. `None` when the right-click landed
    /// on empty space — in that case actions operate on the tree root.
    pub target: Option<usize>,
    pub items: Vec<MenuItem>,
    pub selected: usize,
    pub anchor: (u16, u16),
}

impl App {
    pub(super) fn menu_target_dir(&self, target: Option<usize>) -> std::path::PathBuf {
        match target {
            Some(idx) => {
                let node = &self.tree.nodes[idx];
                if node.is_dir {
                    node.path.clone()
                } else {
                    node.parent
                        .map(|p| self.tree.nodes[p].path.clone())
                        .unwrap_or_else(|| self.tree.root.clone())
                }
            }
            None => self.tree.root.clone(),
        }
    }

    pub(super) fn open_context_menu(&mut self, anchor: (u16, u16), target: Option<usize>) {
        let mut items = vec![MenuItem::NewFolder, MenuItem::NewFile];
        match target {
            Some(idx) if idx != 0 => {
                items.push(MenuItem::Rename);
                items.push(MenuItem::Delete);
                if self.tree.nodes[idx].is_dir {
                    items.push(MenuItem::OpenFolder);
                } else {
                    items.push(MenuItem::OpenFile);
                }
            }
            _ => {
                items.push(MenuItem::OpenFolder);
            }
        }
        self.menu = Some(ContextMenu {
            target,
            items,
            selected: 0,
            anchor,
        });
    }

    pub(super) fn on_key_menu(&mut self, key: KeyEvent) -> Result<Action> {
        let menu = match self.menu.as_mut() {
            Some(m) => m,
            None => return Ok(Action::None),
        };
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => self.menu = None,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => self.menu = None,
            KeyCode::Up | KeyCode::Char('w') | KeyCode::Char('k') => {
                if menu.selected > 0 {
                    menu.selected -= 1;
                }
            }
            KeyCode::Down | KeyCode::Char('s') | KeyCode::Char('j') => {
                if menu.selected + 1 < menu.items.len() {
                    menu.selected += 1;
                }
            }
            KeyCode::Enter => {
                let item = menu.items[menu.selected];
                let target = menu.target;
                self.menu = None;
                return self.execute_menu_item(item, target);
            }
            _ => {}
        }
        Ok(Action::None)
    }

    pub(super) fn execute_menu_item(
        &mut self,
        item: MenuItem,
        target: Option<usize>,
    ) -> Result<Action> {
        match item {
            MenuItem::NewFile | MenuItem::NewFolder => {
                let kind = if item == MenuItem::NewFile {
                    PromptKind::NewFile
                } else {
                    PromptKind::NewFolder
                };
                let dir = self.menu_target_dir(target);
                self.input = Some(Prompt {
                    kind,
                    buffer: String::new(),
                    cursor: 0,
                    target: dir,
                });
            }
            MenuItem::Rename => {
                if let Some(idx) = target {
                    if idx != 0 {
                        let node = &self.tree.nodes[idx];
                        let name = node.name.clone();
                        let cursor = name.chars().count();
                        self.input = Some(Prompt {
                            kind: PromptKind::Rename,
                            buffer: name,
                            cursor,
                            target: node.path.clone(),
                        });
                    }
                }
            }
            MenuItem::Delete => {
                if let Some(idx) = target {
                    if idx != 0 {
                        let node = &self.tree.nodes[idx];
                        self.input = Some(Prompt {
                            kind: PromptKind::Delete,
                            buffer: String::new(),
                            cursor: 0,
                            target: node.path.clone(),
                        });
                    }
                }
            }
            MenuItem::OpenFile => {
                if let Some(idx) = target {
                    let node = &self.tree.nodes[idx];
                    if !node.is_dir {
                        return Ok(Action::OpenInEditor(node.path.clone()));
                    }
                }
            }
            MenuItem::OpenFolder => {
                let dir = self.menu_target_dir(target);
                return Ok(Action::OpenInFileManager(dir));
            }
        }
        Ok(Action::None)
    }

    pub(super) fn on_mouse_menu(&mut self, m: MouseEvent) -> Result<Action> {
        let rect = match self.menu_rect {
            Some(r) => r,
            None => {
                self.menu = None;
                return Ok(Action::None);
            }
        };
        let inside = m.column >= rect.x
            && m.column < rect.x + rect.width
            && m.row >= rect.y
            && m.row < rect.y + rect.height;
        match m.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if !inside {
                    self.menu = None;
                    return Ok(Action::None);
                }
                let row_in = m.row.saturating_sub(rect.y) as i32 - 1;
                if let Some(menu) = self.menu.as_mut() {
                    if row_in >= 0 && (row_in as usize) < menu.items.len() {
                        let item = menu.items[row_in as usize];
                        let target = menu.target;
                        self.menu = None;
                        return self.execute_menu_item(item, target);
                    }
                }
            }
            MouseEventKind::Down(_) => {
                self.menu = None;
            }
            MouseEventKind::Moved if inside => {
                let row_in = m.row.saturating_sub(rect.y) as i32 - 1;
                if let Some(menu) = self.menu.as_mut() {
                    if row_in >= 0 && (row_in as usize) < menu.items.len() {
                        menu.selected = row_in as usize;
                    }
                }
            }
            _ => {}
        }
        Ok(Action::None)
    }
}
