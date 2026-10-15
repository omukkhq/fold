# Benchmarks

`make bench` measures the five runtime budgets against a real `fld serve` process and a seeded 10 000-file, 200 MB repository, and exits non-zero when a target misses its limit. `make bench-smoke` runs the same harness on a fixture 100 times smaller and does not enforce the limits. `FOLD_BENCH_LIMIT_SCALE=<factor>` multiplies every limit, for example `FOLD_BENCH_LIMIT_SCALE=3 make bench` on a slower device, and `FOLD_BENCH_DIR=<dir>` picks the disk. Each run writes `target/bench/results.json` and `target/bench/results.md` with the host description, the scale and the fold version.

## Development computer

- Name: pptx704-B13VEK
- CPU: 13th Gen Intel Core i7-13620H, 16 logical cores
- Memory: 15.3 GiB
- Operating system: Zorin OS 18, kernel 7.0.0-38-generic

## Results

The table is the full run (`make bench`, the 10 000-file fixture of 200 MB) on the development computer, at scale 1.0, on 2026-10-15, while other fold builds ran on the same machine.

| Target | Value | Unit | Limit | Scale | Pass |
|---|---|---|---|---|---|
| idle_memory | 33726464 | bytes | 60000000 | 1 | yes |
| workspace_creation | 16.582 | s | 20.000 | 1 | yes |
| full_stat_scan | 0.099 | s | 2.000 | 1 | yes |
| change_visibility | 0.504 | s | 1.000 | 1 | yes |
| integration | 0.348 | s | 2.000 | 1 | yes |
