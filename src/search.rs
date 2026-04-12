use nucleo_matcher::{
    pattern::{CaseMatching, Normalization, Pattern},
    Config, Matcher, Utf32Str,
};

use crate::tree::Tree;

pub struct SearchMatch {
    pub node: usize,
    pub score: u32,
    pub indices: Vec<u32>,
}

pub struct Search {
    pub query: String,
    pub matches: Vec<SearchMatch>,
    pub selected: usize,
    matcher: Matcher,
}

impl Search {
    pub fn new() -> Self {
        Self {
            query: String::new(),
            matches: Vec::new(),
            selected: 0,
            matcher: Matcher::new(Config::DEFAULT.match_paths()),
        }
    }

    pub fn recompute(&mut self, tree: &Tree) {
        self.matches.clear();
        self.selected = 0;
        if self.query.is_empty() {
            return;
        }
        let pat = Pattern::parse(&self.query, CaseMatching::Smart, Normalization::Smart);
        let mut name_buf: Vec<char> = Vec::new();
        let mut path_buf: Vec<char> = Vec::new();
        for (i, node) in tree.nodes.iter().enumerate() {
            if i == 0 {
                continue; // skip root
            }
            // Match against both the bare name and a path fragment relative
            // to root — gives the fuzzy matcher useful signal for nested
            // directories.
            let rel = node
                .path
                .strip_prefix(&tree.root)
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_else(|_| node.name.clone());
            let mut indices = Vec::new();
            let path_score = pat.indices(
                Utf32Str::new(&rel, &mut path_buf),
                &mut self.matcher,
                &mut indices,
            );
            let name_score = pat.score(
                Utf32Str::new(&node.name, &mut name_buf),
                &mut self.matcher,
            );
            let score = match (path_score, name_score) {
                (Some(p), Some(n)) => Some(p.max(n)),
                (Some(p), None) => Some(p),
                (None, Some(n)) => Some(n),
                (None, None) => None,
            };
            if let Some(s) = score {
                // Remap indices from the rel-path string to the node.name
                // suffix — a highlight aligned to what's displayed.
                let name_indices = map_indices_to_name_suffix(&rel, &node.name, &indices);
                self.matches.push(SearchMatch {
                    node: i,
                    score: s,
                    indices: name_indices,
                });
            }
        }
        self.matches.sort_by(|a, b| b.score.cmp(&a.score));
        // Cap to avoid massive render cost on huge trees.
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
}

/// The matcher operated on the full relative path; the UI displays the
/// leaf name. Convert char indices (in rel) that fall inside the name
/// segment into char indices (in name).
fn map_indices_to_name_suffix(rel: &str, name: &str, indices: &[u32]) -> Vec<u32> {
    let rel_chars: Vec<char> = rel.chars().collect();
    let name_chars_len = name.chars().count();
    if name_chars_len == 0 || rel_chars.len() < name_chars_len {
        return Vec::new();
    }
    let name_start = rel_chars.len() - name_chars_len;
    indices
        .iter()
        .filter_map(|&i| {
            let i = i as usize;
            if i >= name_start && i < rel_chars.len() {
                Some((i - name_start) as u32)
            } else {
                None
            }
        })
        .collect()
}
