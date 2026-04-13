use std::{
    env, fs, io,
    path::PathBuf,
    process::Command,
    time::{Duration, Instant},
};

use anyhow::Result;
use crossterm::{
    event::{
        DisableMouseCapture, EnableMouseCapture, Event as CtEvent, KeyCode, KeyEvent,
        KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::Backend, layout::Rect, Terminal};

use crossbeam_channel::Sender;

use crate::{
    config::{Config, DoubleClick},
    event::{AppEvent, EventLoop},
    search::Search,
    tree::{ScanOptions, Tree, WatchDelta},
    ui,
    watcher::FsWatcher,
};

const SEARCH_POOL_NODE_CAP: usize = 50_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Search,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptKind {
    NewFile,
    NewFolder,
    Rename,
    Delete,
}

#[derive(Debug, Clone)]
pub struct Prompt {
    pub kind: PromptKind,
    pub buffer: String,
    /// Cursor position as a char (not byte) index into `buffer`.
    pub cursor: usize,
    /// For NewFile/NewFolder: the parent directory the new entry will be
    /// created in. For Rename/Delete: the full path of the entry being
    /// renamed or deleted.
    pub target: PathBuf,
}

impl Prompt {
    fn char_len(&self) -> usize {
        self.buffer.chars().count()
    }

    fn byte_at(&self, char_idx: usize) -> usize {
        self.buffer
            .char_indices()
            .nth(char_idx)
            .map(|(b, _)| b)
            .unwrap_or(self.buffer.len())
    }

    fn insert_char(&mut self, c: char) {
        let byte = self.byte_at(self.cursor);
        self.buffer.insert(byte, c);
        self.cursor += 1;
    }

    fn delete_before(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let start = self.byte_at(self.cursor - 1);
        let end = self.byte_at(self.cursor);
        self.buffer.replace_range(start..end, "");
        self.cursor -= 1;
    }

    fn delete_at(&mut self) {
        let len = self.char_len();
        if self.cursor >= len {
            return;
        }
        let start = self.byte_at(self.cursor);
        let end = self.byte_at(self.cursor + 1);
        self.buffer.replace_range(start..end, "");
    }

    fn delete_word_before(&mut self) {
        let chars: Vec<char> = self.buffer.chars().collect();
        let mut i = self.cursor;
        while i > 0 && chars[i - 1].is_whitespace() {
            i -= 1;
        }
        while i > 0 && !chars[i - 1].is_whitespace() {
            i -= 1;
        }
        let start = self.byte_at(i);
        let end = self.byte_at(self.cursor);
        self.buffer.replace_range(start..end, "");
        self.cursor = i;
    }

    fn kill_to_start(&mut self) {
        let end = self.byte_at(self.cursor);
        self.buffer.replace_range(0..end, "");
        self.cursor = 0;
    }

    fn move_left(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
        }
    }

    fn move_right(&mut self) {
        let len = self.char_len();
        if self.cursor < len {
            self.cursor += 1;
        }
    }

    fn move_home(&mut self) {
        self.cursor = 0;
    }

    fn move_end(&mut self) {
        self.cursor = self.char_len();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuItem {
    NewFolder,
    NewFile,
    Rename,
    OpenFile,
    OpenFolder,
}

impl MenuItem {
    pub fn label(self) -> &'static str {
        match self {
            MenuItem::NewFolder => "New Folder",
            MenuItem::NewFile => "New File",
            MenuItem::Rename => "Rename",
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

pub struct App {
    pub tree: Tree,
    pub selected: usize,
    pub scroll: usize,
    pub status: String,
    pub status_until: Option<Instant>,
    pub should_quit: bool,
    pub mode: Mode,
    pub search: Search,
    pub cfg: Config,
    pub show_help: bool,
    /// Inner rect of the currently rendered list (set by the UI each frame);
    /// used by mouse handlers to map screen coordinates to row indices.
    pub list_area: Option<Rect>,
    /// Scroll offset used by the last frame (for search mode, which
    /// recomputes scroll in the renderer).
    pub list_scroll: usize,
    pub input: Option<Prompt>,
    pub menu: Option<ContextMenu>,
    /// Screen rect of the context menu popup (set by UI each frame while
    /// the menu is visible). Used by mouse handlers to hit-test clicks.
    pub menu_rect: Option<Rect>,
    last_click: Option<(Instant, u16, u16)>,
    watcher: FsWatcher,
}

enum Action {
    None,
    OpenInEditor(PathBuf),
    OpenInGui(PathBuf),
    OpenInFileManager(PathBuf),
    RootChanged,
}

impl App {
    pub fn new(root: PathBuf, cfg: Config, tx: Sender<AppEvent>) -> Result<Self> {
        let opts = ScanOptions {
            show_hidden: cfg.show_hidden,
            respect_gitignore: cfg.respect_gitignore,
        };
        let mut tree = Tree::new(root, opts)?;
        let mut watcher = FsWatcher::new(tx)?;
        apply_watch_delta(&mut watcher, tree.take_watch_delta());
        Ok(Self {
            tree,
            selected: 0,
            scroll: 0,
            status: String::new(),
            status_until: None,
            should_quit: false,
            mode: Mode::Normal,
            search: Search::new(),
            cfg,
            show_help: false,
            list_area: None,
            list_scroll: 0,
            input: None,
            menu: None,
            menu_rect: None,
            last_click: None,
            watcher,
        })
    }

    fn drain_watch(&mut self) {
        let delta = self.tree.take_watch_delta();
        apply_watch_delta(&mut self.watcher, delta);
    }

    /// Replace the tree root with its parent directory. Returns the new root
    /// path so the caller can rewatch it.
    fn ascend_root(&mut self) -> Result<Option<PathBuf>> {
        let parent = match self.tree.root.parent() {
            Some(p) if p != self.tree.root.as_path() && !p.as_os_str().is_empty() => {
                p.to_path_buf()
            }
            _ => return Ok(None),
        };
        let opts = self.tree.opts;
        let prev_root = self.tree.root.clone();
        self.watcher.unwatch_all();
        self.tree = Tree::new(parent.clone(), opts)?;
        self.drain_watch();
        // Try to keep selection on the child we came from.
        if let Some(prev_idx) = self.tree.find_by_path(&prev_root) {
            if let Some(pos) = self.tree.visible.iter().position(|&i| i == prev_idx) {
                self.selected = pos;
            }
        } else {
            self.selected = 0;
        }
        self.scroll = 0;
        if self.mode == Mode::Search {
            self.exit_search();
        }
        Ok(Some(parent))
    }

    /// Make the currently selected directory the new tree root. Returns the
    /// new root path so the caller can rewatch it.
    fn descend_root(&mut self) -> Result<Option<PathBuf>> {
        let idx = match self.selected_node() {
            Some(i) => i,
            None => return Ok(None),
        };
        if idx == 0 || !self.tree.nodes[idx].is_dir {
            return Ok(None);
        }
        let new_root = self.tree.nodes[idx].path.clone();
        let opts = self.tree.opts;
        self.watcher.unwatch_all();
        self.tree = Tree::new(new_root.clone(), opts)?;
        self.drain_watch();
        self.selected = 0;
        self.scroll = 0;
        if self.mode == Mode::Search {
            self.exit_search();
        }
        Ok(Some(new_root))
    }

    fn flash(&mut self, msg: impl Into<String>) {
        self.status = msg.into();
        self.status_until = Some(Instant::now() + Duration::from_secs(3));
    }

    fn expire_status(&mut self) {
        if let Some(t) = self.status_until {
            if Instant::now() > t {
                self.status.clear();
                self.status_until = None;
            }
        }
    }

    pub fn tree_move(&mut self, delta: isize) {
        let len = self.tree.visible.len() as isize;
        if len == 0 {
            return;
        }
        let new = (self.selected as isize + delta).clamp(0, len - 1);
        self.selected = new as usize;
    }

    pub fn selected_node(&self) -> Option<usize> {
        match self.mode {
            Mode::Normal => self.tree.visible.get(self.selected).copied(),
            Mode::Search => self.search.selected_node(),
        }
    }

    fn enter_search(&mut self) -> Result<()> {
        let before = self.tree.nodes.len();
        self.tree.load_all(SEARCH_POOL_NODE_CAP)?;
        let loaded = self.tree.nodes.len();
        self.tree.rebuild_visible();
        self.mode = Mode::Search;
        self.search.query.clear();
        self.search.recompute(&self.tree);
        if loaded > before {
            self.flash(format!("indexed {loaded} entries for search"));
        }
        Ok(())
    }

    fn exit_search(&mut self) {
        self.mode = Mode::Normal;
        // Keep selection on a reasonable tree node — jump to the match that
        // was highlighted so the user lands on it.
        if let Some(node_idx) = self.search.selected_node() {
            self.reveal(node_idx);
        }
        self.search.query.clear();
        self.search.matches.clear();
        self.search.selected = 0;
    }

    /// Expand all ancestors of `node_idx` and place selection on it.
    fn reveal(&mut self, node_idx: usize) {
        // Collect ancestor chain (root -> node).
        let mut chain = Vec::new();
        let mut cur = Some(node_idx);
        while let Some(i) = cur {
            chain.push(i);
            cur = self.tree.nodes[i].parent;
        }
        chain.reverse();
        // Expand each ancestor dir (skip the target itself; only expand if dir).
        for &i in &chain[..chain.len().saturating_sub(1)] {
            if self.tree.nodes[i].is_dir {
                let _ = self.tree.expand(i);
            }
        }
        self.tree.rebuild_visible();
        if let Some(pos) = self.tree.visible.iter().position(|&i| i == node_idx) {
            self.selected = pos;
        }
    }

    fn on_key_normal(&mut self, key: KeyEvent) -> Result<Action> {
        match key.code {
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.should_quit = true
            }
            KeyCode::Down | KeyCode::Char('s') => self.tree_move(1),
            KeyCode::Up | KeyCode::Char('w') => self.tree_move(-1),
            KeyCode::PageDown | KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.tree_move(10)
            }
            KeyCode::PageUp | KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
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
            KeyCode::Enter => {
                if let Some(idx) = self.selected_node() {
                    if self.tree.nodes[idx].is_dir {
                        self.tree.toggle_expand(idx)?;
                        if let Some(pos) = self.tree.visible.iter().position(|&i| i == idx) {
                            self.selected = pos;
                        }
                    } else {
                        return Ok(Action::OpenInEditor(self.tree.nodes[idx].path.clone()));
                    }
                }
            }
            KeyCode::Char(' ') | KeyCode::Right | KeyCode::Char('l') => {
                if let Some(idx) = self.selected_node() {
                    if self.tree.nodes[idx].is_dir {
                        self.tree.toggle_expand(idx)?;
                        if let Some(pos) = self.tree.visible.iter().position(|&i| i == idx) {
                            self.selected = pos;
                        }
                    }
                }
            }
            KeyCode::Left | KeyCode::Char('u') => {
                if let Some(idx) = self.selected_node() {
                    // At the root node: ascend to parent directory.
                    if idx == 0 {
                        match self.ascend_root()? {
                            Some(new_root) => {
                                self.flash(format!("root: {}", new_root.display()));
                                return Ok(Action::RootChanged);
                            }
                            None => self.flash("already at filesystem root"),
                        }
                    } else if self.tree.nodes[idx].is_dir && self.tree.nodes[idx].expanded {
                        self.tree.toggle_expand(idx)?;
                        if let Some(pos) = self.tree.visible.iter().position(|&i| i == idx) {
                            self.selected = pos;
                        }
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

    fn on_key_search(&mut self, key: KeyEvent) -> Result<Action> {
        match key.code {
            KeyCode::Esc => self.exit_search(),
            KeyCode::Enter => {
                if let Some(idx) = self.search.selected_node() {
                    let is_dir = self.tree.nodes[idx].is_dir;
                    let path = self.tree.nodes[idx].path.clone();
                    self.exit_search();
                    if is_dir {
                        // Expand the directory we jumped to for continuity.
                        if let Some(node_idx) = self.tree.find_by_path(&path) {
                            let _ = self.tree.expand(node_idx);
                            self.tree.rebuild_visible();
                            if let Some(pos) = self.tree.visible.iter().position(|&i| i == node_idx)
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
            KeyCode::Backspace => {
                self.search.query.pop();
                self.search.recompute(&self.tree);
            }
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.exit_search()
            }
            KeyCode::Char('w') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                // Delete last word.
                while matches!(self.search.query.chars().last(), Some(c) if c.is_whitespace()) {
                    self.search.query.pop();
                }
                while matches!(self.search.query.chars().last(), Some(c) if !c.is_whitespace()) {
                    self.search.query.pop();
                }
                self.search.recompute(&self.tree);
            }
            KeyCode::Char(c) => {
                self.search.query.push(c);
                self.search.recompute(&self.tree);
            }
            _ => {}
        }
        Ok(Action::None)
    }

    /// Resolve the directory that should host a new file/folder, or that the
    /// "open in file manager" action should target. If a directory is
    /// selected, it's used directly; if a file is selected, its parent is
    /// used. Returns `None` if nothing sensible is available.
    fn target_dir(&self) -> Option<PathBuf> {
        let idx = self.selected_node()?;
        let node = &self.tree.nodes[idx];
        if node.is_dir {
            Some(node.path.clone())
        } else {
            node.parent
                .map(|p| self.tree.nodes[p].path.clone())
                .or_else(|| node.path.parent().map(|p| p.to_path_buf()))
        }
    }

    fn start_new_file(&mut self) {
        match self.target_dir() {
            Some(dir) => {
                self.input = Some(Prompt {
                    kind: PromptKind::NewFile,
                    buffer: String::new(),
                    cursor: 0,
                    target: dir,
                });
            }
            None => self.flash("no target directory"),
        }
    }

    fn start_new_folder(&mut self) {
        match self.target_dir() {
            Some(dir) => {
                self.input = Some(Prompt {
                    kind: PromptKind::NewFolder,
                    buffer: String::new(),
                    cursor: 0,
                    target: dir,
                });
            }
            None => self.flash("no target directory"),
        }
    }

    fn start_rename(&mut self) {
        let idx = match self.selected_node() {
            Some(i) => i,
            None => {
                self.flash("nothing selected");
                return;
            }
        };
        if idx == 0 {
            self.flash("cannot rename the root");
            return;
        }
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

    fn start_delete(&mut self) {
        let idx = match self.selected_node() {
            Some(i) => i,
            None => {
                self.flash("nothing selected");
                return;
            }
        };
        if idx == 0 {
            self.flash("cannot delete the root");
            return;
        }
        let node = &self.tree.nodes[idx];
        self.input = Some(Prompt {
            kind: PromptKind::Delete,
            buffer: String::new(),
            cursor: 0,
            target: node.path.clone(),
        });
    }

    fn open_selected_in_filemanager(&mut self) -> Action {
        match self.target_dir() {
            Some(dir) => Action::OpenInFileManager(dir),
            None => {
                self.flash("no target directory");
                Action::None
            }
        }
    }

    fn cancel_prompt(&mut self) {
        self.input = None;
    }

    fn on_key_prompt(&mut self, key: KeyEvent) -> Result<Action> {
        let prompt = match self.input.as_mut() {
            Some(p) => p,
            None => return Ok(Action::None),
        };
        if prompt.kind == PromptKind::Delete {
            match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => return self.confirm_prompt(),
                _ => {
                    self.cancel_prompt();
                    self.flash("delete cancelled");
                }
            }
            return Ok(Action::None);
        }
        match key.code {
            KeyCode::Esc => self.cancel_prompt(),
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.cancel_prompt()
            }
            KeyCode::Enter => return self.confirm_prompt(),
            KeyCode::Left => prompt.move_left(),
            KeyCode::Right => prompt.move_right(),
            KeyCode::Home => prompt.move_home(),
            KeyCode::End => prompt.move_end(),
            KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                prompt.move_home()
            }
            KeyCode::Char('e') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                prompt.move_end()
            }
            KeyCode::Char('b') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                prompt.move_left()
            }
            KeyCode::Char('f') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                prompt.move_right()
            }
            KeyCode::Backspace => prompt.delete_before(),
            KeyCode::Delete => prompt.delete_at(),
            KeyCode::Char('w') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                prompt.delete_word_before()
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                prompt.kill_to_start()
            }
            KeyCode::Char(c) => prompt.insert_char(c),
            _ => {}
        }
        Ok(Action::None)
    }

    fn confirm_prompt(&mut self) -> Result<Action> {
        let prompt = match self.input.take() {
            Some(p) => p,
            None => return Ok(Action::None),
        };
        if prompt.kind == PromptKind::Delete {
            return self.perform_delete(&prompt.target);
        }
        let name = prompt.buffer.trim().to_string();
        if name.is_empty() {
            self.flash("cancelled: empty name");
            return Ok(Action::None);
        }
        if name.contains('/') || name.contains('\\') {
            self.flash("name may not contain path separators");
            return Ok(Action::None);
        }

        match prompt.kind {
            PromptKind::NewFile => {
                let new_path = prompt.target.join(&name);
                if new_path.exists() {
                    self.flash(format!("exists: {}", name));
                    return Ok(Action::None);
                }
                if let Err(e) = fs::File::create(&new_path) {
                    self.flash(format!("create failed: {}", e));
                    return Ok(Action::None);
                }
                self.post_mutation(&prompt.target, Some(&new_path));
                self.flash(format!("created {}", name));
            }
            PromptKind::NewFolder => {
                let new_path = prompt.target.join(&name);
                if new_path.exists() {
                    self.flash(format!("exists: {}", name));
                    return Ok(Action::None);
                }
                if let Err(e) = fs::create_dir(&new_path) {
                    self.flash(format!("mkdir failed: {}", e));
                    return Ok(Action::None);
                }
                self.post_mutation(&prompt.target, Some(&new_path));
                self.flash(format!("created {}/", name));
            }
            PromptKind::Delete => unreachable!("handled above"),
            PromptKind::Rename => {
                let parent = match prompt.target.parent() {
                    Some(p) => p.to_path_buf(),
                    None => {
                        self.flash("cannot rename: no parent");
                        return Ok(Action::None);
                    }
                };
                if name == prompt.target.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default() {
                    return Ok(Action::None);
                }
                let new_path = parent.join(&name);
                if new_path.exists() {
                    self.flash(format!("exists: {}", name));
                    return Ok(Action::None);
                }
                if let Err(e) = fs::rename(&prompt.target, &new_path) {
                    self.flash(format!("rename failed: {}", e));
                    return Ok(Action::None);
                }
                self.post_mutation(&parent, Some(&new_path));
                self.flash(format!("renamed → {}", name));
            }
        }
        Ok(Action::None)
    }

    fn perform_delete(&mut self, target: &PathBuf) -> Result<Action> {
        let parent = match target.parent() {
            Some(p) => p.to_path_buf(),
            None => {
                self.flash("cannot delete: no parent");
                return Ok(Action::None);
            }
        };
        let name = target
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| target.display().to_string());
        // Preserve the current visible position so the selection stays near
        // where the deleted entry was (rather than jumping to the top).
        let prev_pos = self.selected;
        let meta = fs::symlink_metadata(target);
        let res = match meta {
            Ok(m) if m.file_type().is_dir() && !m.file_type().is_symlink() => {
                fs::remove_dir_all(target)
            }
            _ => fs::remove_file(target),
        };
        if let Err(e) = res {
            self.flash(format!("delete failed: {}", e));
            return Ok(Action::None);
        }
        self.post_mutation(&parent, None);
        if !self.tree.visible.is_empty() {
            self.selected = prev_pos.min(self.tree.visible.len() - 1);
        } else {
            self.selected = 0;
        }
        self.flash(format!("deleted {}", name));
        Ok(Action::None)
    }

    /// After creating/renaming on disk, refresh the affected directory and
    /// place the selection on the new node when possible.
    fn post_mutation(&mut self, parent_dir: &PathBuf, select_path: Option<&PathBuf>) {
        if let Some(parent_idx) = self.tree.find_by_path(parent_dir) {
            if self.tree.nodes[parent_idx].is_dir
                && !self.tree.nodes[parent_idx].expanded
            {
                if let Err(e) = self.tree.expand(parent_idx) {
                    self.flash(format!("expand failed: {}", e));
                    return;
                }
            }
        }
        if let Err(e) = self.tree.refresh_dir(parent_dir) {
            self.flash(format!("refresh failed: {}", e));
            return;
        }
        self.tree.rebuild_visible();
        if let Some(path) = select_path {
            if let Some(node_idx) = self.tree.find_by_path(path) {
                if let Some(pos) = self.tree.visible.iter().position(|&i| i == node_idx) {
                    self.selected = pos;
                }
            }
        }
        if self.selected >= self.tree.visible.len() {
            self.selected = self.tree.visible.len().saturating_sub(1);
        }
    }

    fn menu_target_dir(&self, target: Option<usize>) -> PathBuf {
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

    fn open_context_menu(&mut self, anchor: (u16, u16), target: Option<usize>) {
        let mut items = vec![MenuItem::NewFolder, MenuItem::NewFile];
        match target {
            Some(idx) if idx != 0 => {
                items.push(MenuItem::Rename);
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

    fn on_key_menu(&mut self, key: KeyEvent) -> Result<Action> {
        let menu = match self.menu.as_mut() {
            Some(m) => m,
            None => return Ok(Action::None),
        };
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => self.menu = None,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.menu = None
            }
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

    fn execute_menu_item(
        &mut self,
        item: MenuItem,
        target: Option<usize>,
    ) -> Result<Action> {
        match item {
            MenuItem::NewFile => {
                let dir = self.menu_target_dir(target);
                self.input = Some(Prompt {
                    kind: PromptKind::NewFile,
                    buffer: String::new(),
                    cursor: 0,
                    target: dir,
                });
            }
            MenuItem::NewFolder => {
                let dir = self.menu_target_dir(target);
                self.input = Some(Prompt {
                    kind: PromptKind::NewFolder,
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

    fn on_mouse_menu(&mut self, m: MouseEvent) -> Result<Action> {
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
                // Item rows start after the top border.
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

    fn on_key(&mut self, key: KeyEvent) -> Result<Action> {
        if key.kind != KeyEventKind::Press {
            return Ok(Action::None);
        }
        if self.show_help {
            // While the help overlay is visible, any key dismisses it;
            // Ctrl-c still quits as a safety hatch.
            if matches!(key.code, KeyCode::Char('c'))
                && key.modifiers.contains(KeyModifiers::CONTROL)
            {
                self.should_quit = true;
            } else {
                self.show_help = false;
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

    fn on_mouse(&mut self, m: MouseEvent) -> Result<Action> {
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
                let is_double = match self.last_click {
                    Some((t, col, row))
                        if row == m.row
                            && col.abs_diff(m.column) <= 1
                            && now.duration_since(t) < Duration::from_millis(400) =>
                    {
                        true
                    }
                    _ => false,
                };
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

    fn move_in_current_mode(&mut self, delta: isize) {
        match self.mode {
            Mode::Normal => self.tree_move(delta),
            Mode::Search => self.search.move_selection(delta),
        }
    }

    /// Activate handler for double-click. Files honor `cfg.double_click`;
    /// directories always toggle expansion (matching the normal Enter flow).
    fn activate_selected(&mut self) -> Result<Action> {
        let file_action = |path: PathBuf, cfg: &Config| -> Action {
            match cfg.double_click {
                DoubleClick::Editor => Action::OpenInEditor(path),
                DoubleClick::Gui => Action::OpenInGui(path),
            }
        };
        match self.mode {
            Mode::Normal => {
                if let Some(idx) = self.selected_node() {
                    if self.tree.nodes[idx].is_dir {
                        self.tree.toggle_expand(idx)?;
                        if let Some(pos) = self.tree.visible.iter().position(|&i| i == idx) {
                            self.selected = pos;
                        }
                    } else {
                        return Ok(file_action(self.tree.nodes[idx].path.clone(), &self.cfg));
                    }
                }
            }
            Mode::Search => {
                if let Some(idx) = self.search.selected_node() {
                    let is_dir = self.tree.nodes[idx].is_dir;
                    let path = self.tree.nodes[idx].path.clone();
                    self.exit_search();
                    if is_dir {
                        if let Some(node_idx) = self.tree.find_by_path(&path) {
                            let _ = self.tree.expand(node_idx);
                            self.tree.rebuild_visible();
                            if let Some(pos) = self.tree.visible.iter().position(|&i| i == node_idx)
                            {
                                self.selected = pos;
                            }
                        }
                    } else {
                        return Ok(file_action(path, &self.cfg));
                    }
                }
            }
        }
        Ok(Action::None)
    }

    pub fn on_fs_changed(&mut self, paths: Vec<PathBuf>) {
        let mut refreshed = 0usize;
        for p in paths {
            if self.tree.find_by_path(&p).is_some() {
                if let Err(e) = self.tree.refresh_dir(&p) {
                    self.flash(format!("refresh error: {e}"));
                } else {
                    refreshed += 1;
                }
            }
        }
        if refreshed > 0 {
            if self.selected >= self.tree.visible.len() {
                self.selected = self.tree.visible.len().saturating_sub(1);
            }
            if self.mode == Mode::Search {
                self.search.recompute(&self.tree);
            }
        }
    }
}

pub fn run<B: Backend + io::Write>(
    terminal: &mut Terminal<B>,
    root: PathBuf,
    cfg: Config,
) -> Result<()> {
    let events = EventLoop::new()?;
    let mut app = App::new(root, cfg, events.tx.clone())?;

    loop {
        app.expire_status();
        terminal.draw(|f| ui::draw(f, &mut app))?;
        if app.should_quit {
            break;
        }

        let action = match events.rx.recv()? {
            AppEvent::Input(CtEvent::Key(key)) => app.on_key(key)?,
            AppEvent::Input(CtEvent::Mouse(m)) => app.on_mouse(m)?,
            AppEvent::Input(CtEvent::Resize(_, _)) => Action::None,
            AppEvent::Input(_) => Action::None,
            AppEvent::FsChanged(paths) => {
                app.on_fs_changed(paths);
                Action::None
            }
            AppEvent::Tick => Action::None,
        };
        app.drain_watch();

        match action {
            Action::None => {}
            Action::OpenInEditor(path) => {
                suspend_and_run(terminal, |_| {
                    let editor = env::var("EDITOR")
                        .or_else(|_| env::var("VISUAL"))
                        .unwrap_or_else(|_| "vi".to_string());
                    let _ = Command::new(&editor).arg(&path).status();
                    Ok(())
                })?;
                app.flash(format!("opened {}", path.display()));
            }
            Action::OpenInGui(path) => {
                // Detach — we stay in the TUI while the GUI app runs.
                let (bin, extra) = split_command(&app.cfg.gui_editor);
                match Command::new(&bin)
                    .args(&extra)
                    .arg(&path)
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn()
                {
                    Ok(_) => app.flash(format!("opened {} in {}", path.display(), bin)),
                    Err(e) => app.flash(format!("{}: {}", bin, e)),
                }
            }
            Action::OpenInFileManager(path) => {
                let (bin, extra) = split_command(&app.cfg.file_manager);
                match Command::new(&bin)
                    .args(&extra)
                    .arg(&path)
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn()
                {
                    Ok(_) => app.flash(format!("opened {} in {}", path.display(), bin)),
                    Err(e) => app.flash(format!("{}: {}", bin, e)),
                }
            }
            Action::RootChanged => {
                // Watcher was swapped inside ascend_root/descend_root; nothing
                // to do here.
            }
        }
    }

    Ok(())
}

fn apply_watch_delta(watcher: &mut FsWatcher, delta: WatchDelta) {
    use std::collections::HashSet;
    let added_set: HashSet<&PathBuf> = delta.added.iter().collect();
    for p in &delta.removed {
        if !added_set.contains(p) {
            watcher.unwatch_dir(p);
        }
    }
    for p in &delta.added {
        let _ = watcher.watch_dir(p);
    }
}

/// Split a shell-ish command string on whitespace. First token is the
/// program; the rest are preceding arguments (the selected path is
/// appended afterward). Quoting is not supported — keep commands simple.
fn split_command(cmd: &str) -> (String, Vec<String>) {
    let mut parts = cmd.split_whitespace();
    let bin = parts.next().unwrap_or("xdg-open").to_string();
    let extra = parts.map(|s| s.to_string()).collect();
    (bin, extra)
}

/// Leave the alternate screen / raw mode, run `f`, then restore.
fn suspend_and_run<B: Backend + io::Write, F>(terminal: &mut Terminal<B>, f: F) -> Result<()>
where
    F: FnOnce(&mut Terminal<B>) -> Result<()>,
{
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    let res = f(terminal);

    enable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        EnterAlternateScreen,
        EnableMouseCapture
    )?;
    terminal.clear()?;
    res
}
