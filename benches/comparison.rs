//! Head-to-head comparison: old algorithm vs new algorithm on identical data.
//!
//! Measures the same operations the app actually performs, using synthetic
//! node data that matches realistic tree shapes.

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use nucleo_matcher::{
    pattern::{CaseMatching, Normalization, Pattern},
    Config, Matcher, Utf32Str,
};

// ── Shared synthetic data ────────────────────────────────────────────────────

#[derive(Clone)]
struct FakeNode {
    parent: Option<usize>,
    name: String,
    rel_path: String,
}

/// Build a flat synthetic tree: 1 root + `dirs` dirs each with `files` files.
/// Returns (nodes, children_index) where children_index mirrors the new Tree.children.
fn make_nodes(dirs: usize, files: usize) -> (Vec<FakeNode>, Vec<Vec<usize>>) {
    let total = 1 + dirs + dirs * files;
    let mut nodes: Vec<FakeNode> = Vec::with_capacity(total);
    let mut children: Vec<Vec<usize>> = Vec::with_capacity(total);

    // root
    nodes.push(FakeNode { parent: None, name: "root".into(), rel_path: String::new() });
    children.push(Vec::new());

    for d in 0..dirs {
        let dir_idx = nodes.len();
        nodes.push(FakeNode {
            parent: Some(0),
            name: format!("dir{:04}", d),
            rel_path: format!("dir{:04}", d),
        });
        children.push(Vec::new());
        children[0].push(dir_idx);

        for f in 0..files {
            let file_idx = nodes.len();
            let name = format!("file{:04}.rs", f);
            let rel = format!("dir{:04}/{}", d, name);
            nodes.push(FakeNode { parent: Some(dir_idx), name, rel_path: rel });
            children.push(Vec::new());
            children[dir_idx].push(file_idx);
        }
    }
    (nodes, children)
}

// ── children_of: old (O(N) scan) vs new (O(children) index) ─────────────────

fn old_children_of(nodes: &[FakeNode], idx: usize) -> Vec<usize> {
    nodes
        .iter()
        .enumerate()
        .filter(|(_, n)| n.parent == Some(idx))
        .map(|(i, _)| i)
        .collect()
}

fn new_children_of(children: &[Vec<usize>], idx: usize) -> Vec<usize> {
    children[idx].clone()
}

fn bench_children_of(c: &mut Criterion) {
    let mut group = c.benchmark_group("children_of");
    for (dirs, files) in [(10, 10), (50, 20), (100, 50)] {
        let n = 1 + dirs + dirs * files;
        let (nodes, children) = make_nodes(dirs, files);

        group.bench_with_input(BenchmarkId::new("old_linear_scan", n), &n, |b, _| {
            b.iter(|| {
                // Simulate rebuild_visible: call children_of for every node.
                for i in 0..nodes.len() {
                    let _ = old_children_of(&nodes, i);
                }
            });
        });

        group.bench_with_input(BenchmarkId::new("new_index_lookup", n), &n, |b, _| {
            b.iter(|| {
                for i in 0..children.len() {
                    let _ = new_children_of(&children, i);
                }
            });
        });
    }
    group.finish();
}

// ── search recompute: old (Pattern re-parsed + rel computed inline) vs new ───

fn old_recompute(nodes: &[FakeNode], root_prefix: &str, query: &str) -> usize {
    let pat = Pattern::parse(query, CaseMatching::Smart, Normalization::Smart);
    let mut matcher = Matcher::new(Config::DEFAULT.match_paths());
    let mut name_buf = Vec::<char>::new();
    let mut path_buf = Vec::<char>::new();
    let mut count = 0usize;
    for (i, node) in nodes.iter().enumerate() {
        if i == 0 { continue; }
        // Old: compute rel_path inline each time
        let rel = if node.rel_path.starts_with(root_prefix) {
            node.rel_path.clone()
        } else {
            node.name.clone()
        };
        let mut indices = Vec::new();
        let path_score = pat.indices(
            Utf32Str::new(&rel, &mut path_buf),
            &mut matcher,
            &mut indices,
        );
        let name_score = pat.score(
            Utf32Str::new(&node.name, &mut name_buf),
            &mut matcher,
        );
        if path_score.is_some() || name_score.is_some() {
            count += 1;
        }
    }
    count
}

fn new_recompute(
    nodes: &[FakeNode],
    query: &str,
    cached_pattern: &mut Option<(String, Pattern)>,
    path_buf: &mut Vec<char>,
    name_buf: &mut Vec<char>,
    indices_buf: &mut Vec<u32>,
) -> usize {
    let pat = match cached_pattern {
        Some((q, p)) if q == query => p,
        _ => {
            *cached_pattern = Some((
                query.to_string(),
                Pattern::parse(query, CaseMatching::Smart, Normalization::Smart),
            ));
            &cached_pattern.as_ref().unwrap().1
        }
    };
    let mut matcher = Matcher::new(Config::DEFAULT.match_paths());
    let mut count = 0usize;
    for (i, node) in nodes.iter().enumerate() {
        if i == 0 { continue; }
        indices_buf.clear();
        let path_score = pat.indices(
            Utf32Str::new(&node.rel_path, path_buf),
            &mut matcher,
            indices_buf,
        );
        let name_score = pat.score(
            Utf32Str::new(&node.name, name_buf),
            &mut matcher,
        );
        if path_score.is_some() || name_score.is_some() {
            count += 1;
        }
    }
    count
}

fn bench_search_recompute(c: &mut Criterion) {
    let mut group = c.benchmark_group("search_recompute_comparison");
    for (dirs, files) in [(20, 50), (100, 50)] {
        let n = 1 + dirs + dirs * files;
        let (nodes, _) = make_nodes(dirs, files);
        let query = "file0042";

        group.bench_with_input(BenchmarkId::new("old_reparse_each_call", n), &n, |b, _| {
            b.iter(|| old_recompute(&nodes, "", query));
        });

        group.bench_with_input(BenchmarkId::new("new_cached_pattern", n), &n, |b, _| {
            let mut cached: Option<(String, Pattern)> = None;
            let mut pb = Vec::new();
            let mut nb = Vec::new();
            let mut ib = Vec::new();
            b.iter(|| new_recompute(&nodes, query, &mut cached, &mut pb, &mut nb, &mut ib));
        });
    }
    group.finish();
}

criterion_group!(benches, bench_children_of, bench_search_recompute);
criterion_main!(benches);
