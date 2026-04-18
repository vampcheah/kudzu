use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use crossbeam_channel::unbounded;
use kudzu::{
    event::AppEvent,
    search::Search,
    tree::ScanOptions,
};
use std::{fs, path::PathBuf, time::{Duration, Instant}};

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

/// Wait until indexing finishes (IndexDone) or timeout.
fn wait_indexed(s: &mut Search, rx: &crossbeam_channel::Receiver<AppEvent>, timeout_ms: u64) {
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    while s.indexing && Instant::now() < deadline {
        let _ = rx.recv_timeout(Duration::from_millis(20));
        s.tick();
    }
}

fn bench_tick_after_index(c: &mut Criterion) {
    let mut group = c.benchmark_group("search_tick");
    let root = tmp("search");
    make_flat_tree(&root, 20, 50); // 1 + 20 + 20*50 = 1021 files

    for query in ["r", "file", "file0042", "dir0010/file0042"] {
        let (tx, rx) = unbounded::<AppEvent>();
        let mut s = Search::new(tx);
        s.start_indexing(root.clone(), ScanOptions::default());
        wait_indexed(&mut s, &rx, 5000);
        s.set_query(query);
        s.tick(); // prime

        group.bench_with_input(
            BenchmarkId::new("query", query),
            query,
            |b, q| {
                b.iter(|| {
                    // Simulate the main loop: set query, tick, count results.
                    s.set_query(q);
                    s.tick();
                    criterion::black_box(s.matches.len());
                });
            },
        );
    }
    group.finish();
    let _ = fs::remove_dir_all(&root);
}

criterion_group!(benches, bench_tick_after_index);
criterion_main!(benches);
