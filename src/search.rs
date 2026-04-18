use nucleo_matcher::{
    pattern::{CaseMatching, Normalization, Pattern},
    Config, Matcher, Utf32Str,
};

use crate::tree::Tree;

pub struct SearchMatch {
    pub node: usize,
    pub score: u32,
    pub indices: Vec<u32>,
    /// Parent directory relative to tree root, for display.
    pub parent_rel: String,
}

pub struct Search {
    pub query: String,
    pub matches: Vec<SearchMatch>,
    pub selected: usize,
    matcher: Matcher,
    /// Cached parsed pattern — re-parsed only when `query` changes.
    cached_pattern: Option<(String, Pattern)>,
    /// Reusable char buffers for nucleo to avoid per-node allocation.
    path_buf: Vec<char>,
    name_buf: Vec<char>,
    indices_buf: Vec<u32>,
}

impl Search {
    pub fn new() -> Self {
        Self {
            query: String::new(),
            matches: Vec::new(),
            selected: 0,
            matcher: Matcher::new(Config::DEFAULT.match_paths()),
            cached_pattern: None,
            path_buf: Vec::new(),
            name_buf: Vec::new(),
            indices_buf: Vec::new(),
        }
    }

    pub fn recompute(&mut self, tree: &Tree) {
        self.matches.clear();
        self.selected = 0;
        if self.query.is_empty() {
            return;
        }

        // Re-parse pattern only when query changes.
        let query = self.query.clone();
        let pat = match &self.cached_pattern {
            Some((q, p)) if q == &query => p,
            _ => {
                self.cached_pattern = Some((
                    query.clone(),
                    Pattern::parse(&query, CaseMatching::Smart, Normalization::Smart),
                ));
                &self.cached_pattern.as_ref().unwrap().1
            }
        };

        for (i, node) in tree.nodes.iter().enumerate() {
            if i == 0 {
                continue; // skip root
            }

            // Run indices (which also gives score) on the full rel-path — the
            // path signal helps match nested files by directory fragment.
            self.indices_buf.clear();
            let path_score = pat.indices(
                Utf32Str::new(&node.rel_path, &mut self.path_buf),
                &mut self.matcher,
                &mut self.indices_buf,
            );

            // Also score the bare name so a direct name match wins over a
            // distant path match, but reuse a separate buf without indices
            // (cheaper than a second indices call).
            let name_score = pat.score(
                Utf32Str::new(&node.name, &mut self.name_buf),
                &mut self.matcher,
            );

            let score = match (path_score, name_score) {
                (Some(p), Some(n)) => Some(p.max(n)),
                (Some(p), None) => Some(p),
                (None, Some(n)) => Some(n),
                (None, None) => None,
            };

            if let Some(s) = score {
                let name_indices =
                    map_indices_to_name_suffix(&node.rel_path, &node.name, &self.indices_buf);
                let parent_rel = node
                    .path
                    .strip_prefix(&tree.root)
                    .ok()
                    .and_then(|p| p.parent())
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_default();
                self.matches.push(SearchMatch {
                    node: i,
                    score: s,
                    indices: name_indices,
                    parent_rel,
                });
            }
        }
        self.matches.sort_by(|a, b| b.score.cmp(&a.score));
        self.matches.truncate(5000);
    }

    pub fn selected_node(&self) -> Option<usize> {
        self.matches.get(self.selected).map(|m| m.node)
    }

    pub fn move_selection(&mut self, delta: isize) {
        let len = self.matches.len() as isize;
        if len == 0 {
            return;
        }
        let new = (self.selected as isize + delta).clamp(0, len - 1);
        self.selected = new as usize;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tree::{ScanOptions, Tree};
    use std::{fs, path::PathBuf};

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

        let mut tree = Tree::new(root.clone(), ScanOptions::default()).unwrap();
        tree.load_all(10_000).unwrap();

        let mut s = Search::new();
        s.query = "widget".to_string();
        s.recompute(&tree);
        assert!(!s.matches.is_empty());
        let top = s.matches[0].node;
        assert!(tree.nodes[top].path.ends_with("widget.rs"));
    }

    #[test]
    fn empty_query_yields_no_matches() {
        let root = tmp("empty");
        fs::write(root.join("x.txt"), "").unwrap();
        let tree = Tree::new(root, ScanOptions::default()).unwrap();
        let mut s = Search::new();
        s.recompute(&tree);
        assert!(s.matches.is_empty());
    }

    #[test]
    fn pattern_cached_across_recompute() {
        let root = tmp("cache");
        fs::write(root.join("foo.rs"), "").unwrap();
        let tree = Tree::new(root, ScanOptions::default()).unwrap();
        let mut s = Search::new();
        s.query = "foo".to_string();
        s.recompute(&tree);
        let first_count = s.matches.len();
        s.recompute(&tree); // second call reuses cached pattern
        assert_eq!(s.matches.len(), first_count);
    }
}

/// Convert char indices in the full rel-path string into indices within the
/// name suffix — so highlights align with what the UI renders.
fn map_indices_to_name_suffix(rel: &str, name: &str, indices: &[u32]) -> Vec<u32> {
    let name_chars_len = name.chars().count();
    let rel_chars_len = rel.chars().count();
    if name_chars_len == 0 || rel_chars_len < name_chars_len {
        return Vec::new();
    }
    let name_start = rel_chars_len - name_chars_len;
    indices
        .iter()
        .filter_map(|&i| {
            let i = i as usize;
            if i >= name_start && i < rel_chars_len {
                Some((i - name_start) as u32)
            } else {
                None
            }
        })
        .collect()
}
