use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
};

use crossbeam_channel::Sender;
use ignore::WalkBuilder;
use nucleo::pattern::{CaseMatching, Normalization};
use nucleo::{Config, Matcher, Nucleo, Utf32Str};

use crate::event::AppEvent;
use crate::tree::ScanOptions;

/// Data stored per item inside the nucleo index.
pub struct NodeHandle {
    pub generation: u64,
    pub name: String,
    pub rel_path: String,
    pub parent_rel: String,
    pub is_dir: bool,
    pub is_hidden: bool,
    pub is_symlink: bool,
    pub path: PathBuf,
}

/// A pre-rendered snapshot of a single nucleo match, rebuilt on every tick.
pub struct SearchMatch {
    pub indices: Vec<u32>,
    pub parent_rel: String,
    pub name: String,
    pub is_dir: bool,
    pub is_hidden: bool,
    pub is_symlink: bool,
    pub path: PathBuf,
}

pub struct Search {
    pub query: String,
    pub selected: usize,
    /// True while the background walker thread is still running.
    pub indexing: bool,
    /// Pre-built list of matches for this frame (rebuilt by tick()).
    pub matches: Vec<SearchMatch>,
    nucleo: Nucleo<NodeHandle>,
    cancel: Arc<AtomicBool>,
    /// Reusable fuzzy matcher for computing highlight indices.
    matcher: Matcher,
    /// Scratch buffer for highlight indices during rebuild.
    indices_buf: Vec<u32>,
    tx: Sender<AppEvent>,
    generation: u64,
}

impl Search {
    pub fn new(tx: Sender<AppEvent>) -> Self {
        let notify_tx = tx.clone();
        let notify: Arc<dyn Fn() + Sync + Send> = Arc::new(move || {
            notify_tx.send(AppEvent::SearchUpdate).ok();
        });
        let workers = thread::available_parallelism()
            .map(|n| n.get().clamp(2, 4))
            .unwrap_or(2) as u32;
        let nucleo = Nucleo::new(Config::DEFAULT.match_paths(), notify, None, workers);
        Self {
            query: String::new(),
            selected: 0,
            indexing: false,
            matches: Vec::new(),
            nucleo,
            cancel: Arc::new(AtomicBool::new(false)),
            matcher: Matcher::new(Config::DEFAULT.match_paths()),
            indices_buf: Vec::new(),
            tx,
            generation: 0,
        }
    }

    /// Cancel any running indexer, clear nucleo state, and start a fresh walk.
    pub fn start_indexing(&mut self, root: PathBuf, opts: ScanOptions) -> u64 {
        self.cancel.store(true, Ordering::Relaxed);
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel = cancel.clone();
        self.indexing = true;
        self.matches.clear();
        self.selected = 0;
        self.nucleo.restart(true);

        let injector = self.nucleo.injector();
        let tx = self.tx.clone();
        thread::spawn(move || {
            let walker = WalkBuilder::new(&root)
                .hidden(!opts.show_hidden)
                .git_ignore(opts.respect_gitignore)
                .git_global(opts.respect_gitignore)
                .git_exclude(opts.respect_gitignore)
                .parents(opts.respect_gitignore)
                .require_git(false)
                .build();

            for result in walker {
                if cancel.load(Ordering::Relaxed) {
                    return;
                }
                let entry = match result {
                    Ok(e) => e,
                    Err(_) => continue,
                };
                if entry.depth() == 0 {
                    continue; // skip root
                }
                let path = entry.path().to_path_buf();
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let rel_path = path
                    .strip_prefix(&root)
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_else(|_| name.clone());
                let parent_rel = path
                    .strip_prefix(&root)
                    .ok()
                    .and_then(|p| p.parent())
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let ft = entry.file_type();
                let is_dir = ft.map(|t| t.is_dir()).unwrap_or(false);
                let is_symlink = ft.map(|t| t.is_symlink()).unwrap_or(false);
                let is_hidden = name.starts_with('.');
                let handle = NodeHandle {
                    generation,
                    name,
                    rel_path,
                    parent_rel,
                    is_dir,
                    is_hidden,
                    is_symlink,
                    path,
                };
                injector.push(handle, |h, cols| {
                    cols[0] = h.name.as_str().into();
                    cols[1] = h.rel_path.as_str().into();
                });
            }
            drop(injector);
            tx.send(AppEvent::IndexDone(generation)).ok();
        });
        generation
    }

    /// Update the search query and reparse the nucleo pattern.
    pub fn set_query(&mut self, new_query: &str) {
        let append = new_query.starts_with(self.query.as_str());
        self.query = new_query.to_string();
        for col in 0..2 {
            self.nucleo.pattern.reparse(
                col,
                new_query,
                CaseMatching::Smart,
                Normalization::Smart,
                append,
            );
        }
        self.selected = 0;
    }

    /// Modify the current query in-place without an extra clone.
    pub fn mutate_query(&mut self, f: impl FnOnce(&mut String)) {
        let mut q = std::mem::take(&mut self.query);
        f(&mut q);
        self.set_query(&q);
    }

    /// Drive the nucleo worker for up to 10ms; returns true if the snapshot changed.
    pub fn tick(&mut self) -> bool {
        let changed = self.nucleo.tick(10).changed;
        if changed {
            self.rebuild_matches();
        }
        changed
    }

    pub fn selected_match(&self) -> Option<&SearchMatch> {
        self.matches.get(self.selected)
    }

    pub fn move_selection(&mut self, delta: isize) {
        let len = self.matches.len() as isize;
        if len == 0 {
            return;
        }
        let new = (self.selected as isize + delta).clamp(0, len - 1);
        self.selected = new as usize;
    }

    pub fn cancel_indexing(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn current_generation(&self) -> u64 {
        self.generation
    }

    /// Total number of items in the nucleo index (not just matches).
    pub fn nucleo_item_count(&self) -> u32 {
        self.nucleo.snapshot().item_count()
    }

    /// Rebuild self.matches from the current nucleo snapshot.
    fn rebuild_matches(&mut self) {
        self.matches.clear();
        if self.query.is_empty() {
            return;
        }

        // Phase 1: clone raw data out of the snapshot (releases the borrow before
        // we need &mut self.matcher and &mut self.indices_buf).
        let pat = self.nucleo.pattern.column_pattern(0).clone();
        let raw: Vec<(String, String, bool, bool, bool, PathBuf)> = {
            let snapshot = self.nucleo.snapshot();
            let count = (snapshot.matched_item_count() as usize).min(5000);
            (0..count as u32)
                .filter_map(|i| {
                    let item = snapshot.get_matched_item(i)?;
                    let h = item.data;
                    (h.generation == self.generation).then(|| {
                        (
                            h.name.clone(),
                            h.parent_rel.clone(),
                            h.is_dir,
                            h.is_hidden,
                            h.is_symlink,
                            h.path.clone(),
                        )
                    })
                })
                .collect()
        };

        // Phase 2: compute highlight indices with the cloned pattern.
        let mut name_buf = Vec::new();
        for (name, parent_rel, is_dir, is_hidden, is_symlink, path) in raw {
            self.indices_buf.clear();
            name_buf.clear();
            pat.indices(
                Utf32Str::new(&name, &mut name_buf),
                &mut self.matcher,
                &mut self.indices_buf,
            );
            let indices = self.indices_buf.clone();
            self.matches.push(SearchMatch {
                indices,
                parent_rel,
                name,
                is_dir,
                is_hidden,
                is_symlink,
                path,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossbeam_channel::unbounded;
    use std::{
        fs,
        path::PathBuf,
        time::{Duration, Instant},
    };

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("kudzu-search-{}-{}", tag, std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn fuzzy_finds_nested_file() {
        let root = tmp("nested");
        fs::create_dir_all(root.join("src/deep")).unwrap();
        fs::write(root.join("src/deep/widget.rs"), "").unwrap();
        fs::write(root.join("README.md"), "").unwrap();

        let (tx, rx) = unbounded::<AppEvent>();
        let mut s = Search::new(tx);
        s.start_indexing(root.clone(), ScanOptions::default());
        s.set_query("widget");

        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            s.tick();
            if !s.matches.is_empty() {
                break;
            }
            if Instant::now() > deadline {
                break;
            }
            let _ = rx.recv_timeout(Duration::from_millis(10));
        }
        assert!(!s.matches.is_empty(), "should find widget.rs");
        let m = s.selected_match().expect("should have a selection");
        assert!(m.path.ends_with("widget.rs"));
    }

    #[test]
    fn indexing_generation_increments() {
        let root = tmp("generation");
        fs::write(root.join("file.txt"), "").unwrap();
        let (tx, _rx) = unbounded::<AppEvent>();
        let mut s = Search::new(tx);

        let first = s.start_indexing(root.clone(), ScanOptions::default());
        let second = s.start_indexing(root, ScanOptions::default());

        assert_ne!(first, second);
        assert_eq!(s.current_generation(), second);
    }

    #[test]
    fn empty_query_yields_no_matches() {
        let (tx, _rx) = unbounded::<AppEvent>();
        let s = Search::new(tx);
        assert_eq!(s.matches.len(), 0);
    }
}
