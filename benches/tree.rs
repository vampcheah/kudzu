use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use kudzu::tree::{ScanOptions, Tree};
use std::{fs, path::PathBuf};

fn tmp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("kudzu-bench-{}-{}", tag, std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

/// Create a directory tree: `dirs` top-level subdirs each containing `files_per_dir` files.
fn make_flat_tree(root: &PathBuf, dirs: usize, files_per_dir: usize) {
    for d in 0..dirs {
        let dir = root.join(format!("dir{:04}", d));
        fs::create_dir_all(&dir).unwrap();
        for f in 0..files_per_dir {
            fs::write(dir.join(format!("file{:04}.txt", f)), "").unwrap();
        }
    }
}

fn bench_rebuild_visible(c: &mut Criterion) {
    let mut group = c.benchmark_group("rebuild_visible");
    for (dirs, files) in [(10, 10), (20, 50), (40, 50)] {
        let total = dirs * files + dirs + 1;
        let root = tmp(&format!("rebuild-{}", total));
        make_flat_tree(&root, dirs, files);
        let mut tree = Tree::new(root.clone(), ScanOptions::default()).unwrap();
        tree.load_all(100_000).unwrap();
        // Expand all dirs so rebuild_visible has real work to do.
        let indices: Vec<usize> = (0..tree.nodes.len())
            .filter(|&i| tree.nodes[i].is_dir)
            .collect();
        for i in indices {
            tree.expand(i).unwrap();
        }

        group.bench_with_input(
            BenchmarkId::from_parameter(total),
            &total,
            |b, _| {
                b.iter(|| {
                    tree.rebuild_visible();
                });
            },
        );
        let _ = fs::remove_dir_all(&root);
    }
    group.finish();
}

fn bench_load_all(c: &mut Criterion) {
    let mut group = c.benchmark_group("load_all");
    for (dirs, files) in [(10, 10), (20, 50)] {
        let total = dirs * files + dirs + 1;
        let root = tmp(&format!("loadall-{}", total));
        make_flat_tree(&root, dirs, files);

        group.bench_with_input(
            BenchmarkId::from_parameter(total),
            &total,
            |b, _| {
                b.iter(|| {
                    let mut tree = Tree::new(root.clone(), ScanOptions::default()).unwrap();
                    tree.load_all(100_000).unwrap();
                    tree
                });
            },
        );
        let _ = fs::remove_dir_all(&root);
    }
    group.finish();
}

fn bench_refresh_dir(c: &mut Criterion) {
    let root = tmp("refresh");
    make_flat_tree(&root, 5, 100);
    let mut tree = Tree::new(root.clone(), ScanOptions::default()).unwrap();
    tree.load_all(100_000).unwrap();

    c.bench_function("refresh_dir_500", |b| {
        b.iter(|| {
            tree.refresh_dir(&root).unwrap();
        });
    });
    let _ = fs::remove_dir_all(&root);
}

criterion_group!(benches, bench_rebuild_visible, bench_load_all, bench_refresh_dir);
criterion_main!(benches);
