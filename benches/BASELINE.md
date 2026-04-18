# Benchmark Baseline

Recorded after Phase 1–3 optimisations (children index, path index, Pattern cache,
rel_path pre-compute, event-driven redraw, highlighted_name pointer walk, OnceLock help width).

Machine: Linux 6.17 · release profile (`lto = "thin"`, `codegen-units = 1`).
Bench corpus: synthetic flat tree — N dirs × M files each.

## search_recompute (1021-node pool: 20 dirs × 50 files)

| query            | time (median) | notes                       |
|------------------|---------------|-----------------------------|
| `r`              | 143 µs        | 1-char, many matches        |
| `file`           | 202 µs        | 4-char, many matches        |
| `file0042`       | 61 µs         | 8-char, few matches         |
| `dir0010/file0042` | 37 µs       | path query, 1 match         |
| `file0042` ×2 (cached pattern) | 63 µs | second call reuses Pattern |

## rebuild_visible (all dirs expanded)

| nodes | time   | scaling vs 111 nodes |
|-------|--------|----------------------|
| 111   | 219 ns | 1×                   |
| 1021  | 1.68 µs | 7.7× (node count 9.2×) |
| 2041  | 3.28 µs | 15× (node count 18.4×) |

Scaling is linear in node count — confirms O(N) after children-index refactor.

## load_all

| nodes | time    |
|-------|---------|
| 111   | 517 µs  |
| 1021  | 1.95 ms |

Dominated by filesystem IO (`WalkBuilder`); not an algorithmic concern.

## refresh_dir (500 entries)

| bench           | time  |
|-----------------|-------|
| refresh_dir_500 | 55 µs |
