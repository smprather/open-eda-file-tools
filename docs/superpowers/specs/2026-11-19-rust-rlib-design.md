# Design: plain-Rust `rlib` core for the LEF/DEF and Liberty readers

Issue: smprather/open-eda-file-tools#12 — "Publish the LEF/DEF and Liberty
readers as a plain Rust rlib (no pyo3), with attribute-only Liberty parsing".

## Intent and why

A downstream Rust tool (the author's Voltus IR-drop viewer) needs three
capabilities from this repo without Python: DEF `COMPONENTS` → per-instance
bboxes with LEF `SIZE` extents and `UNITS DISTANCE MICRONS` honored, LEF
macro sizes, and Liberty cell-level attributes for `is_seq` classification.
Today those exist only behind the pyo3 extension (`crate-type = ["cdylib"]`,
`pyo3` pervasive in the parse paths, Arrow/Parquet linked into the same
module), so the consumer hand-rolled a weaker parser. The goal is to make
the existing readers linkable as a plain Rust library with a minimal,
offline-buildable dependency surface — not to fork or rewrite the parsing.

## Goals

1. A new workspace member crate `open-eda-file-tools-core` (`crate-type =
   ["rlib"]`) with **no pyo3 and no arrow/parquet** in its dependency tree,
   exporting the DEF instance-bbox streaming scan, LEF macro sizes, the
   Liberty attribute-only path, and `LibraryIndex` callable from plain Rust
   (`LibraryIndex::open(&Path)` / `.cell(name)`, no `pyo3::Python` token).
2. The existing Python extension keeps working byte-for-byte: same `_native`
   module, same pyclasses/pyfunctions, same CSV/table/Parquet outputs. All
   335 Python tests, Rust unit tests, and `scripts/def_equivalence.py`
   against the pre-change build stay green.
3. The core crate builds fully offline from the existing `vendor/cargo`
   mirror with no C toolchain (flate2/zlib-rs, memchr, rayon,
   quine-mc_cluskey only). The wheel/build of the bindings crate keeps its
   existing parquet→zstd→zstd-sys→cc C requirement unchanged — this
   design narrows the C-free claim to `core`, it does not drop it from
   `vendor.yml`.
4. The public core API is documented (docs/ai-api.md gains a Rust section;
   core crate has rustdoc on its public items) per the docs-sync mandate.

## Non-goals (v1)

- No crates.io publication or CalVer release in this change (a release cut
  is a separate, explicitly-requested action; the consumer is happy to test
  a branch/path dependency).
- No new parsing features, no behavioral changes to any parser (grammar,
  error text, row contents, counts, bboxes all stay identical).
- `pex`/`otw` parsers stay in the bindings crate; they are not requested by
  the issue. (The shared tokenizer they need moves to core as an internal
  dependency; the bindings' pex/otw modules keep their current shape.)
- No restructuring of the Python packages, CLIs, or the `liberty_tools._native`
  module name.

## Anti-goals (failure modes)

- Python output that differs from today for any input (equivalence gate).
- A core crate that still pulls pyo3 or Arrow transitively (checked in CI).
- Duplicated parser logic: the bindings must delegate to core, never keep a
  second copy of the DEF scanner, LEF parser, or Liberty parser.
- Slower Python paths: the bindings wrap core data by reference (getters
  delegating to an inner core struct), never by cloning rows.

## Constraints

- Single project, single CalVer version; repo conventions (offline vendored
  builds, `cargo build --workspace --offline --locked` in CI, docs-sync
  mandate, spaces-only, gates before every commit: `cargo test`, `cargo fmt
  --all --check`, `cargo clippy --release` ≤ 26 warnings, `ruff`, `ty`).
- Maturin must keep discovering the bindings crate: the **root** `Cargo.toml`
  stays the bindings package (`_native`, cdylib, pyo3, arrow/parquet) and
  also carries the `[workspace]` table with `members = ["core"]`. This keeps
  `pyproject.toml`, `[tool.maturin]`, `scripts/run_example_offline.sh`
  (`cargo build --lib`), and CI paths unchanged.
- The Liberty parser (`src/lib.rs`), DEF machinery (`src/def.rs`), LEF
  (`src/lef/`) and `src/threads.rs` are already pure Rust under the pyo3
  shell; the move must not rewrite their logic, only relocate it and add
  `pub` at the new crate boundary.
- rustc 1.96+/edition 2024 (workspace `resolver = "3"`), same as today.

## Approaches considered

1. **Workspace split: root = bindings crate + workspace, `core/` = rlib
   member.** Core owns the pure machinery (`threads`, LEF lexer/cell parser,
   DEF scanner + bbox/net-shape/clock trace, the Liberty lexer/parser/
   evaluator/data model + `LibraryIndex`); bindings keep the pyo3 pyclasses/
   pyfunctions, the Arrow/Parquet writers, pex/otw, and delegate to core.
   Root manifest stays where maturin and the scripts expect it. Picked:
   matches the issue's "separate crate" suggestion, gives the consumer the
   dependency-light core (no pyo3/Arrow possible by construction), and the
   repo's tools (maturin, offline script, CI) work unchanged.
2. **Single crate, `crate-type = ["rlib", "cdylib"]`, feature-gate pyo3.**
   Less layout churn, but the pyo3 derives are woven through the parse-path
   types (`#[pyclass]` on data-carrying structs); feature-gating them means
   cfg-riddled structs and the consumer still compiles the whole crate with
   features they don't want. The issue itself calls the pyo3 entanglement
   "the real blocker". Rejected.
3. **Separate top-level workspace with three members (core, bindings,
   pyproject at root).** Cleanest academically, but moves the bindings
   manifest out from under maturin's default discovery and breaks
   `run_example_offline.sh`/CI assumptions for no consumer benefit.
   Rejected.

## Decided how

### Layout

```
Cargo.toml            # root: [workspace] (members = ["core"], resolver 3,
                      #   workspace.package version/edition) AND the bindings
                      #   package open-eda-file-tools-native (lib _native,
                      #   cdylib; pyo3, arrow, parquet, flate2, memchr,
                      #   rayon, quine-mc_cluskey)
core/Cargo.toml       # package open-eda-file-tools-core, lib
                      #   open_eda_file_tools_core, crate-type ["rlib"];
                      #   deps: flate2(zlib-rs), memchr, rayon,
                      #   quine-mc_cluskey ONLY
core/src/lib.rs       # Liberty: lexer, parser, bool evaluator, *Data model,
                      #   Document, units/templates, LibraryIndex, public
                      #   attribute-only API (moved from src/lib.rs)
core/src/def.rs       # DEF scanner, ByteTokens, records, instance bbox,
                      #   net-shapes, connectivity, clock trace, LEF-for-bbox
                      #   (moved from src/def.rs)
core/src/lef/         # lexer.rs (pyo3-free) + cell_lef.rs data/parser half:
                      #   the LefLibrary/LefMacro/LefPin/LefPort/LefRect
                      #   pyclasses stay in the bindings as delegating
                      #   wrappers, exactly like the Liberty ones
core/src/threads.rs   # the CPU-limit state and install/install_for; the
                      #   set_threads/get_threads pyfunctions stay in the
                      #   bindings over them (moved data/py split, not a
                      #   whole-file move)
src/lib.rs            # bindings: pymodule registration, pyclasses/pyfunctions
                      #   for liberty/def/lef wrapping core types, parquet
                      #   writers
src/pex.rs, src/otw.rs, src/threads re-export as needed  # stay in bindings
```

The root `src/` keeps: `mod pex; mod otw;` plus the new binding layers. The
Liberty pyclasses (`Document`, `Cell`, `Pin`, `TimingArc`, `TimingTable`,
`Bus`, `Bundle`, `BusType`, `PgPin`, `LibraryIndex`, …) stay in the bindings
as wrappers over `core::` data structs; `parse_file`/`parse_file_simple`/
`LibraryIndex::open` pyfunctions call core under `py.detach` exactly as today.

### Core public API (v1)

```rust
// def
pub struct InstanceBBox { pub instance: String, pub macro_name: String,
    pub status: String, pub orient: String,
    pub llx: i64, pub lly: i64, pub urx: i64, pub ury: i64 }
pub struct InstanceBBoxIter { /* DefScanner + MacroMap + pending batch */ }
impl Iterator for InstanceBBoxIter { type Item = Result<InstanceBBox, Error>; }
pub fn scan_instance_bboxes(def: &Path, lefs: &[PathBuf])
    -> Result<InstanceBBoxIter, Error>;
/// The DEF's UNITS DISTANCE MICRONS value (default 1000): the scale
/// consumers need to convert the i64 DBU bbox fields. Exposed because the
/// row carries DBU ints (the issue's sketch), not pre-scaled microns.
pub fn dbu_per_micron(def: &Path) -> Result<i64, Error>;

// lef
pub struct MacroSize { pub width_um: f64, pub height_um: f64 }
pub fn macro_sizes(lefs: &[PathBuf])
    -> Result<HashMap<String, MacroSize>, Error>;

// liberty — attribute-only
pub struct CellAttributes { pub name: String,
    pub attributes: Vec<(String, String)>, pub is_sequential: bool }
/// Owning, lazy per-cell parse: the index (file bytes) is held inside the
/// iterator, cells parse one at a time with skip_tables=true. This is the
/// issue's iterator sketch, not a materialized Vec — multi-GB libraries
/// never hold every cell's attributes at once.
pub struct CellAttributesIter { /* LibraryIndex + position */ }
impl Iterator for CellAttributesIter { type Item = Result<CellAttributes, Error>; }
pub fn cell_attributes(lib: &Path) -> Result<CellAttributesIter, Error>;

// liberty — indexed access (the primitive the issue names)
pub struct LibraryIndex { … }                // plain struct, no pyo3
impl LibraryIndex {
    pub fn open(path: &Path) -> Result<Self, Error>;
    pub fn library_name(&self) -> &str;
    pub fn cell_names(&self) -> Vec<String>;
    pub fn num_cells(&self) -> usize;
    pub fn cell(&self, name: &str) -> Result<Cell, Error>;        // full parse
    pub fn cell_attributes(&self, name: &str)
        -> Result<CellAttributes, Error>;                          // skip_tables
    pub fn cell_source(&self, name: &str) -> Result<String, Error>;
}
pub struct Cell { pub name: String, … pins/buses/bundles/attributes/
    ff/latch/pg_pins …, pub fn is_sequential(&self) -> bool }

pub struct Error(pub String);   // impl Display/Error; From<io::Error>, From<String>
```

`is_sequential` = the cell has an `ff` or `latch` group (the same test
`def_tools/clock_roles.py` uses today for `is_seq`). `cell_attributes` is
implemented over the index with a new `skip_tables` flag on the per-slice
parse (`parse_slice_as_library` gains a `skip_tables: bool` parameter; the
existing `Parser::new(reader, filter, skip_tables)` machinery already
supports it). Def/LEF internals additionally expose what the bindings need
(net-shapes scan, connectivity, clock trace, `MacroMap`) as `pub` items
inside `core::def` / `core::lef` — the consumer-facing contract above is
what gets documented; the rest is pub by necessity of the crate boundary.

### Bindings: delegation, not cloning

Every pyclass that carried data directly now wraps its core counterpart and
delegates by reference:

```rust
#[pyclass(frozen)]
pub struct InstanceBBox { inner: core::def::InstanceBBox }
#[pymethods]
impl InstanceBBox {
    #[getter] fn instance(&self) -> &str { &self.inner.instance }
    // … one getter per field; as_tuple() etc. unchanged
}
```

Python attribute names, signatures, return types and error classes stay
exactly as they are; only the internal representation changes. Rows are
moved out of core into the pyclass once per row (no field copies).

### Error handling

Core internals keep their `Result<_, String>`/`io::Result` conventions; the
public core API wraps failures in `Error(String)` (with `From<io::Error>`).
No error-message changes: the Python surface must produce identical text.

### Verification (the refactor gate)

1. Existing gates per commit, widened to the workspace (the root package
   alone is `default-members`; `core` must be built/tested/linted too):
   `cargo test --workspace`, `cargo fmt --all --check`,
   `cargo clippy --workspace --release` (≤26 warnings), full
   `uv run pytest -q`, `ruff`, `ty`. `.github/workflows/tests.yml` gains a
   `cargo test --workspace --offline --locked` step (CI currently runs no
   Rust tests at all; the moved 13k lines need the guard).
2. `scripts/def_equivalence.py` with a reference `lib_native.so` built from
   `main` (fba740b) before the change: all `ok`, `differences: 0` on a
   generated design — proves the move changed nothing observable.
3. New `core/tests/core_api.rs` integration tests: synthetic DEF+LEF
   (inline fixtures, same style as the existing Rust tests) driving
   `scan_instance_bboxes`, `macro_sizes`, `cell_attributes`/`is_sequential`,
   `LibraryIndex::open`/`.cell`/`.cell_attributes`/`.cell_source`, and the
   error paths (missing file, missing macro, unknown cell).
4. Offline proof: `CARGO_NET_OFFLINE=true cargo build --workspace --offline
   --locked` plus `uv build --wheel` (mirrors `vendor.yml`).
5. A `cargo tree -p open-eda-file-tools-core` check that pyo3/arrow/parquet
   are absent from the core graph (documented; asserted in CI via the
   workspace build succeeding without those deps — plus a comment in
   `core/Cargo.toml` listing the allowed deps).

### Documentation (docs-sync mandate, same commits)

- `docs/ai-api.md`: new "Rust core (`open-eda-file-tools-core`)" section
  with the public API above and the dependency note.
- `BUILD.md`: workspace layout, core crate, offline facts unchanged but
  paths updated; the consumer recipe (path/git dependency, vendored mirror).
- `CLAUDE.md`: architecture section updated — workspace members, the
  delegation invariant ("bindings delegate to core, never re-implement"),
  the no-pyo3/no-Arrow rule for core, and where each old file lives now.
- `README.md`: one-line mention of the Rust core crate.
- `Cargo.lock` updated; `scripts/check_vendored_crates.py` unchanged (no new
  external crates).

## What's left to the builder

- Exact file-split mechanics: which functions stay private inside core,
  exact struct field lists of core `Cell` (mirroring `CellData`), the
  `InstanceBBoxIter` implementation details (batch size reuse), and the
  mechanical translation of each pyclass to a delegating wrapper.
- Ordering/commit granularity of the tasks in the implementation plan.
- Doc wording and rustdoc details beyond the API contract above.

## Approval

The author (repo owner) filed the issue and asked for it to be handled on a
branch; per their standing preference, design approval defaults to yes and
the calls above are reported, not gated. The one judgment call with real
alternatives is the workspace shape (Approaches 1 vs 2 vs 3) — this doc
picks Approach 1; correcting it later costs one `Cargo.toml` move.
