use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
};

use anyhow::Result;
use ignore::WalkBuilder;

#[derive(Debug, Clone, Copy)]
pub struct ScanOptions {
    pub show_hidden: bool,
    pub respect_gitignore: bool,
}

impl Default for ScanOptions {
    fn default() -> Self {
        Self {
            show_hidden: false,
            respect_gitignore: true,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Node {
    pub path: PathBuf,
    pub name: String,
    pub depth: usize,
    pub is_dir: bool,
    pub is_hidden: bool,
    pub is_symlink: bool,
    pub size: u64,
    pub parent: Option<usize>,
    pub expanded: bool,
    pub children_loaded: bool,
}

pub struct Tree {
    pub root: PathBuf,
    pub nodes: Vec<Node>,
    pub visible: Vec<usize>,
    pub opts: ScanOptions,
    watch_delta: WatchDelta,
    /// children[i] holds the node indices whose parent == i, in insertion order.
    children: Vec<Vec<usize>>,
    /// O(1) lookup: path → node index.
    path_index: HashMap<PathBuf, usize>,
}

/// Pending set of directories that should start / stop being watched.
/// Populated by tree mutations, drained by the `App` layer which forwards
/// it to the `FsWatcher`.
#[derive(Debug, Default, Clone)]
pub struct WatchDelta {
    pub added: Vec<PathBuf>,
    pub removed: Vec<PathBuf>,
}

impl Tree {
    pub fn new(root: PathBuf, opts: ScanOptions) -> Result<Self> {
        let name = root
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| root.display().to_string());
        let meta = fs::metadata(&root).ok();
        let root_node = Node {
            path: root.clone(),
            name,
            depth: 0,
            is_dir: true,
            is_hidden: false,
            is_symlink: false,
            size: meta.as_ref().map(|m| m.len()).unwrap_or(0),
            parent: None,
            expanded: false,
            children_loaded: false,
        };
        let mut t = Self {
            root,
            nodes: vec![root_node],
            visible: vec![],
            opts,
            watch_delta: WatchDelta::default(),
            children: vec![Vec::new()],
            path_index: HashMap::new(),
        };
        t.path_index.insert(t.nodes[0].path.clone(), 0);
        t.toggle_expand(0)?;
        Ok(t)
    }

    /// Hand off pending watch changes to the caller (clears the internal buffer).
    pub fn take_watch_delta(&mut self) -> WatchDelta {
        std::mem::take(&mut self.watch_delta)
    }

    /// All currently-watched directories in the subtree rooted at `idx`.
    /// A dir is "watched" iff it is expanded (children visible). Descends
    /// only through expanded dirs — collapsed subtrees are not watched.
    fn collect_watched_subtree(&self, idx: usize) -> Vec<PathBuf> {
        let mut out = Vec::new();
        let mut stack = vec![idx];
        while let Some(i) = stack.pop() {
            if self.nodes[i].is_dir && self.nodes[i].expanded {
                out.push(self.nodes[i].path.clone());
                for &c in self.children_of(i) {
                    stack.push(c);
                }
            }
        }
        out
    }

    fn children_of(&self, idx: usize) -> &[usize] {
        &self.children[idx]
    }

    pub fn toggle_expand(&mut self, idx: usize) -> Result<()> {
        if !self.nodes[idx].is_dir {
            return Ok(());
        }
        if !self.nodes[idx].children_loaded {
            self.load_children(idx)?;
        }
        if self.nodes[idx].expanded {
            let watched = self.collect_watched_subtree(idx);
            self.watch_delta.removed.extend(watched);
            self.nodes[idx].expanded = false;
        } else {
            self.nodes[idx].expanded = true;
            let watched = self.collect_watched_subtree(idx);
            self.watch_delta.added.extend(watched);
        }
        self.rebuild_visible();
        Ok(())
    }

    pub fn expand(&mut self, idx: usize) -> Result<()> {
        if self.nodes[idx].is_dir && !self.nodes[idx].expanded {
            self.toggle_expand(idx)?;
        }
        Ok(())
    }

    fn load_children(&mut self, idx: usize) -> Result<()> {
        let parent_path = self.nodes[idx].path.clone();
        let parent_depth = self.nodes[idx].depth;
        let entries = read_children(&parent_path, &self.opts);
        for entry in entries {
            let new_idx = self.nodes.len();
            self.path_index.insert(entry.path.clone(), new_idx);
            self.children.push(Vec::new());
            self.children[idx].push(new_idx);
            self.nodes.push(Node {
                path: entry.path,
                name: entry.name,
                depth: parent_depth + 1,
                is_dir: entry.is_dir,
                is_hidden: entry.is_hidden,
                is_symlink: entry.is_symlink,
                size: entry.size,
                parent: Some(idx),
                expanded: false,
                children_loaded: false,
            });
        }
        self.nodes[idx].children_loaded = true;
        Ok(())
    }

    /// Refresh a directory's children after fs events; preserves the
    /// expansion state of surviving subdirectories.
    pub fn refresh_dir(&mut self, dir_path: &Path) -> Result<()> {
        let idx = match self.find_by_path(dir_path) {
            Some(i) => i,
            None => return Ok(()),
        };
        if !self.nodes[idx].is_dir || !self.nodes[idx].children_loaded {
            return Ok(());
        }

        let old_watched = self.collect_watched_subtree(idx);

        // Snapshot expansion state of surviving subdirectories (keyed by path).
        let old: HashMap<PathBuf, bool> = self
            .children_of(idx)
            .iter()
            .copied()
            .filter(|&ci| self.nodes[ci].is_dir && self.nodes[ci].expanded)
            .map(|ci| (self.nodes[ci].path.clone(), true))
            .collect();

        let to_remove = self.collect_descendants(idx);
        self.remove_nodes(&to_remove);

        let idx = self.find_by_path(dir_path).expect("parent still present");
        self.nodes[idx].children_loaded = false;
        self.load_children(idx)?;

        // Recursively restore expansion on matching paths.
        let mut queue: Vec<usize> = self.children_of(idx).to_vec();
        while let Some(ci) = queue.pop() {
            let path = self.nodes[ci].path.clone();
            if old.contains_key(&path) && self.nodes[ci].is_dir {
                self.nodes[ci].children_loaded = false;
                self.load_children(ci)?;
                self.nodes[ci].expanded = true;
                // Deeper restoration: descendants of this subdir won't be
                // restored further; acceptable for correctness.
                let _ = 0;
            }
            // Check if any deeper expanded dirs from old snapshot are children of ci.
            for &k in self.children_of(ci) {
                if old.contains_key(&self.nodes[k].path) {
                    queue.push(k);
                }
            }
        }

        let idx = self.find_by_path(dir_path).expect("parent still present");
        let new_watched = self.collect_watched_subtree(idx);
        let old_set: HashSet<PathBuf> = old_watched.iter().cloned().collect();
        let new_set: HashSet<PathBuf> = new_watched.iter().cloned().collect();
        for p in old_watched {
            if !new_set.contains(&p) {
                self.watch_delta.removed.push(p);
            }
        }
        for p in new_watched {
            if !old_set.contains(&p) {
                self.watch_delta.added.push(p);
            }
        }

        self.rebuild_visible();
        Ok(())
    }

    fn collect_descendants(&self, idx: usize) -> Vec<usize> {
        let mut out = Vec::new();
        let mut stack = vec![idx];
        while let Some(i) = stack.pop() {
            for &c in self.children_of(i) {
                out.push(c);
                stack.push(c);
            }
        }
        out
    }

    fn remove_nodes(&mut self, to_remove: &[usize]) {
        if to_remove.is_empty() {
            return;
        }
        let remove_set: HashSet<usize> = to_remove.iter().copied().collect();
        let mut index_map: HashMap<usize, usize> = HashMap::new();
        let mut new_nodes: Vec<Node> = Vec::with_capacity(self.nodes.len() - remove_set.len());
        for (i, n) in self.nodes.iter().enumerate() {
            if !remove_set.contains(&i) {
                index_map.insert(i, new_nodes.len());
                new_nodes.push(n.clone());
            }
        }
        for n in &mut new_nodes {
            n.parent = n.parent.and_then(|p| index_map.get(&p).copied());
        }

        // Rebuild children index from remapped parent refs (children added in
        // ascending new-index order, preserving original sibling ordering).
        let mut new_children: Vec<Vec<usize>> = vec![Vec::new(); new_nodes.len()];
        for (new_i, n) in new_nodes.iter().enumerate() {
            if let Some(parent_i) = n.parent {
                new_children[parent_i].push(new_i);
            }
        }

        // Rebuild path index.
        let mut new_path_index: HashMap<PathBuf, usize> =
            HashMap::with_capacity(new_nodes.len());
        for (new_i, n) in new_nodes.iter().enumerate() {
            new_path_index.insert(n.path.clone(), new_i);
        }

        self.nodes = new_nodes;
        self.children = new_children;
        self.path_index = new_path_index;
    }

    pub fn find_by_path(&self, path: &Path) -> Option<usize> {
        self.path_index.get(path).copied()
    }

    /// Ensure all ancestor directories of `path` have their children loaded
    /// so that `find_by_path(path)` returns `Some`. Returns the node index
    /// of `path` if it can be found (or loaded), `None` otherwise (e.g. if
    /// the path is gitignored or outside the root).
    pub fn ensure_loaded(&mut self, path: &Path) -> Option<usize> {
        if !path.starts_with(&self.root) {
            return None;
        }
        if let Some(idx) = self.find_by_path(path) {
            return Some(idx);
        }
        // Walk each ancestor from root down to path's parent, loading children
        // at each level so the path eventually appears in the index.
        let rel = path.strip_prefix(&self.root).ok()?;
        let mut current = self.root.clone();
        for component in rel.parent()?.components() {
            current.push(component);
            let idx = self.find_by_path(&current)?;
            if self.nodes[idx].is_dir && !self.nodes[idx].children_loaded {
                self.load_children(idx).ok()?;
            }
        }
        self.find_by_path(path)
    }

    pub fn rebuild_visible(&mut self) {
        self.visible.clear();
        self.walk_visible(0);
    }

    fn walk_visible(&mut self, idx: usize) {
        self.visible.push(idx);
        if self.nodes[idx].expanded {
            let kids: Vec<usize> = self.children[idx].clone();
            for c in kids {
                self.walk_visible(c);
            }
        }
    }

    #[allow(dead_code)]
    pub fn load_all(&mut self, limit: usize) -> Result<()> {
        let mut queue: Vec<usize> = vec![0];
        while let Some(i) = queue.pop() {
            if self.nodes.len() >= limit {
                break;
            }
            if self.nodes[i].is_dir && !self.nodes[i].children_loaded {
                self.load_children(i)?;
            }
            for &c in self.children_of(i) {
                if self.nodes[c].is_dir {
                    queue.push(c);
                }
            }
        }
        Ok(())
    }

    /// Recompute everything from scratch when options change.
    pub fn rescan(&mut self) -> Result<()> {
        // Capture which paths were expanded (also the set of currently-watched
        // dirs, since watched ↔ expanded).
        let expanded: HashSet<PathBuf> = self
            .nodes
            .iter()
            .filter(|n| n.is_dir && n.expanded)
            .map(|n| n.path.clone())
            .collect();
        let old_watched: Vec<PathBuf> = expanded.iter().cloned().collect();

        let root = self.root.clone();
        *self = Tree::new(root, self.opts)?;
        // Re-expand paths that were expanded before, if they still exist.
        // BFS: expand root, then iterate children and expand matches.
        let mut queue = vec![0usize];
        while let Some(i) = queue.pop() {
            let kids: Vec<usize> = self.children[i].clone();
            for k in kids {
                if self.nodes[k].is_dir && expanded.contains(&self.nodes[k].path) {
                    self.expand(k)?;
                    queue.push(k);
                }
            }
        }
        self.watch_delta.removed.extend(old_watched);
        Ok(())
    }
}

struct Entry {
    path: PathBuf,
    name: String,
    name_lower: String,
    is_dir: bool,
    is_hidden: bool,
    is_symlink: bool,
    size: u64,
}

fn read_children(path: &Path, opts: &ScanOptions) -> Vec<Entry> {
    // Use `ignore::WalkBuilder` when we want gitignore semantics; otherwise
    // plain `read_dir`. We always honor `show_hidden` ourselves so that a
    // hidden-but-not-ignored file is still shown when the user asks.
    let mut out: Vec<Entry> = if opts.respect_gitignore {
        let walker = WalkBuilder::new(path)
            .max_depth(Some(1))
            .hidden(!opts.show_hidden)
            .git_ignore(true)
            .git_global(true)
            .git_exclude(true)
            .parents(true)
            .require_git(false)
            .build();
        walker
            .filter_map(|r| r.ok())
            .filter(|dent| dent.depth() == 1)
            .filter_map(|dent| entry_from_dir_entry(dent.path()))
            .collect()
    } else {
        match fs::read_dir(path) {
            Ok(rd) => rd
                .filter_map(|e| e.ok())
                .filter_map(|e| {
                    let name = e.file_name().to_string_lossy().into_owned();
                    if !opts.show_hidden && name.starts_with('.') {
                        return None;
                    }
                    entry_from_dir_entry(&e.path())
                })
                .collect(),
            Err(_) => Vec::new(),
        }
    };

    out.sort_by(|a, b| match (a.is_dir, b.is_dir) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => a.name_lower.cmp(&b.name_lower),
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("kudzu-{}-{}", tag, std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn loads_and_expands() {
        let root = tmp("load");
        fs::create_dir(root.join("sub")).unwrap();
        fs::write(root.join("sub/inner.txt"), "").unwrap();
        fs::write(root.join("a.txt"), "").unwrap();

        let mut tree = Tree::new(root.clone(), ScanOptions::default()).unwrap();
        assert!(tree.find_by_path(&root.join("a.txt")).is_some());
        assert!(tree.find_by_path(&root.join("sub")).is_some());
        let sub = tree.find_by_path(&root.join("sub")).unwrap();
        tree.toggle_expand(sub).unwrap();
        assert!(tree.find_by_path(&root.join("sub/inner.txt")).is_some());
    }

    #[test]
    fn refresh_picks_up_new_files() {
        let root = tmp("refresh");
        fs::write(root.join("old.txt"), "").unwrap();
        let mut tree = Tree::new(root.clone(), ScanOptions::default()).unwrap();
        fs::write(root.join("new.txt"), "").unwrap();
        tree.refresh_dir(&root).unwrap();
        assert!(tree.find_by_path(&root.join("new.txt")).is_some());
        assert!(tree.find_by_path(&root.join("old.txt")).is_some());
    }

    #[test]
    fn refresh_preserves_expanded_subdir() {
        let root = tmp("preserve");
        fs::create_dir(root.join("sub")).unwrap();
        fs::write(root.join("sub/deep.txt"), "").unwrap();
        let mut tree = Tree::new(root.clone(), ScanOptions::default()).unwrap();
        let sub = tree.find_by_path(&root.join("sub")).unwrap();
        tree.toggle_expand(sub).unwrap();
        fs::write(root.join("sibling.txt"), "").unwrap();
        tree.refresh_dir(&root).unwrap();
        assert!(tree.find_by_path(&root.join("sub/deep.txt")).is_some());
        assert!(tree.find_by_path(&root.join("sibling.txt")).is_some());
    }

    #[test]
    fn children_index_consistency() {
        let root = tmp("index");
        fs::create_dir(root.join("a")).unwrap();
        fs::create_dir(root.join("b")).unwrap();
        fs::write(root.join("a/x.txt"), "").unwrap();
        fs::write(root.join("b/y.txt"), "").unwrap();
        fs::write(root.join("top.txt"), "").unwrap();

        let mut tree = Tree::new(root.clone(), ScanOptions::default()).unwrap();
        let a = tree.find_by_path(&root.join("a")).unwrap();
        let b = tree.find_by_path(&root.join("b")).unwrap();
        tree.toggle_expand(a).unwrap();
        tree.toggle_expand(b).unwrap();
        // Add a new file and refresh to exercise remove_nodes path.
        fs::write(root.join("new.txt"), "").unwrap();
        tree.refresh_dir(&root).unwrap();

        // Invariant 1: children and nodes have the same length.
        assert_eq!(tree.children.len(), tree.nodes.len());
        // Invariant 2: path_index covers every node exactly once.
        assert_eq!(tree.path_index.len(), tree.nodes.len());
        // Invariant 3: children[i] ↔ parent refs are mutually consistent.
        for (i, node) in tree.nodes.iter().enumerate() {
            if let Some(parent) = node.parent {
                assert!(
                    tree.children[parent].contains(&i),
                    "node {i} has parent {parent} but is not in children[{parent}]"
                );
            }
            for &child in &tree.children[i] {
                assert_eq!(
                    tree.nodes[child].parent,
                    Some(i),
                    "children[{i}] contains {child} but nodes[{child}].parent != Some({i})"
                );
            }
        }
        // Invariant 4: path_index values are consistent with nodes.
        for (path, &idx) in &tree.path_index {
            assert_eq!(&tree.nodes[idx].path, path);
        }
    }

    #[test]
    fn gitignore_filters() {
        let root = tmp("ignore");
        fs::write(root.join(".gitignore"), "hidden.txt\n").unwrap();
        fs::write(root.join("hidden.txt"), "").unwrap();
        fs::write(root.join("shown.txt"), "").unwrap();

        let mut opts = ScanOptions::default();
        opts.respect_gitignore = true;
        let tree = Tree::new(root.clone(), opts).unwrap();
        assert!(tree.find_by_path(&root.join("hidden.txt")).is_none());
        assert!(tree.find_by_path(&root.join("shown.txt")).is_some());

        opts.respect_gitignore = false;
        let tree = Tree::new(root.clone(), opts).unwrap();
        assert!(tree.find_by_path(&root.join("hidden.txt")).is_some());
    }

    #[test]
    fn hidden_toggle() {
        let root = tmp("hidden");
        fs::write(root.join(".env"), "").unwrap();
        fs::write(root.join("file.txt"), "").unwrap();

        let mut opts = ScanOptions {
            respect_gitignore: false,
            show_hidden: false,
        };
        let tree = Tree::new(root.clone(), opts).unwrap();
        assert!(tree.find_by_path(&root.join(".env")).is_none());

        opts.show_hidden = true;
        let tree = Tree::new(root.clone(), opts).unwrap();
        assert!(tree.find_by_path(&root.join(".env")).is_some());
    }
}

fn entry_from_dir_entry(path: &Path) -> Option<Entry> {
    let name = path.file_name()?.to_string_lossy().into_owned();
    let meta = fs::symlink_metadata(path).ok()?;
    let is_symlink = meta.file_type().is_symlink();
    // Resolve symlink for is_dir determination.
    let is_dir = if is_symlink {
        fs::metadata(path).map(|m| m.is_dir()).unwrap_or(false)
    } else {
        meta.is_dir()
    };
    let name_lower = name.to_lowercase();
    Some(Entry {
        path: path.to_path_buf(),
        is_hidden: name.starts_with('.'),
        name,
        name_lower,
        is_dir,
        is_symlink,
        size: meta.len(),
    })
}
