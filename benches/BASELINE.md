# Benchmark Baseline

## v1.2.0 — async nucleo search + borrowed Span rendering + cleanup

Machine: Linux 6.17 · release profile (`lto = "thin"`, `codegen-units = 1`).
Bench corpus: synthetic flat tree — N dirs × M files each.

### search_tick (1021-node pool: 20 dirs × 50 files, already indexed)

`tick()` cost = pattern reparse + nucleo re-match + match list rebuild (clones visible data for rendering).

| query                   | time (median) | matches |
|-------------------------|---------------|---------|
| `r`                     | 578 µs        | many    |
| `file`                  | 450 µs        | many    |
| `file0042`              | 29 µs         | few     |
| `dir0010/file0042`      | 1.7 µs        | 1       |

Compared to v1.1 synchronous `recompute` on the same pool:

| query                   | v1.1 recompute | v1.2 tick | note                        |
|-------------------------|----------------|-----------|-----------------------------|
| `r`                     | 143 µs         | 578 µs    | more overhead from cloning  |
| `file`                  | 202 µs         | 450 µs    |                             |
| `file0042`              | 61 µs          | 29 µs     | fewer matches → less cloning|
| `dir0010/file0042`      | 37 µs          | 1.7 µs    | 22× faster (nucleo path idx)|

**Key v1.2 benefit**: `tick()` runs on the main thread but is O(matches), not O(total items). The actual fuzzy matching runs on background threads. Pressing `/` no longer blocks the UI — indexing starts immediately and results stream in.

### rebuild_visible (all dirs expanded)

| nodes | time    | scaling vs 111 nodes |
|-------|---------|----------------------|
| 111   | 219 ns  | 1×                   |
| 1021  | 1.68 µs | 7.7× (node count 9.2×)|
| 2041  | 3.28 µs | 15× (node count 18.4×)|

Linear scaling confirmed (unchanged from v1.1).

### load_all

| nodes | time    |
|-------|---------|
| 111   | 517 µs  |
| 1021  | 1.95 ms |

Dominated by filesystem IO; not an algorithmic concern.

### refresh_dir (500 entries)

| bench           | time  |
|-----------------|-------|
| refresh_dir_500 | 55 µs |

## v1.1.0 baseline (Phase 1–3 optimisations)

Recorded after children index, path index, Pattern cache,
rel_path pre-compute, event-driven redraw, highlighted_name pointer walk, OnceLock help width.

### search_recompute (1021-node pool)

| query            | time (median) |
|------------------|---------------|
| `r`              | 143 µs        |
| `file`           | 202 µs        |
| `file0042`       | 61 µs         |
| `dir0010/file0042` | 37 µs       |
