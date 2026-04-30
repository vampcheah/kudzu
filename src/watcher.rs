use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::Result;
use crossbeam_channel::Sender;
use notify::{RecursiveMode, Watcher};
use notify_debouncer_full::{DebounceEventResult, Debouncer, FileIdMap, new_debouncer};

use crate::event::AppEvent;

pub struct FsWatcher {
    debouncer: Debouncer<notify::RecommendedWatcher, FileIdMap>,
    watching: HashSet<PathBuf>,
}

impl FsWatcher {
    pub fn new(tx: Sender<AppEvent>) -> Result<Self> {
        let debouncer = new_debouncer(
            Duration::from_millis(150),
            None,
            move |result: DebounceEventResult| match result {
                Ok(events) => {
                    let mut paths: HashSet<PathBuf> = HashSet::new();
                    for ev in events {
                        for p in &ev.paths {
                            // Report the parent directory — that's what we need to re-read.
                            if let Some(parent) = p.parent() {
                                paths.insert(parent.to_path_buf());
                            }
                        }
                    }
                    if !paths.is_empty() {
                        let _ = tx.send(AppEvent::FsChanged(paths.into_iter().collect()));
                    }
                }
                Err(errs) => {
                    eprintln!("watcher errors: {:?}", errs);
                }
            },
        )?;
        Ok(Self {
            debouncer,
            watching: HashSet::new(),
        })
    }

    /// Register a non-recursive watch on `path`. Idempotent — repeated calls
    /// with the same path are a no-op so callers can safely replay deltas.
    pub fn watch_dir(&mut self, path: &Path) -> Result<()> {
        if self.watching.contains(path) {
            return Ok(());
        }
        self.debouncer
            .watcher()
            .watch(path, RecursiveMode::NonRecursive)?;
        self.watching.insert(path.to_path_buf());
        Ok(())
    }

    /// Remove a watch. Silent if the path isn't currently watched or the
    /// underlying directory has already been deleted.
    pub fn unwatch_dir(&mut self, path: &Path) {
        if self.watching.remove(path) {
            let _ = self.debouncer.watcher().unwatch(path);
        }
    }

    /// Drop every active watch. Used when swapping the tree root.
    pub fn unwatch_all(&mut self) {
        for p in std::mem::take(&mut self.watching) {
            let _ = self.debouncer.watcher().unwatch(&p);
        }
    }
}
