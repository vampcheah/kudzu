use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use kudzu::{
    search::Search,
    tree::{ScanOptions, Tree},
};
use std::{fs, path::PathBuf};

fn tmp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("kudzu-sbench-{}-{}", tag, std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

fn make_flat_tree(root: &PathBuf, dirs: usize, files_per_dir: usize) {
    for d in 0..dirs {
        let dir = root.join(format!("dir{:04}", d));
        fs::create_dir_all(&dir).unwrap();
        for f in 0..files_per_dir {
            fs::write(dir.join(format!("file{:04}.rs", f)), "").unwrap();
        }
    }
}

fn bench_search_recompute(c: &mut Criterion) {
    let mut group = c.benchmark_group("search_recompute");
    let root = tmp("search");
    make_flat_tree(&root, 20, 50);
    let mut tree = Tree::new(root.clone(), ScanOptions::default()).unwrap();
    tree.load_all(100_000).unwrap();

    for query in ["r", "file", "file0042", "dir0010/file0042"] {
        group.bench_with_input(
            BenchmarkId::new("query", query),
            query,
            |b, q| {
                let mut s = Search::new();
                s.query = q.to_string();
                b.iter(|| {
                    s.recompute(&tree);
                });
            },
        );
    }
    group.finish();
    let _ = fs::remove_dir_all(&root);
}

fn bench_search_pattern_cache(c: &mut Criterion) {
    let root = tmp("cache");
    make_flat_tree(&root, 20, 50);
    let mut tree = Tree::new(root.clone(), ScanOptions::default()).unwrap();
    tree.load_all(100_000).unwrap();

    // Measure repeated recomputes with the same query (exercises pattern cache).
    c.bench_function("search_same_query_cached", |b| {
        let mut s = Search::new();
        s.query = "file0042".to_string();
        b.iter(|| {
            s.recompute(&tree);
        });
    });
    let _ = fs::remove_dir_all(&root);
}

criterion_group!(benches, bench_search_recompute, bench_search_pattern_cache);
criterion_main!(benches);
