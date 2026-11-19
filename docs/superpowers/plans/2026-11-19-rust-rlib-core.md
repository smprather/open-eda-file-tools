# Plain-Rust `rlib` Core Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Split the repo into a pyo3/Arrow-free `open-eda-file-tools-core` rlib workspace member plus the existing `_native` cdylib bindings, with the DEF bbox scan, LEF macro sizes, Liberty attribute-only parse, and `LibraryIndex` callable from plain Rust (issue #12).

**Architecture:** Root `Cargo.toml` stays the bindings package (`_native`, cdylib, pyo3/arrow/parquet) and gains a `[workspace]` table with `members = ["core"]`. All pure machinery (threads, LEF lexer/parser, DEF scanner + bbox/net-shapes/clock logic, the Liberty lexer/parser/evaluator/data model + `LibraryIndex`) moves into `core/` unchanged in behavior; bindings pyclasses become delegating wrappers (getters over an inner core struct, never field copies).

**Tech Stack:** Rust 1.96+/edition 2024 (workspace resolver 3), pyo3 0.28.3, maturin, rayon, flate2 (zlib-rs), vendored crates.io mirror, pytest/ruff/ty.

**Spec:** `docs/superpowers/specs/2026-11-19-rust-rlib-design.md`

## Global Constraints

- Python behavior must stay byte-for-byte identical: same `_native` module name, same pyclasses/pyfunctions, same CSV/table/Parquet outputs, same error classes and texts.
- `core` must have **no pyo3, arrow or parquet** in its dependency graph (asserted: `cargo tree -p open-eda-file-tools-core`); its only direct deps are flate2 (zlib-rs), memchr, rayon, quine-mc_cluskey.
- Build with `uv run maturin develop --release` after any Rust change, then test with `uv run --no-sync pytest -q`. Plain `uv run` can reinstall a stale cached wheel.
- Gates before every commit: `cargo test --workspace`, `cargo fmt --all --check`, `cargo clippy --workspace --release` (warning count ≤ 26), `uv run --no-sync ruff check .`, `uv run --no-sync ty check`. Offline proof after Task 1: `CARGO_NET_OFFLINE=true cargo build --workspace --offline --locked`.
- Documentation synchronization mandate: every public API, CLI, layout or architecture change updates `docs/ai-api.md`, `docs/cli.md`, `docs/*.md`, `README.md`, `CLAUDE.md` (and `.pyi` stubs where the Python API changes — it must not) in the same commit.
- No design files committed; tests use inline synthetic DEF/LEF text.
- Parallel work stays inside `core::threads::install`; output must not depend on `--threads`.
- Indent with spaces only; errors on moved code keep their exact text.
- Workspace version stays `2026.11.13` (no release in this change); `Cargo.lock` is committed.

## Review Focus

- A DEF with no `UNITS` line: `dbu_per_micron` returns 1000 (the default), the scan still works. (Task 4 test `test_dbu_per_micron_defaults_to_1000`.)
- A DEF whose `COMPONENTS` section is missing entirely: `scan_instance_bboxes` fails with the existing "DEF missing COMPONENTS section" error, not a panic. (Task 4 test `test_scan_without_components_errors`.)
- A Liberty file whose units/templates are declared *after* the cells (late items): `cell_attributes` / `LibraryIndex::cell` still resolve them, as the Python surface does today. (Task 4 test `test_attributes_on_late_declared_units`.)
- `CellAttributesIter` after a per-cell parse error: `next()` returns the error once and the iterator does not panic or loop forever. (Task 4 test `test_attributes_iter_survives_error_cell`.)
- `scan_instance_bboxes` under `set_threads(1)` vs default: identical rows. (Task 4 test `test_scan_thread_count_does_not_change_output`.)

---

### Task 1: Workspace scaffold, `threads` and LEF move to core

**Files:**
- Modify: `Cargo.toml` (workspace + package inheritance), `.github/workflows/tests.yml` (cargo test step)
- Create: `core/Cargo.toml`, `core/src/lib.rs`, `core/src/threads.rs`, `core/src/lef/mod.rs`, `core/src/lef/lexer.rs`, `core/src/lef/cell_lef.rs`
- Modify: `src/lib.rs` (module decls → re-exports), `src/threads.rs`, `src/lef/mod.rs`, `src/lef/cell_lef.rs`, new `src/lef/python.rs`

**Interfaces:**
- Produces (used by Tasks 2–5):
  - `open_eda_file_tools_core::threads::{set_threads(n: Option<usize>), get_threads() -> usize, install, install_for}` — the existing CPU-limit state and helpers, now public in core.
  - `open_eda_file_tools_core::lef::{parse_library(paths, filter, read_only) -> LefResult<LefLibrary>, LefLibrary, LefMacro, LefPin, LefPort, LefRect, LefError}` — moved verbatim; struct fields become `pub` (they carry `#[pyo3(get)]` today — drop the attr, keep the field).
  - Bindings keep `src/lef/` with a `python` module holding the five pyclasses (`LefLibrary`, `LefMacro`, `LefPin`, `LefPort`, `LefRect`) as delegation wrappers and the `parse_lef` pyfunction; `src/lib.rs` keeps `pub mod lef;` so `crate::lef::parse_library` still resolves for `src/def.rs` until Task 2.

- [ ] **Step 1: Convert `Cargo.toml` to a root-package workspace.** Add `[workspace] members = ["core"]`, `resolver = "3"`, `[workspace.package] version = "2026.11.13"`, `edition = "2024"`; root `[package]` switches to `version.workspace = true`, `edition.workspace = true`. Create `core/Cargo.toml`: package `open-eda-file-tools-core`, lib name `open_eda_file_tools_core`, `crate-type = ["rlib"]`, workspace-inherited version/edition, deps exactly `flate2` (default-features off, `zlib-rs`), `memchr`, `rayon`, `quine-mc_cluskey`; a comment lists the allowed-deps rule.

- [ ] **Step 2: Move `src/threads.rs` to core.** `core/src/threads.rs` gets the state, `set_threads`/`get_threads`/`install`/`install_for` exactly as written (public). `src/threads.rs` keeps the two `#[pyfunction]`s `set_threads`/`get_threads` delegating to `core::threads`, and `src/lib.rs` re-exports `pub use open_eda_file_tools_core::threads;`.

- [ ] **Step 3: Move the LEF lexer and parser to core.** `core/src/lef/lexer.rs` = verbatim move. `core/src/lef/cell_lef.rs` = `src/lef/cell_lef.rs` minus the pyclasses and `parse_lef` pyfunction: keep `LefError`, `parse_library`, all `*Data` internals, and make the data structs' fields `pub`. `core/src/lef/mod.rs` declares both.

- [ ] **Step 4: Rewrite the bindings' LEF layer as delegation.** New `src/lef/python.rs`: each pyclass (`LefLibrary`/`LefMacro`/`LefPin`/`LefPort`/`LefRect`) is `#[pyclass] struct X { inner: open_eda_file_tools_core::lef::X }` with `#[getter]` methods delegating to `inner` — Python attribute names identical to today (e.g. `LefRect.layer/x0_microns/…`, `LefMacro.name/size/…`). `parse_lef` pyfunction calls `core::lef::parse_library` and converts, keeping the existing `ValueError`/`OSError`/`KeyError` mapping from `LefError`. `src/lef/mod.rs` re-exports core's lexer module path for internal users.

- [ ] **Step 5: Run the verification gates.** `uv run maturin develop --release && uv run --no-sync pytest -q` (335 passed, 8 skipped), `cargo test --workspace`, `cargo fmt --all --check`, `cargo clippy --workspace --release` (≤26), `uv run --no-sync ruff check .`, `uv run --no-sync ty check`, `CARGO_NET_OFFLINE=true cargo build --workspace --offline --locked`.

- [ ] **Step 6: Add the CI Rust-test step.** `.github/workflows/tests.yml`: after "Sync dependencies and build the extension", add a step `cargo test --workspace --offline --locked`.

- [ ] **Step 7: Commit.** `git add -A && git commit -m "split core rlib out of the extension: workspace, threads and LEF"` (commit `Cargo.lock` too).

### Task 2: DEF machinery to core

**Files:**
- Modify: `src/def.rs` (becomes bindings-only), create `core/src/def.rs`, modify `core/src/lib.rs`

**Interfaces:**
- Consumes: `core::threads` (Task 1).
- Produces (used by Tasks 3–5):
  - `open_eda_file_tools_core::def::{InstanceBBox, NetShape, SkippedRoutes, Component, NetConn, NetInfo, MacroDef}` — data structs with `pub` fields (the `#[pyo3(get)]` attrs drop).
  - `open_eda_file_tools_core::def::{DefScanner, NetRecordScanner, ByteTokens, parse_lef_files, to_dbu, process_net_record_batch, parse_net_shapes_record, read_route_point, RoutePoint, RouteContext, make_net_shape, collect_def_connectivity, clock_tree_cells_impl, clock_object_instances_impl, …}` — every pure `fn`/`struct`/`const` from `src/def.rs` (67 fns) moved verbatim, `pub` where the bindings or Task 4 call them.
  - Bindings `src/def.rs` keeps: the 4 pyclasses (`InstanceBBox`, `NetShape`, `InstanceBBoxIter`, `NetShapeIter`) as delegation wrappers, and the 12 pyfunctions (`instance_bboxes`, `net_shapes`, `write_instance_table`, `write_instance_bbox_csv`, `write_instance_parquet`, `write_net_shapes_csv`, `write_def_cache`, `write_net_shapes_csv_from_cache`, `write_clock_instances_csv`, `write_clock_instances_csv_from_cache`, `clock_tree_cells`, `clock_object_instances`) delegating to core with identical signatures, error classes and texts.

- [ ] **Step 1: Move the pure half.** `core/src/def.rs` = `src/def.rs` with the pyclasses, pyfunctions, `use pyo3::…` imports and the parquet/arrow writer plumbing removed; the cache writers (`write_def_cache_impl` and friends) move to core too but the Arrow `RecordBatch`/`Compression::ZSTD` writer code (`write_instances_parquet_impl`) stays in the bindings (it needs arrow/parquet). `SkippedRoutes`, the route state machine and `parse_net_shapes_record` move with everything else, unchanged. Add `pub mod def;` to `core/src/lib.rs`.

- [ ] **Step 2: Make the bindings delegate.** `src/def.rs` keeps the four pyclasses as `{ inner: core::def::X }` wrappers with `#[getter]` per field (names unchanged: `instance, macro_name, status, orient, llx, lly, urx, ury` / `net, section, route_status, shape_type, layer, width, x1, y1, x2, y2, llx, lly, urx, ury`; `InstanceBBoxIter`/`NetShapeIter` keep their `__iter__`/`__next__`/`skipped` methods calling `core::def::process_*_batch`), and the twelve pyfunctions call the moved core fns. `crate::lef::parse_library`/`crate::threads` uses inside core become `crate::lef`/`crate::threads` within core (same names at the core boundary).

- [ ] **Step 3: Run the verification gates** (Task 1 Step 5 command set). The DEF test surface is `tests/test_instances.py`, `tests/test_net_shapes_routes.py`, `tests/test_real_world_def.py` — all must pass unchanged.

- [ ] **Step 4: Commit.** `git commit -m "move DEF scanner, bbox, net-shapes and clock logic into core"`

### Task 3: Liberty parser to core

**Files:**
- Modify: `src/lib.rs` (becomes bindings + registration), create `core/src/liberty.rs`, modify `core/src/lib.rs`

**Interfaces:**
- Consumes: `core::threads` (Task 1).
- Produces (used by Tasks 4–5):
  - `open_eda_file_tools_core::liberty::{Document, Cell, Pin, InternalPower, Bus, Bundle, BusType, TimingArc, CcsnStage, TimingTable, PgPin, LeakagePower, Ff, Latch, DynamicCurrent, SwitchingGroup, PgCurrent, BooleanExpression, LibraryIndex, LibraryScan, UnitContext, AttrEdit, WhenFilter, BoolParser}` — the data structs renamed by dropping the `Data` suffix (`CellData`→`Cell`, `PinData`→`Pin`, …), fields `pub`, `#[pyo3(get)]` attrs dropped.
  - `open_eda_file_tools_core::liberty::{Lexer, Parser, parse_document, parse_slice_as_library(name, body, skip_tables), open_library_index, index_cells, read_to_memory, bool_implies, energy_unit_joules, …}` — all pure fns moved verbatim; `parse_slice_as_library` gains the `skip_tables: bool` parameter (existing `Parser::new(reader, filter, skip_tables)` already supports it).
  - Bindings `src/lib.rs` keeps: the 19 pyclasses (`Document`, `Cell`, `Pin`, `InternalPower`, `Bus`, `Bundle`, `BusType`, `TimingArc`, `CcsnStage`, `TimingTable`, `PgPin`, `LeakagePower`, `Ff`, `Latch`, `DynamicCurrent`, `SwitchingGroup`, `PgCurrent`, `BooleanExpression`, `LibraryIndex`) as delegation wrappers with every existing method/getter delegating (signatures and behavior identical, including `lookup`/`value_at`, `attributes()`, `templates(py)`, `is_seq`-relevant `ff()`/`latch()`), and the pyfunctions `parse_file`, `parse_file_simple`, `set_threads` re-export; `pub mod otw; pub mod pex;` stay; the `_native` pymodule registration keeps registering everything.

- [ ] **Step 1: Move the pure half.** Create `core/src/liberty.rs` from `src/lib.rs`'s liberty machinery (UnitContext, AttrEdit, Document, all data structs, Lexer/Parser/BoolParser/WhenFilter, parse fns, LibraryScan/index/`open_library_index`, `parse_slice_as_library`), dropping `#[pyo3]`/`PyResult`/`PyErr` and the pyclass/pyfunction blocks, making fields `pub`, and renaming the `*Data` structs to their suffix-less names. `LibraryIndex` becomes a plain struct whose `open`/`cell`/`cell_source`/`cell_names`/`num_cells`/header accessors take no `pyo3::Python` (they already don't — only the pyclass shell did).

- [ ] **Step 2: Rewrite the bindings layer.** `src/lib.rs` = `#[pymodule] fn _native` + `pub mod def; pub mod lef; pub mod otw; pub mod pex;` re-exports + the 19 delegating pyclasses + `parse_file`/`parse_file_simple` pyfunctions calling `core::liberty` under `py.detach` (GIL release preserved, error enums `ParseLoadError`/`CellLookupError`/`CellSourceError` keep their `into_py` mappings). Every existing `#[pymethods]` body moves onto the wrapper and calls `self.inner.<same logic>` — do not re-derive logic, only relocate it.

- [ ] **Step 3: Run the verification gates** (Task 1 Step 5 command set). The Liberty test surface is `tests/test_*.py` (extraction, comparison, writer, viewer data) — all unchanged.

- [ ] **Step 4: Commit.** `git commit -m "move Liberty parser, data model and LibraryIndex into core"`

### Task 4: Public core API + Rust integration tests

**Files:**
- Modify: `core/src/lib.rs`, `core/src/def.rs`, `core/src/lef/mod.rs`, `core/src/liberty.rs`
- Test: `core/tests/core_api.rs` (create)

**Interfaces:**
- Consumes: all of Tasks 1–3.
- Produces (documented in Task 5):
  - `core::Error(pub String)` — `impl Display + std::error::Error`, `From<String>`, `From<std::io::Error>`.
  - `core::def::{InstanceBBox, InstanceBBoxIter, scan_instance_bboxes(def: &Path, lefs: &[PathBuf]) -> Result<InstanceBBoxIter, Error>, dbu_per_micron(def: &Path) -> Result<i64, Error>, NetShape, NetShapeIter, scan_net_shapes(def: &Path, net: &str, layer: &str) -> Result<NetShapeIter, Error>, SkippedRoutes}`
  - `core::lef::{MacroSize { width_um: f64, height_um: f64 }, macro_sizes(lefs: &[PathBuf]) -> Result<HashMap<String, MacroSize>, Error>}`
  - `core::liberty::{CellAttributes { name: String, attributes: Vec<(String, String)>, is_sequential: bool }, CellAttributesIter, cell_attributes(lib: &Path) -> Result<CellAttributesIter, Error>, LibraryIndex::open(path: &Path) -> Result<LibraryIndex, Error>, LibraryIndex::cell(name) -> Result<Cell, Error>, LibraryIndex::cell_attributes(name) -> Result<CellAttributes, Error>, LibraryIndex::cell_source(name) -> Result<String, Error>, LibraryIndex::{library_name, cell_names, num_cells, voltage_unit, current_unit, time_unit, capacitive_load_unit, energy_unit_joules, templates() -> HashMap<String, Vec<String>>, attributes() -> Vec<(String, String)>}}` — core `templates()` returns a plain map; the bindings' `Document.templates(py)`/`LibraryIndex.templates(py)` build the existing `PyDict` from it. `is_sequential` = `ff.is_some() || latch.is_some()`; `CellAttributesIter` owns its `LibraryIndex` and parses cells lazily with `skip_tables=true`.

- [ ] **Step 1: Write the failing tests** in `core/tests/core_api.rs`. Helper: `write_def(tmp, body) -> PathBuf` / `write_lef(tmp) -> PathBuf` / `write_lib(tmp, cells: &str) -> PathBuf` building minimal synthetic files (same shapes as the existing Python fixtures: `UNITS DISTANCE MICRONS 1000`, `COMPONENTS 2`, `MACRO INV SIZE 1.2 BY 2.4`, `library (x) { … }` with an `ff` cell, a plain cell, and a cell with attributes). Tests:

```rust
#[test] fn test_scan_instance_bboxes_rows()   // 2 components incl. E-orient width/height swap
#[test] fn test_dbu_per_micron_defaults_to_1000()   // DEF with no UNITS line
#[test] fn test_scan_without_components_errors()    // matches "DEF missing COMPONENTS section"
#[test] fn test_scan_unknown_macro_errors()         // "missing LEF macro '…' for instance '…'"
#[test] fn test_macro_sizes_microns()               // width_um 1.2 / height_um 2.4
#[test] fn test_cell_attributes_iter()              // order == file order; ff cell is_sequential, plain cell not; attributes present
#[test] fn test_attributes_on_late_declared_units() // units declared after cells still resolve
#[test] fn test_attributes_iter_survives_error_cell() // a cell body that fails to parse yields one Err item, then the rest continue
#[test] fn test_library_index_open_cell_and_source() // cell()/cell_source()/cell_names()/num_cells(); unknown name errors
#[test] fn test_scan_thread_count_does_not_change_output() // set_threads(1) vs default rows identical
#[test] fn test_scan_net_shapes_and_skipped()        // a routed net with a via: rows + SkippedRoutes
```

  Exact row values for `test_scan_instance_bboxes_rows`: instance `u1 INV PLACED N (10,20) SIZE 1.2x2.4 @1000 → (10,20,1210,2420)`; `u2 INV FIXED E (30,40) → (30,40,2430,1240)` (width/height swapped for E). These mirror `tests/test_instances.py::test_python_iterator_returns_placed_instance_bboxes`.

- [ ] **Step 2: Run the tests and see them fail.** `cargo test --workspace` — Expected: FAIL (module/API missing).

- [ ] **Step 3: Implement the API in core.** `Error` in `core/src/lib.rs`; `scan_instance_bboxes`/`dbu_per_micron`/`scan_net_shapes` wrap the moved `DefScanner`/`NetRecordScanner`/`parse_lef_files`; `dbu_per_micron` = a header-only lexer pass reusing `parse_def_units` (stop at the first section, return `unwrap_or(1000)`); `macro_sizes` maps `parse_library` macros to `MacroSize`; `cell_attributes`/`CellAttributesIter`/`LibraryIndex` public methods over the Task 3 machinery; the bindings' pyfunctions switch to the same public fns where they exist (no duplication). Keep internal `Result<_, String>` conventions; convert at the public boundary only.

- [ ] **Step 4: Run the tests and see them pass**, then the full gates (Task 1 Step 5 command set).

- [ ] **Step 5: Commit.** `git commit -m "publish the core Rust API: DEF bbox scan, LEF macro sizes, Liberty attributes and LibraryIndex"`

### Task 5: Documentation, equivalence gate, and offline proof

**Files:**
- Modify: `docs/ai-api.md`, `BUILD.md`, `CLAUDE.md`, `README.md`, `docs/def.md` (Rust section only)
- No code changes unless a check fails.

- [ ] **Step 1: Equivalence against pre-change main.** Build the reference from `fba740b` (see `scripts/def_equivalence.py` docstring), generate `uv run --no-sync python -m tests.gen.design_gen -o /tmp/gen -n 200000 --no-gzip`, run `uv run --no-sync python scripts/def_equivalence.py /tmp/ref-target/release/lib_native.so /tmp/gen`. Expected: **every line `ok`, `differences: 0`** — this refactor changes nothing observable (unlike the route-geometry change, there is no intentional DIFF to list).

- [ ] **Step 2: Dependency-surface proof.** `cargo tree -p open-eda-file-tools-core` — Expected: only flate2/memchr/rayon/quine-mc_cluskey and their transitive pure-Rust deps; no pyo3, arrow, parquet, zstd-sys. Record the output in the commit message body.

- [ ] **Step 3: Performance sanity.** `uv run --no-sync python scripts/def_bench.py /tmp/cva6-bench` (or `/tmp/gen`): `instances csv` and `net-shapes signal` within ~10% of the pre-change numbers (0.53 s / 0.02 s on 2026-11-19). Expected: no delegation-cost regression.

- [ ] **Step 4: Docs.** `docs/ai-api.md`: new "Rust core (`open-eda-file-tools-core`)" section with the Task 4 API signatures, the `Error` type, the `dbu_per_micron` accessor, and the dependency-surface note (offline, no pyo3/Arrow, no C toolchain for core). `BUILD.md`: workspace layout (root = bindings, `core/` = rlib), the consumer recipe (`[dependencies] open-eda-file-tools-core = { git = "…", branch = "rust-rlib-core" }`, vendored mirror note), and the gates updated to `--workspace`. `CLAUDE.md`: architecture section gains the workspace members, the "bindings delegate to core, never re-implement" invariant, the core allowed-deps rule, and where each moved file lives now. `README.md`: one line naming the Rust core crate. `docs/def.md`: mention the Rust API next to the Python one.

- [ ] **Step 5: Full gates + offline wheel proof.** Task 1 Step 5 command set, plus `uv build --wheel` with `CARGO_NET_OFFLINE=true`, plus `scripts/run_example_offline.sh` from a scratch dir (instances CSV on `/tmp/gen`). Expected: all pass.

- [ ] **Step 6: Commit.** `git commit -m "document the Rust core crate and workspace layout"`

---

## Execution notes

- Tasks run in order; each ends green and committed. The bindings keep zero copies of parser logic at every step (Task 2's `crate::lef` re-export is removed once `src/def.rs` no longer needs it, by Task 2 Step 2).
- The 26-warning clippy budget is for `--workspace --release`; the move may *lower* it (the 10-arg `make_net_shape` is already gone) but must never raise it.
- The `.worktrees/` directory and branch naming (`rust-rlib-core`) already exist; commit directly on the branch.
