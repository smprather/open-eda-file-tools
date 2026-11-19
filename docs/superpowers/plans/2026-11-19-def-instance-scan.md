# DEF Instance Scan Follow-ups Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the sequential component scan cheaper, make the table format stop reading the DEF twice, and make `uv run` never test a stale native build.

**Architecture:** Three independent changes:
- **Build cache:** `pyproject.toml` gains uv cache keys for the Rust sources.
- **Record batches:** `DefScanner` returns a batch whose records share one byte buffer, not one `Vec` per record. Since the overlapped scan is now the critical path of the instance writers, every saved allocation shortens the run.
- **Table format (gated):** if the table format is still much slower than CSV after the batch change, it writes parsed rows to a temporary spool while measuring column widths, then formats from the spool instead of re-reading the DEF.

**Tech Stack:** Rust (PyO3 0.28, rayon), uv, pytest.

**Spec:** No spec document. Requirements come from the 2026-11-19 measurements below.

## Background (2026-11-19, commit 0812a7d)

- Phase timings on a 458 MB generated DEF (2M instances), before the overlap work: component scan 273 ms (sequential), parse 166 ms (parallel). With the scan now overlapped, `instances` CSV takes 0.53 s and the scan is its critical path.
- `instances` table takes 1.06 s vs 0.53 s for CSV, because `instance_table_widths` scans the DEF once for column widths before the writing scan. On cva6: 0.15 vs 0.08 s (0.139 vs 0.074 s gzipped).
- `pyproject.toml` has no `[tool.uv] cache-keys` covering `src/`. During the 2026-11-19 work, `uv run pytest` replaced a fresh `maturin develop` build with a cached wheel built from older Rust sources. That is how a removed debug probe kept printing.

## Global Constraints

- Until Task 1 lands: build with `uv run maturin develop --release`, test with `uv run --no-sync pytest -q`.
- Gates before every commit: `cargo test`, `cargo fmt --all --check`, `cargo clippy --release` (warning count must not rise above 26), `uv run --no-sync ruff check .`, `uv run --no-sync ty check`.
- Output identity: `scripts/def_equivalence.py <ref .so> <dir>` reports `differences: 0` after Tasks 2 and 3. Run it on a generated design (`-n 200000`) at default threads and `--threads 1`, and on a routed DEF when available. Build the reference from the commit before Task 2.
- Memory must not grow with design size: `uv run --no-sync pytest tests/test_perf.py --runperf -q` passes (`peak RSS` for 800k instances stays below 2× that for 200k).
- Parallel work stays inside `crate::threads::install` / `install_for`; the scan thread counts inside `--threads`.
- Documentation synchronization mandate (`CLAUDE.md`): update `CLAUDE.md` (Commands section, DEF invariants) and `docs/def.md` (performance table, table-format note).
- Indent with spaces only.

## Review Focus

- A DEF whose single component record is larger than a whole batch's usual size (a component with a huge `+ PROPERTY` string): offsets in the shared buffer must not overflow or truncate. (Task 2 test `test_oversized_component_record`.)
- Error messages still name the record's DEF byte offset after the batch change. (Task 2 test `test_component_errors_keep_their_offset`.)
- The table spool is removed on success, on a parse error and on a write error. (Task 3 test `test_table_spool_is_removed_on_error`.)
- `--threads 1` with the spool: identical bytes. (Covered by `tests/test_def_scanning.py::test_outputs_do_not_depend_on_the_thread_limit`, which already writes the table.)
- `TMPDIR` pointing at a missing directory: the table writer fails with an error naming the path, and no output file is left behind. (Task 3 test `test_table_spool_needs_a_temp_dir`.)

---

### Task 1: uv rebuilds the extension when Rust sources change

**Files:**
- Modify: `pyproject.toml`: add a `[tool.uv]` table with `cache-keys = [{ file = "pyproject.toml" }, { file = "Cargo.toml" }, { file = "Cargo.lock" }, { file = "src/**/*.rs" }]`. Merge into an existing `[tool.uv]` table if there is one.
- Modify: `CLAUDE.md`, Commands section: say that `uv run` rebuilds after Rust changes, and that `maturin develop --release` is still the fast path for an optimized build.

- [ ] **Step 1: Reproduce the stale build.** `uv run maturin develop --release`. Then add a line `eprintln!("STALE-CHECK");` at the top of `get_threads` in `src/threads.rs`, and run `uv run python -c "from liberty_tools import _native; _native.get_threads()"`. Expected today: no `STALE-CHECK` on stderr (the old wheel is used).

- [ ] **Step 2: Add the cache keys** and run the same command. Expected: uv rebuilds the extension (a build line appears), and `STALE-CHECK` is printed.

- [ ] **Step 3: Remove the probe line**, then run `uv run python -c ...` again. Expected: a rebuild, and no `STALE-CHECK`.

- [ ] **Step 4: Run the gates and commit.** `git commit -m "uv: rebuild the native extension when Rust sources change"`

### Task 2: Component batches share one buffer

**Files:**
- Modify: `src/def.rs`: `RawRecord` (around line 157), `DefScanner::next_batch` / `next_record` (around line 2808), `parse_component_record`, `component_name_lengths`, `parse_components`, `process_record_batch`, `instance_table_widths`, `write_component_rows`, `write_instance_parquet_impl`, `InstanceBBoxIter`
- Test: `tests/test_def_scanning.py`

**Interfaces:**
- Produces:

```rust
/// Component records of one scan batch, stored back to back.
struct RecordBatch {
    bytes: Vec<u8>,
    records: Vec<RecordSpan>,
}
#[derive(Clone, Copy)]
struct RecordSpan { start: usize, end: usize, offset: u64 }   // offset = DEF byte of the record's '-'
#[derive(Clone, Copy)]
struct RecordRef<'a> { bytes: &'a [u8], offset: u64 }
impl RecordBatch {
    fn len(&self) -> usize;
    fn is_empty(&self) -> bool;
    fn get(&self, index: usize) -> RecordRef<'_>;
    fn record(&self, span: RecordSpan) -> RecordRef<'_>;   // parallel code iterates batch.records.par_chunks(CHUNK)
}
impl DefScanner {
    fn next_batch(&mut self, max_items: usize) -> io::Result<RecordBatch>;   // replaces Vec<RawRecord>
}
fn parse_component_record<'a>(record: RecordRef<'a>, macros: &MacroMap) -> Result<Option<Component<'a>>, String>;
fn component_name_lengths(record: RecordRef<'_>) -> Result<Option<(usize, usize)>, String>;
```

  `RawRecord` is deleted. `next_record` appends `-` and then `read_until_semicolon` into `batch.bytes`, which already appends. `for_each_batch` stays generic: implement `for_each_batch` over a `next` closure returning `RecordBatch`, by making its batch type a trait `Batch { fn is_empty(&self) -> bool }`, implemented for `RecordBatch` and for `Vec<T>`.

- [ ] **Step 1: Write the failing tests.**

```python
def test_oversized_component_record(tmp_path):
    lef, design = write_design(tmp_path, components=3)
    big = "x" * (3 << 20)
    text = design.read_text(encoding="utf-8").replace(
        "- u1 INV + PLACED", f'- u1 INV + PROPERTY note "{big}" + PLACED'
    )
    design.write_text(text, encoding="utf-8")
    rows = [r.as_tuple() for r in def_tools.instance_bboxes([lef], design)]
    assert [r[0] for r in rows] == ["u0", "u1", "u2"]

def test_component_errors_keep_their_offset(tmp_path):
    lef, design = write_design(tmp_path, components=3)
    text = design.read_text(encoding="utf-8")
    offset = text.index("- u2 INV")
    design.write_text(text.replace("- u2 INV + PLACED ( 20 2 ) E", "- u2 INV + PLACED ( 20 2 ) Q"), encoding="utf-8")
    with pytest.raises(RuntimeError, match=f"near DEF byte {offset}"):
        def_tools.write_instance_bbox_csv([lef], design, tmp_path / "o.csv")
```

  Both pass today (they guard the refactor). Run them before the change to confirm, and confirm the exact `u2` text matches the generator line `- u2 INV + PLACED ( 20 2 ) E`.

- [ ] **Step 2: Measure the baseline.** Generate the 2M design: `uv run --no-sync python -m tests.gen.design_gen -o /tmp/gen2m -n 2000000 --no-gzip`. Then run `uv run --no-sync python scripts/def_bench.py /tmp/gen2m`. Record `instances csv`, `instances parquet`, `instances table` and `instance_bboxes iter`.

- [ ] **Step 3: Implement** `RecordBatch` and switch every user listed under Files.

- [ ] **Step 4: Verify.** Full gates, `tests/test_perf.py --runperf`, and the equivalence script (default and `--threads 1`). Expected: `differences: 0`.

- [ ] **Step 5: Measure again** with the same command as Step 2. Expected: `instances csv` at least 10% below the baseline (target ≤ 0.45 s on the 5900X). If it is not, revert this task and record the measurement in `docs/def.md` instead. Otherwise update the `docs/def.md` numbers.

- [ ] **Step 6: Commit.** `git commit -m "DEF scan: component batches share one buffer"`

### Task 3 (gated): The table format reads the DEF once

**Gate:** Run `scripts/def_bench.py /tmp/gen2m` after Task 2. If `instances table` ÷ `instances csv` ≤ 1.5, skip this task. In that case, record both numbers in `docs/def.md` under the table-format note and commit that alone. Otherwise do the steps below.

**Files:**
- Create: `src/spool.rs` (`mod spool;` in `src/lib.rs`, crate-private)
- Modify: `src/def.rs`: `write_instance_table_impl` (around line 1200), `write_instance_table_stream`; delete `instance_table_widths` and `component_name_lengths` once unused
- Test: `tests/test_def_scanning.py`

**Interfaces:**
- Produces:

```rust
/// Rows of placed components on disk, for a second pass that needs global
/// column widths. Removed when dropped.
pub(crate) struct TableSpool { /* path: PathBuf, writer: Option<BufWriter<File>> */ }
impl TableSpool {
    pub(crate) fn create() -> Result<Self, String>;   // in std::env::temp_dir(); name def-tools-table-<pid>-<nanos>.spool
    pub(crate) fn push(&mut self, row: &Component<'_>) -> Result<(), String>;
    pub(crate) fn rows(&mut self) -> Result<SpoolRows<'_>, String>;  // finishes writing, reads back in file order
}
pub(crate) struct SpoolRows<'a> { /* ... */ }
impl SpoolRows<'_> {
    /// Next batch of up to `max` rows; empty at the end.
    pub(crate) fn next_batch(&mut self, max: usize) -> Result<Vec<OwnedRow>, String>;
}
pub(crate) struct OwnedRow { pub instance: String, pub macro_name: String, pub status: &'static str, pub orient: &'static str, pub llx: i64, pub lly: i64, pub urx: i64, pub ury: i64 }
```

  Record layout: `u32` instance length, its bytes, `u32` macro length, its bytes, `u8` status (`0` PLACED, `1` FIXED, `2` COVER), `u8` orient (index into `N S E W FN FS FE FW`), then four little-endian `i64`. `Drop` removes the file and ignores a missing one. `OwnedRow` exists only so a batch of rows can be rendered in parallel. The render closure becomes generic over a small `TableRow` trait implemented by `Component<'_>` and `OwnedRow`, so the CSV path keeps borrowing.

- [ ] **Step 1: Write the failing tests.**

```python
def test_table_spool_is_removed_on_error(tmp_path, monkeypatch):
    monkeypatch.setenv("TMPDIR", str(tmp_path / "spool"))
    (tmp_path / "spool").mkdir()
    lef, design = write_design(tmp_path, components=3)
    design.write_text(design.read_text(encoding="utf-8").replace(") E ;", ") Q ;"), encoding="utf-8")
    with pytest.raises(RuntimeError, match="unsupported orientation"):
        def_tools.write_instance_table([lef], design, tmp_path / "t.txt")
    assert list((tmp_path / "spool").iterdir()) == []

def test_table_spool_needs_a_temp_dir(tmp_path, monkeypatch):
    monkeypatch.setenv("TMPDIR", str(tmp_path / "missing"))
    lef, design = write_design(tmp_path, components=3)
    with pytest.raises(RuntimeError, match="missing"):
        def_tools.write_instance_table([lef], design, tmp_path / "t.txt")
    assert not (tmp_path / "t.txt").exists()
```

  `std::env::temp_dir()` reads `TMPDIR` on every call, so `monkeypatch.setenv` reaches the native code.

- [ ] **Step 2: Run the tests and see them fail.** Run: `uv run --no-sync pytest tests/test_def_scanning.py -q -k spool`. Expected: FAIL. No spool is created today, and the missing-temp-dir case succeeds.

- [ ] **Step 3: Implement.** One overlapped pass (`for_each_batch` + `parse_components`) pushes rows to the spool and tracks the instance and macro name widths. After the pass, write the header, then read the spool in batches of `BATCH_SIZE` rows, render each batch with `render_rows` and write in order. Create the output file only after the spool pass succeeds (keep today's rule: no partial output on a parse error).

- [ ] **Step 4: Verify.** Full gates, `tests/test_perf.py --runperf`, and the equivalence script (`write_instance_table_*` lines `ok`).

- [ ] **Step 5: Measure.** Run `scripts/def_bench.py /tmp/gen2m` and the cva6 directory. Expected: `instances table` ≤ 1.3 × `instances csv`. Update `docs/def.md` (numbers, and replace "The table format scans the DEF twice" with the spool note: temporary disk use is about name lengths + 42 bytes per placed instance, in `TMPDIR`).

- [ ] **Step 6: Commit.** `git commit -m "instances table: one DEF pass with a temporary row spool"`

## Not planned

- **Releasing the GIL inside `instance_bboxes` / `net_shapes` iteration:** iteration cost is dominated by creating Python objects, which needs the GIL anyway; the gain is concurrency only, and no measurement motivates it yet.
- **Regular-net wire widths in `net-shapes`:** they need technology LEF layer widths, which no reader in this repo parses. Noted in `docs/superpowers/plans/2026-11-19-def-route-geometry.md`, Decision 2.
