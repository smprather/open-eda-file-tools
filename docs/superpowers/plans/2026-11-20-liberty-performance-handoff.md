# Liberty performance implementation handoff

The working tree contains Tasks 1–14 from the [implementation plan](2026-11-20-liberty-performance.md).
The comparison baseline is `ed884cc62445cb6222a7d80c715f23487ffbfa85`.
The verified candidate is the Liberty change set in this checkout. The managed
sandbox mounts the original Git metadata read-only, so implementation stayed in
the current checkout after Git rejected worktree creation.

## Release benchmark results

Five measured runs per timing, after one warmup. Both builds used the Rust
release profile, CPython 3.14.10, and Polars 2.1.0 on the same Linux machine.
Memory values use fresh workers. The optional IHP input was the 1,720,231-byte
SG13G2 typical 1.20 V, 25 °C library with 84 cells.

| Workload | Baseline → candidate |
| --- | ---: |
| IHP full parse, plain | 25.18 → 18.57 ms |
| IHP full parse, gzip of the same bytes | 69.44 → 19.21 ms |
| Select first / last / absent cell on IHP plain | 19.75 → 7.51 / 19.76 → 7.34 / 20.02 → 7.04 ms |
| IHP index open / structural sequential classification | 1.26 → 1.60 / 16.33 → 9.69 ms |
| Indexed Rust cell attributes, IHP, open excluded | 14.00 → 9.47 ms |
| All-cell area lookup, 4k / 8k / 16k generated cells, parse excluded | 25.79 → 1.18 / 103.22 → 2.38 / 402.35 → 4.85 ms |
| `dev.lib` DFF tree | 70.25 → 1.79 ms and 29 → 1 native parses |
| 16k-cell viewer index, repeated filtered page | 1.10 → 0.61 ms |
| IHP timing frame, 52,442 rows | 143.04 → 8.09 ms, 128.0 → 33.9 MiB additional peak RSS |
| 2,000 cells × 49 timing points | 241.10 → 12.14 ms, 219.2 → 47.6 MiB additional peak RSS |
| Reindent 100k one-input cells | 605.62 → 636.50 ms, 57.8 → 66.9 MiB process peak RSS |
| Token verification on the same 100k cells | 532.56 → 151.71 ms |
| Formatter CLI on the same 100k cells | 1144.53 → 797.00 ms, 376.4 → 30.6 MiB process peak RSS |
| Comparison on `dev.lib`, 3,912 entries | 18.43 → 6.54 ms |
| Equal grid with 1,024 axis points | 109.49 → 0.96 ms |
| 128 reversed seven-input minterms | 313.58 → 5.58 ms |
| 12-input implication, one / 16 repeats | 1.57 → 0.31 / 29.90 → 0.31 ms |
| Bulk render with 4,000 edited cells and 2 KiB comments | 287.81 → 42.32 ms, 54.3 → 59.7 MiB process peak RSS |
| Source annotation, 2k / 4k / 8k sibling vectors | 10.88 → 6.09 / 33.86 → 10.14 / 170.33 → 20.84 ms |
| Combo builder, 16 gzip passes on `dev.lib`, level 6 | 139.81 → 124.27 ms |
| `def-tools instances`, 1,000 generated DEF instances with IHP Liberty | 103.64 → 92.02 ms including process startup |

The standalone Rust attribute harness returned the same ordered attribute and
sequential results on IHP. Its index-open median was 1.42 → 1.83 ms. The
full-role and flag-only classifiers agreed on all 84 cells, with 14 sequential.
The DEF dump produced identical CSV. A separate 4,000-edit run within one cell
fell from 227.5 ms with the original inner replacement loop to 44.2 ms with
forward assembly. It produced the expected bytes.

Both viewer trees and all `dev.lib` comparison entries matched the saved
baseline in a 1,350,833-byte exact output comparison. The buffer tree used five
native parses in the baseline and one in the candidate. The concurrency test
observed one load across four callers and confirmed retry after a failed load.
Formatter, annotation, writer, and decompressed combo outputs matched their
baseline checksums. Frame tests also checked values, positions, order, and null
dtypes against the row APIs. IHP and generated timing frames occupied 4,865,754
and 6,566,000 bytes respectively. The IPC transport buffer is proportional to
emitted data and coexists briefly with the decoded frame.

## Verification and reproduction

```bash
cargo fmt --all --check
cargo test --workspace --locked --offline
uv run maturin develop --release
.venv/bin/python -m pytest -q -rs
uv run --no-sync ruff check .
uv run --no-sync ty check
node --check viewer/static/app.js
node --check viewer/static/symbol.js
```

The Rust workspace passed 53 tests and pytest passed 445 tests with eight skips.
Rust formatting, Ruff, ty, and both JavaScript syntax checks passed.

Both formatter entry points emitted identical output on `dev.lib`. Headless
Chromium exercised library metadata, tree, source, symbols, scalar view,
driver waveform, multi-curve power, heatmap and surface, and the 3D, CCS, and
CCSN waveform click paths. The first chart initiated the sole Plotly request. Closing
the page triggered the server’s exit-on-close watchdog.

Run `scripts/liberty_bench.py WORKLOAD [INPUT] --iterations 5` with the release
extension installed in the checkout’s interpreter. Workloads are
`parse`, `select`, `attributes`, `traverse`, `frame`, `format`, `viewer`, `compare`, `writer`, `boolean`, `combo`, `annotation`. The runner reports source digests, dimensions, native
module path, versions, median and individual samples in JSON. A saved
baseline module can be used from outside the checkout with `PYTHONPATH` and
`--revision`. The raw results from this run are under `/tmp/liberty-final-bench`
and `/tmp/liberty-verified-bench`.

The six opt-in DEF performance tests and two tests requiring the local
`generic80_ss_125c_1p116v_0p84v.lib` fixture were skipped.
