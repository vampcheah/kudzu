mod input;
mod menu;
mod ops;
mod prompt;

pub use menu::ContextMenu;
pub use prompt::{Prompt, PromptKind};

use std::{
    env, io,
    path::PathBuf,
    process::Command,
    time::{Duration, Instant},
};

use anyhow::Result;
use crossbeam_channel::Sender;
use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::Backend, layout::Rect, Terminal};

use crate::{
    config::Config,
    event::{AppEvent, EventLoop},
    search::Search,
    tree::{ScanOptions, Tree, WatchDelta},
    ui,
    watcher::FsWatcher,
};

use crossterm::event::Event as CtEvent;

const SEARCH_POOL_NODE_CAP: usize = 50_000;

/// Number of pages in the help overlay (must match HELP_PAGES in ui.rs).
pub const HELP_PAGES_LEN: usize = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Search,
}

pub(super) enum Action {
    None,
    OpenInEditor(PathBuf),
    OpenInGui(PathBuf),
    OpenInFileManager(PathBuf),
    RootChanged,
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
    pub help_tab: usize,
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
    pub(super) last_click: Option<(Instant, u16, u16)>,
    pub(super) watcher: FsWatcher,
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
            help_tab: 0,
            list_area: None,
            list_scroll: 0,
            input: None,
            menu: None,
            menu_rect: None,
            last_click: None,
            watcher,
        })
    }

    pub(super) fn drain_watch(&mut self) {
        let delta = self.tree.take_watch_delta();
        apply_watch_delta(&mut self.watcher, delta);
    }

    /// Replace the tree root with its parent directory.
    pub(super) fn ascend_root(&mut self) -> Result<Option<PathBuf>> {
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

    /// Make the currently selected directory the new tree root.
    pub(super) fn descend_root(&mut self) -> Result<Option<PathBuf>> {
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

    pub(super) fn flash(&mut self, msg: impl Into<String>) {
        self.status = msg.into();
        self.status_until = Some(Instant::now() + Duration::from_secs(3));
    }

    pub(super) fn expire_status(&mut self) {
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

    pub(super) fn enter_search(&mut self) -> Result<()> {
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

    pub(super) fn exit_search(&mut self) {
        self.mode = Mode::Normal;
        if let Some(node_idx) = self.search.selected_node() {
            self.reveal(node_idx);
        }
        self.search.query.clear();
        self.search.matches.clear();
        self.search.selected = 0;
    }

    /// Expand all ancestors of `node_idx` and place selection on it.
    pub(super) fn reveal(&mut self, node_idx: usize) {
        let mut chain = Vec::new();
        let mut cur = Some(node_idx);
        while let Some(i) = cur {
            chain.push(i);
            cur = self.tree.nodes[i].parent;
        }
        chain.reverse();
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
) -> Result<()>
where
    B::Error: Send + Sync + 'static,
{
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
                if is_image(&path) {
                    let (bin, extra) = split_command(&app.cfg.gui_editor);
                    match Command::new(&bin)
                        .args(&extra)
                        .arg(&path)
                        .stdin(std::process::Stdio::null())
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::null())
                        .spawn()
                    {
                        Ok(_) => app.flash(format!("image → opened in {}", bin)),
                        Err(e) => app.flash(format!("{}: {}", bin, e)),
                    }
                } else {
                    suspend_and_run(terminal, |_| {
                        let editor = env::var("EDITOR")
                            .or_else(|_| env::var("VISUAL"))
                            .unwrap_or_else(|_| "vi".to_string());
                        let _ = Command::new(&editor).arg(&path).status();
                        Ok(())
                    })?;
                    app.flash(format!("opened {}", path.display()));
                }
            }
            Action::OpenInGui(path) => {
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
            Action::RootChanged => {}
        }
    }

    Ok(())
}

fn is_image(path: &std::path::Path) -> bool {
    const EXTS: &[&str] = &[
        "png", "jpg", "jpeg", "gif", "webp", "bmp", "svg", "ico", "tiff", "tif", "avif", "heic",
        "heif",
    ];
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .map(|e| EXTS.contains(&e.as_str()))
        .unwrap_or(false)
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
    B::Error: Send + Sync + 'static,
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
