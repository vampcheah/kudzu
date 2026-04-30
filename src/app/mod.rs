mod input;
mod menu;
mod ops;
mod prompt;

pub use menu::ContextMenu;
pub use prompt::{Prompt, PromptKind};

use std::{
    collections::HashSet,
    env, fs, io,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use anyhow::Result;
use crossbeam_channel::Sender;
use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::Backend, layout::Rect};

use crate::{
    config::Config,
    event::{AppEvent, EventLoop},
    search::Search,
    tree::{ScanOptions, Tree, WatchDelta},
    ui,
    watcher::FsWatcher,
};

use crossterm::event::Event as CtEvent;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipboardMode {
    Copy,
    Move,
}

#[derive(Debug, Clone)]
pub struct Clipboard {
    pub mode: ClipboardMode,
    pub paths: Vec<PathBuf>,
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
    pub marked: HashSet<PathBuf>,
    pub clipboard: Option<Clipboard>,
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
        let mut watcher = FsWatcher::new(tx.clone())?;
        apply_watch_delta(&mut watcher, tree.take_watch_delta());
        Ok(Self {
            tree,
            selected: 0,
            scroll: 0,
            status: String::new(),
            status_until: None,
            should_quit: false,
            mode: Mode::Normal,
            search: Search::new(tx.clone()),
            cfg,
            show_help: false,
            help_tab: 0,
            list_area: None,
            list_scroll: 0,
            input: None,
            menu: None,
            marked: HashSet::new(),
            clipboard: None,
            menu_rect: None,
            last_click: None,
            watcher,
        })
    }

    pub(super) fn drain_watch(&mut self) {
        let delta = self.tree.take_watch_delta();
        apply_watch_delta(&mut self.watcher, delta);
    }

    fn replace_root(&mut self, new_root: PathBuf) -> Result<()> {
        let opts = self.tree.opts;
        self.watcher.unwatch_all();
        self.tree = Tree::new(new_root, opts)?;
        self.drain_watch();
        self.scroll = 0;
        if self.mode == Mode::Search {
            self.exit_search();
        }
        Ok(())
    }

    /// Replace the tree root with its parent directory.
    pub(super) fn ascend_root(&mut self) -> Result<Option<PathBuf>> {
        let parent = match self.tree.root.parent() {
            Some(p) if p != self.tree.root.as_path() && !p.as_os_str().is_empty() => {
                p.to_path_buf()
            }
            _ => return Ok(None),
        };
        let prev_root = self.tree.root.clone();
        self.replace_root(parent.clone())?;
        if let Some(prev_idx) = self.tree.find_by_path(&prev_root) {
            if let Some(pos) = self.tree.visible.iter().position(|&i| i == prev_idx) {
                self.selected = pos;
            }
        } else {
            self.selected = 0;
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
        self.replace_root(new_root.clone())?;
        self.selected = 0;
        Ok(Some(new_root))
    }

    pub(super) fn flash(&mut self, msg: impl Into<String>) {
        self.status = msg.into();
        self.status_until = Some(Instant::now() + Duration::from_secs(3));
    }

    pub(super) fn expire_status(&mut self) {
        if let Some(t) = self.status_until
            && Instant::now() > t
        {
            self.status.clear();
            self.status_until = None;
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
            Mode::Search => None,
        }
    }

    pub(super) fn enter_search(&mut self) -> Result<()> {
        self.mode = Mode::Search;
        self.search.set_query("");
        self.search
            .start_indexing(self.tree.root.clone(), self.tree.opts);
        self.flash("indexing\u{2026}");
        Ok(())
    }

    pub(super) fn exit_search(&mut self) {
        self.search.cancel_indexing();
        self.mode = Mode::Normal;
        let selected_path = self.search.selected_match().map(|m| m.path.clone());
        if let Some(path) = selected_path
            && let Some(node_idx) = self.tree.ensure_loaded(&path)
        {
            self.reveal(node_idx);
        }
        self.search.set_query("");
        self.search.matches.clear();
        self.search.selected = 0;
        self.search.indexing = false;
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
        if refreshed > 0 && self.selected >= self.tree.visible.len() {
            self.selected = self.tree.visible.len().saturating_sub(1);
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

    // Initial draw.
    terminal.draw(|f| ui::draw(f, &mut app))?;
    if app.cfg.osc7 {
        emit_osc7(terminal.backend_mut(), &app.tree.root.clone());
    }

    loop {
        if app.should_quit {
            break;
        }

        // Returns true when a redraw is needed after processing.
        let (action, needs_draw) = match events.rx.recv()? {
            AppEvent::Input(CtEvent::Key(key)) => (app.on_key(key)?, true),
            AppEvent::Input(CtEvent::Mouse(m)) => (app.on_mouse(m)?, true),
            AppEvent::Input(CtEvent::Resize(_, _)) => (Action::None, true),
            AppEvent::Input(_) => (Action::None, true),
            AppEvent::FsChanged(paths) => {
                app.on_fs_changed(paths);
                (Action::None, true)
            }
            AppEvent::SearchUpdate => {
                let changed = app.search.tick();
                (Action::None, changed)
            }
            AppEvent::IndexDone(generation) => {
                if generation == app.search.current_generation() {
                    app.search.indexing = false;
                    app.status.clear();
                    app.status_until = None;
                }
                (Action::None, true)
            }
            AppEvent::Tick => {
                // Tick only redraws when a flash status just expired or search updated.
                let had_status = !app.status.is_empty();
                app.expire_status();
                let status_cleared = had_status && app.status.is_empty();
                let search_changed = if app.mode == Mode::Search {
                    app.search.tick()
                } else {
                    false
                };
                (Action::None, status_cleared || search_changed)
            }
        };
        app.drain_watch();

        match action {
            Action::None => {}
            Action::RootChanged => {
                if app.cfg.osc7 {
                    emit_osc7(terminal.backend_mut(), &app.tree.root.clone());
                }
            }
            Action::OpenInEditor(path) => {
                if should_use_file_opener(&path) {
                    match spawn_detached(app.cfg.opener_for_path(&path), &path) {
                        Ok(bin) => app.flash(format!("opened {} in {}", path.display(), bin)),
                        Err(e) => app.flash(e),
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
                let opener = if should_use_file_opener(&path) {
                    app.cfg.opener_for_path(&path)
                } else {
                    &app.cfg.gui_editor
                };
                match spawn_detached(opener, &path) {
                    Ok(bin) => app.flash(format!("opened {} in {}", path.display(), bin)),
                    Err(e) => app.flash(e),
                }
            }
            Action::OpenInFileManager(path) => match spawn_detached(&app.cfg.file_manager, &path) {
                Ok(bin) => app.flash(format!("opened {} in {}", path.display(), bin)),
                Err(e) => app.flash(e),
            },
        }

        if needs_draw {
            terminal.draw(|f| ui::draw(f, &mut app))?;
        }
    }

    Ok(())
}

fn should_use_file_opener(path: &Path) -> bool {
    is_image(path) || is_likely_binary(path)
}

/// Spawn a detached process with all stdio connected to /dev/null.
/// Returns the binary name on success or a `"bin: err"` string on failure.
fn spawn_detached(cmd: &str, path: &Path) -> Result<String, String> {
    let (bin, extra) = split_command(cmd)?;
    Command::new(&bin)
        .args(&extra)
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| bin.clone())
        .map_err(|e| format!("{}: {}", bin, e))
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

fn is_likely_binary(path: &Path) -> bool {
    let Ok(meta) = fs::metadata(path) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }

    let Ok(mut file) = fs::File::open(path) else {
        return false;
    };
    let mut sample = [0; 8192];
    let n = {
        use io::Read as _;
        file.read(&mut sample).unwrap_or(0)
    };
    is_binary_sample(&sample[..n])
}

fn is_binary_sample(sample: &[u8]) -> bool {
    sample.contains(&0) || std::str::from_utf8(sample).is_err()
}

fn apply_watch_delta(watcher: &mut FsWatcher, delta: WatchDelta) {
    let added: HashSet<&Path> = delta.added.iter().map(PathBuf::as_path).collect();
    for p in &delta.removed {
        if !added.contains(p.as_path()) {
            watcher.unwatch_dir(p);
        }
    }
    for p in &delta.added {
        let _ = watcher.watch_dir(p);
    }
}

/// Split a small shell-ish command string. Supports whitespace, single quotes,
/// double quotes, and `\"` / `\\` escapes inside double quotes.
fn split_command(cmd: &str) -> Result<(String, Vec<String>), String> {
    let parts = parse_command(cmd)?;
    let mut parts = parts.into_iter();
    let bin = parts.next().unwrap_or_else(|| "xdg-open".to_string());
    Ok((bin, parts.collect()))
}

fn parse_command(cmd: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut chars = cmd.chars().peekable();

    while let Some(c) = chars.next() {
        match quote {
            Some(q) if c == q => quote = None,
            Some('"') if c == '\\' && matches!(chars.peek(), Some('"') | Some('\\')) => {
                current.push(chars.next().expect("peeked char exists"));
            }
            Some(_) => current.push(c),
            None if c == '"' || c == '\'' => quote = Some(c),
            None if c.is_whitespace() => {
                if !current.is_empty() {
                    out.push(std::mem::take(&mut current));
                }
            }
            None => current.push(c),
        }
    }

    if let Some(q) = quote {
        return Err(format!("unterminated quote `{}` in command", q));
    }
    if !current.is_empty() {
        out.push(current);
    }
    Ok(out)
}

fn percent_encode_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for b in path.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                out.push(b as char);
            }
            _ => {
                use std::fmt::Write as _;
                let _ = write!(out, "%{:02X}", b);
            }
        }
    }
    out
}

fn emit_osc7(w: &mut impl io::Write, dir: &Path) {
    let encoded = percent_encode_path(&dir.to_string_lossy());
    let _ = write!(w, "\x1b]7;file://{}\x07", encoded);
    let _ = w.flush();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_command_supports_quotes() {
        let (bin, args) = split_command(r#""/opt/My Editor/bin/edit" -n "--flag=value""#).unwrap();
        assert_eq!(bin, "/opt/My Editor/bin/edit");
        assert_eq!(args, vec!["-n", "--flag=value"]);
    }

    #[test]
    fn split_command_preserves_windows_backslashes() {
        let (bin, args) = split_command(r#"C:\Tools\editor.exe "C:\Program Files""#).unwrap();
        assert_eq!(bin, r#"C:\Tools\editor.exe"#);
        assert_eq!(args, vec![r#"C:\Program Files"#]);
    }

    #[test]
    fn split_command_rejects_unterminated_quote() {
        assert!(split_command(r#""editor"#).is_err());
    }

    #[test]
    fn binary_sample_detection() {
        assert!(!is_binary_sample(b"plain utf-8 text\n"));
        assert!(is_binary_sample(b"\x89PNG\r\n\x1a\n\0"));
        assert!(is_binary_sample(&[0xff, 0xfe, 0xfd]));
    }
}
