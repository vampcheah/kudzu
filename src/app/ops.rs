use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::prompt::{Prompt, PromptKind};
use super::{Action, App, Clipboard, ClipboardMode};

impl App {
    /// Resolve the directory that should host a new file/folder, or that the
    /// "open in file manager" action should target. If a directory is
    /// selected, it's used directly; if a file is selected, its parent is
    /// used. Returns `None` if nothing sensible is available.
    pub(super) fn target_dir(&self) -> Option<PathBuf> {
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

    pub(super) fn start_new_file(&mut self) {
        self.start_new(PromptKind::NewFile);
    }

    pub(super) fn start_new_folder(&mut self) {
        self.start_new(PromptKind::NewFolder);
    }

    fn start_new(&mut self, kind: PromptKind) {
        match self.target_dir() {
            Some(dir) => {
                self.input = Some(Prompt {
                    kind,
                    buffer: String::new(),
                    cursor: 0,
                    target: dir,
                });
            }
            None => self.flash("no target directory"),
        }
    }

    pub(super) fn start_rename(&mut self) {
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

    pub(super) fn start_delete(&mut self) {
        if !self.marked.is_empty() {
            self.input = Some(Prompt {
                kind: PromptKind::Delete,
                buffer: String::new(),
                cursor: 0,
                target: self.tree.root.clone(),
            });
            return;
        }
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

    pub(super) fn open_selected_in_filemanager(&mut self) -> Action {
        match self.target_dir() {
            Some(dir) => Action::OpenInFileManager(dir),
            None => {
                self.flash("no target directory");
                Action::None
            }
        }
    }

    pub(super) fn current_path(&self) -> Option<PathBuf> {
        match self.mode {
            super::Mode::Normal => {
                let idx = self.selected_node()?;
                Some(self.tree.nodes[idx].path.clone())
            }
            super::Mode::Search => self.search.selected_match().map(|m| m.path.clone()),
        }
    }

    pub(super) fn current_dir_for_paste(&self) -> Option<PathBuf> {
        match self.mode {
            super::Mode::Normal => self.target_dir(),
            super::Mode::Search => self
                .search
                .selected_match()
                .and_then(|m| {
                    if m.is_dir {
                        Some(m.path.clone())
                    } else {
                        m.path.parent().map(Path::to_path_buf)
                    }
                })
                .or_else(|| Some(self.tree.root.clone())),
        }
    }

    pub(super) fn selected_or_marked_paths(&self) -> Vec<PathBuf> {
        if !self.marked.is_empty() {
            let mut paths: Vec<PathBuf> = self.marked.iter().cloned().collect();
            paths.sort();
            return paths;
        }
        self.current_path().into_iter().collect()
    }

    pub(super) fn toggle_mark_current(&mut self) {
        let Some(path) = self.current_path() else {
            self.flash("nothing selected");
            return;
        };
        if self.marked.remove(&path) {
            self.flash(format!("unmarked {}", short_path(&path)));
        } else {
            self.marked.insert(path.clone());
            self.flash(format!("marked {}", short_path(&path)));
        }
    }

    pub(super) fn clear_marks(&mut self) {
        let count = self.marked.len();
        self.marked.clear();
        self.flash(format!("cleared {count} marks"));
    }

    pub(super) fn mark_visible(&mut self) {
        if self.mode != super::Mode::Normal {
            self.flash("mark all is only available in tree mode");
            return;
        }
        for &idx in &self.tree.visible {
            if idx != 0 {
                self.marked.insert(self.tree.nodes[idx].path.clone());
            }
        }
        self.flash(format!("marked {}", self.marked.len()));
    }

    pub(super) fn stage_clipboard(&mut self, mode: ClipboardMode) {
        let paths = self.selected_or_marked_paths();
        if paths.is_empty() {
            self.flash("nothing selected");
            return;
        }
        let count = paths.len();
        self.clipboard = Some(Clipboard { mode, paths });
        let verb = match mode {
            ClipboardMode::Copy => "copied",
            ClipboardMode::Move => "cut",
        };
        self.flash(format!("{verb} {count} item(s)"));
    }

    pub(super) fn paste_clipboard(&mut self) -> Result<()> {
        let Some(clipboard) = self.clipboard.clone() else {
            self.flash("clipboard empty");
            return Ok(());
        };
        let Some(target_dir) = self.current_dir_for_paste() else {
            self.flash("no paste target");
            return Ok(());
        };
        if !target_dir.is_dir() {
            self.flash("paste target is not a directory");
            return Ok(());
        }
        for src in &clipboard.paths {
            let name = src
                .file_name()
                .ok_or_else(|| anyhow::anyhow!("cannot paste {}", src.display()))?;
            let dest = unique_destination(&target_dir.join(name));
            match clipboard.mode {
                ClipboardMode::Copy => copy_path(src, &dest)
                    .with_context(|| format!("copy {} to {}", src.display(), dest.display()))?,
                ClipboardMode::Move => fs::rename(src, &dest)
                    .or_else(|_| {
                        copy_path(src, &dest)?;
                        remove_path(src)
                    })
                    .with_context(|| format!("move {} to {}", src.display(), dest.display()))?,
            }
        }
        if clipboard.mode == ClipboardMode::Move {
            self.clipboard = None;
            for p in clipboard.paths {
                if let Some(parent) = p.parent() {
                    let _ = self.tree.refresh_dir(parent);
                }
                self.marked.remove(&p);
            }
        }
        self.tree.refresh_dir(&target_dir)?;
        self.drain_watch();
        self.flash("pasted");
        Ok(())
    }

    pub(super) fn cancel_prompt(&mut self) {
        self.input = None;
    }

    pub(super) fn on_key_prompt(&mut self, key: KeyEvent) -> Result<Action> {
        let prompt = match self.input.as_mut() {
            Some(p) => p,
            None => return Ok(Action::None),
        };
        if prompt.kind == PromptKind::Delete {
            match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => return self.confirm_prompt(),
                _ => {
                    self.cancel_prompt();
                    self.flash("trash cancelled");
                }
            }
            return Ok(Action::None);
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc | KeyCode::Char('c') if ctrl || key.code == KeyCode::Esc => {
                self.cancel_prompt();
                return Ok(Action::None);
            }
            KeyCode::Enter => return self.confirm_prompt(),
            KeyCode::Left | KeyCode::Char('b') if key.code == KeyCode::Left || ctrl => {
                prompt.move_left()
            }
            KeyCode::Right | KeyCode::Char('f') if key.code == KeyCode::Right || ctrl => {
                prompt.move_right()
            }
            KeyCode::Home | KeyCode::Char('a') if key.code == KeyCode::Home || ctrl => {
                prompt.move_home()
            }
            KeyCode::End | KeyCode::Char('e') if key.code == KeyCode::End || ctrl => {
                prompt.move_end()
            }
            KeyCode::Backspace => prompt.delete_before(),
            KeyCode::Delete => prompt.delete_at(),
            KeyCode::Char('w') if ctrl => prompt.delete_word_before(),
            KeyCode::Char('u') if ctrl => prompt.kill_to_start(),
            KeyCode::Char(c) => prompt.insert_char(c),
            _ => {}
        }
        Ok(Action::None)
    }

    pub(super) fn confirm_prompt(&mut self) -> Result<Action> {
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
                if prompt
                    .target
                    .file_name()
                    .map(|s| s == name.as_str())
                    .unwrap_or(false)
                {
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

    pub(super) fn perform_delete(&mut self, target: &PathBuf) -> Result<Action> {
        let targets = if self.marked.is_empty() {
            vec![target.clone()]
        } else {
            let mut paths: Vec<PathBuf> = self.marked.iter().cloned().collect();
            paths.sort();
            paths
        };
        let parent = match targets.first().and_then(|p| p.parent()) {
            Some(p) => p.to_path_buf(),
            None => {
                self.flash("cannot delete: no parent");
                return Ok(Action::None);
            }
        };
        let name = targets[0]
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| targets[0].display().to_string());
        let prev_pos = self.selected;
        for path in &targets {
            if let Err(e) = trash::delete(path) {
                self.flash(format!("trash failed: {}", e));
                return Ok(Action::None);
            }
        }
        self.marked.clear();
        self.post_mutation(&parent, None);
        if !self.tree.visible.is_empty() {
            self.selected = prev_pos.min(self.tree.visible.len() - 1);
        } else {
            self.selected = 0;
        }
        if targets.len() == 1 {
            self.flash(format!("moved to trash: {}", name));
        } else {
            self.flash(format!("moved {} items to trash", targets.len()));
        }
        Ok(Action::None)
    }

    /// After creating/renaming on disk, refresh the affected directory and
    /// place the selection on the new node when possible.
    pub(super) fn post_mutation(&mut self, parent_dir: &Path, select_path: Option<&PathBuf>) {
        if let Some(parent_idx) = self
            .tree
            .find_by_path(parent_dir)
            .filter(|&idx| self.tree.nodes[idx].is_dir && !self.tree.nodes[idx].expanded)
            && let Err(e) = self.tree.expand(parent_idx)
        {
            self.flash(format!("expand failed: {}", e));
            return;
        }
        if let Err(e) = self.tree.refresh_dir(parent_dir) {
            self.flash(format!("refresh failed: {}", e));
            return;
        }
        self.tree.rebuild_visible();
        if let Some(path) = select_path
            && let Some(node_idx) = self.tree.find_by_path(path)
            && let Some(pos) = self.tree.visible.iter().position(|&i| i == node_idx)
        {
            self.selected = pos;
        }
        if self.selected >= self.tree.visible.len() {
            self.selected = self.tree.visible.len().saturating_sub(1);
        }
    }
}

fn short_path(path: &Path) -> String {
    path.file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

fn unique_destination(path: &Path) -> PathBuf {
    if !path.exists() {
        return path.to_path_buf();
    }
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "copy".to_string());
    let ext = path.extension().map(|e| e.to_string_lossy().into_owned());
    for i in 1.. {
        let name = match &ext {
            Some(ext) if !ext.is_empty() => format!("{stem} copy {i}.{ext}"),
            _ => format!("{stem} copy {i}"),
        };
        let candidate = parent.join(name);
        if !candidate.exists() {
            return candidate;
        }
    }
    unreachable!("unbounded loop returns")
}

fn copy_path(src: &Path, dest: &Path) -> Result<()> {
    let meta = fs::symlink_metadata(src)?;
    if meta.is_dir() {
        fs::create_dir(dest)?;
        for entry in fs::read_dir(src)? {
            let entry = entry?;
            copy_path(&entry.path(), &dest.join(entry.file_name()))?;
        }
    } else if meta.file_type().is_symlink() {
        #[cfg(unix)]
        {
            let target = fs::read_link(src)?;
            std::os::unix::fs::symlink(target, dest)?;
        }
        #[cfg(not(unix))]
        {
            fs::copy(src, dest)?;
        }
    } else if meta.is_file() {
        fs::copy(src, dest)?;
    } else {
        bail!("unsupported file type: {}", src.display());
    }
    Ok(())
}

fn remove_path(path: &Path) -> Result<()> {
    let meta = fs::symlink_metadata(path)?;
    if meta.is_dir() && !meta.file_type().is_symlink() {
        fs::remove_dir_all(path)?;
    } else {
        fs::remove_file(path)?;
    }
    Ok(())
}
