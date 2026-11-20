# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

`open-eda-file-tools` is a single project with one `pyproject.toml`, one
`Cargo.toml`, and one version (`2026.11.9` CalVer) that ships several Python
libraries and command-line tools under the `open-eda-file-tools` umbrella:

- `liberty_tools` — Liberty `.lib` parser, writer, viewer and formatter
  (`liberty-view`, `liberty-tools format`, and the `liberty-format` wrapper).
- `lef_tools` — cell LEF parser and queries (`lef-tools`).
- `def_tools` — DEF parser, binary cache and design queries (`def-tools`).
- `sdc_tools` — SDC clock extraction (`sdc-tools`), evaluated in a real Tcl
  interpreter with stubbed SDC commands.
- `pex_tools` — SPEF/DSPF parasitic parser (`pex-tools`), parallel net-block
  parsing with a streaming low-memory mode.

## Repository Layout

```
pyproject.toml, Cargo.toml  Single project manifest; Cargo.toml is a workspace root
                            (members = ["core"]) and the bindings crate
core/                       Plain-Rust rlib `open-eda-file-tools-core` (issue #12):
core/src/lib.rs             Crate root: public re-exports and `Error`
core/src/liberty.rs         Liberty lexer/parser/evaluator/data model + LibraryIndex + public API
core/src/def.rs             DEF scanner, bbox/net-shape/clock logic, cache (no Arrow)
core/src/lef/               Shared LEF/DEF tokenizer (`lexer.rs`) + cell LEF parser (`cell_lef.rs`)
core/src/pex.rs             SPEF/DSPF parser, summaries, and per-net RC data
core/src/threads.rs         Process-wide CPU limit (`set_threads`)
src/                        The `_native` cdylib (pyo3 shell over core):
src/lib.rs                  Liberty pyclasses/pyfunctions + pymodule registration
src/def.rs                  DEF pyclasses/pyfunctions + the Arrow/Parquet writer
src/lef/                    LEF pyclasses/pyfunctions (python.rs)
src/pex.rs                  SPEF/DSPF pyclasses and pyfunctions
src/otw.rs                  OTW reader and Python bindings
src/threads.rs              `set_threads`/`get_threads` pyfunctions over core
liberty_tools/              Liberty Python wrapper and comparison module
viewer/, liberty_format/, logic2svg/
                            Browser viewer (`liberty-view`), formatter (`liberty-tools format`), symbol layout
lef_tools/                  Cell LEF Python package and `lef-tools` CLI
def_tools/                  DEF Python package, `def-tools` CLI, SDC reader, IO configurator
sdc_tools/                  SDC clock extraction, `sdc-tools` CLI (Tcl stub evaluator)
pex_tools/                  SPEF/DSPF parser, `pex-tools` CLI
tests/                      Unified pytest suite (Liberty, LEF, DEF, SDC, PEX, and `tests/gen/design_gen.py`)
core/tests/                 Rust integration tests for the plain-Rust public API
otw-design/                 Design workspace for the next OTW version (non-normative; `docs/otw.md` is the spec)
docs/                       AI-agent and per-tool Markdown documentation (`docs/ai-api.md`, `docs/cli.md`, `docs/*.md`)
```

## Documentation Synchronization Mandate (REQUIRED)

These tools are primarily consumed by AI coding agents. **Documentation MUST be
kept in sync with the code at all times** — never ship a CLI flag, subcommand,
output column, Python function, class, or behavioral change without updating the
corresponding documentation in the same change:

- `docs/ai-api.md` — single-page token-dense AI Agent Reference (decision table,
  typed Python signatures for all five packages, CLI invocations, invariants,
  and multi-tool recipes).
- `docs/cli.md` — command-line reference for all six `[project.scripts]` entry
  points.
- `docs/{liberty,lef,def,sdc,pex,otw,tools,installation}.md` — per-tool and setup guides.
- `README.md` and `CLAUDE.md` — repository overview and architectural invariants.
- `.pyi` type stubs (`liberty_tools/__init__.pyi`, `liberty_tools/_native.pyi`)
  and package docstrings.

## Commands

```bash
uv sync --dev
uv run maturin develop --release   # required after any Rust change (thin LTO; wheels use the fat-LTO `dist` profile)

# Tests
uv run pytest -q                   # Python tests across Liberty, LEF, and DEF (testpaths = tests/)
cargo test --workspace              # Rust unit tests (core + bindings)
cargo fmt --all --check            # Rust format check
cargo check --workspace             # Rust type check

# Single test
uv run pytest tests/test_extraction_api.py::test_1d_2d_and_3d_table_extraction -q

# Lint / type check
uv run ruff check .
uv run ty check
```
The large-file smoke tests need `generic80_ss_125c_1p116v_0p84v.lib` in the repo root; they skip otherwise.

Frontend-only sanity checks:

```bash
node --check viewer/static/app.js
node --check viewer/static/symbol.js
node --check logic2svg/src/layout.js
node --check logic2svg/src/render.js
uv run pytest -q tests/test_symbol_render.py
```

## Screen grabs

When the user says "look at the grab" or references a current screen grab, view the most recently modified image file in `~/Pictures/Screenshots`.

## Architecture

This is a **maturin/PyO3 hybrid over a plain-Rust core**: all parsing and
data storage lives in `core/` (`open-eda-file-tools-core`, an rlib with no
pyo3/Arrow/Parquet); the root crate is the thin `_native` cdylib whose
pyclasses delegate to or convert from core types. **The bindings never
re-implement parser logic**. They call `core::def`, `core::lef`,
`core::liberty`, and `core::pex`. A handful of accessors and writer shims
exist on both sides because pymethods call them on the pyclass. The Liberty
writer machinery (surgical attribute edits) is pure Rust in
`core::liberty` (`apply_edits_to_source`, `read_source_text`,
`write_source_text`, …); the bindings' pyclass methods delegate to it and
never hold a second copy of its logic.

```
core/src/liberty.rs        Rust: Liberty lexer, parser, boolean evaluator, data model, LibraryIndex
src/lib.rs                 pyo3: pyclasses/pyfunctions wrapping core, module `_native`
liberty_tools/__init__.py  Python wrapper classes (LibertyDocument, Cell, Pin, …)
liberty_tools/comparison.py  Python library comparison (arc matching, grid alignment)
liberty_tools/__init__.pyi Type stubs
tests/                     Pytest integration tests (inline fixture strings, no large files)
```

### Rust layer (`core/src/liberty.rs`)

The Liberty machinery is a single core file (plain Rust; the pyo3 shell in
`src/lib.rs` wraps it); it contains:
- **`Lexer`** — byte-level streaming tokenizer; handles `#`, `//`, `/* */` comments, quoted strings, backslash line continuation
- **`Parser`** — recursive descent; builds `*Data` structs (plain Rust) by consuming tokens
- **`BoolParser` / `BoolExpr` / `bool_implies`** — Liberty Boolean expression parser and implication checker for `when` filters; uses brute-force truth-table enumeration
- **`*Data` types, `Document`, `LibraryIndex`** — the parsed data model and the cell-offset index (`LibraryIndex::open`, `cell`, `cell_source`, ...)

`src/lib.rs` (the `_native` module) holds the pyo3 side: the `#[pyclass]` types (`Document`, `Cell`, `Pin`, `Bus`, `Bundle`, `BusType`, `TimingArc`, `TimingTable`) that own `Vec<*Data>` and expose methods to Python, and the `parse_file` `#[pyfunction]` (opens `.lib` or `.lib.gz`, constructs the parser with an optional cell filter).

There are two parallel type hierarchies: `*Data` (plain Rust structs in core, no PyO3 overhead) used during parse, and `#[pyclass]` types in `src/lib.rs` that wrap/clone them when returned to Python. This avoids PyO3 GIL overhead during bulk parse.

### Python layer (`liberty_tools/__init__.py`)

Each class wraps the corresponding native object via `self._native`. `LibertyDocument.to_polars()` calls `timing_tables()` and hands the result to `polars.DataFrame`. `parse_file` is the entry point for reading; `liberty_tools/comparison.py` holds the library comparison (`compare()` and its result types) and is re-exported from the package.

### Browser viewer (`viewer/`)

`viewer/server.py` is the FastAPI backend. `viewer/data.py` converts native
Liberty objects into JSON payloads for the static SPA in `viewer/static/`.

Static asset roles:

- `viewer/static/index.html` owns the shell layout. It must include
  `#content-grid`, `#view`, `#symbol-column #cell-symbol`, `#wave-section`, and
  `#source-section`; `app.js` and `style.css` assume that structure.
- `viewer/static/app.js` renders the tree, tables, Plotly plots, source pane,
  and cell-symbol panel.
- `viewer/static/symbol.js` is a hand-synced browser port of `logic2svg/src/*`.
  When logic-symbol parse/lower/layout/render behavior changes, update both
  copies and run `tests/test_symbol_render.py`.

2D table views keep the heatmap visible at the top. The lower plot defaults to
the existing 3D surface and can toggle to 2D wave families grouped by either
axis. `dc_current` remains special in 3D mode: current surface plus derived
resistance surface; 2D mode plots only raw current values.

Logic-symbol rendering lessons learned:

- Prefer output bubbles over input bubbles when an inverted child is a gate.
  Only leaf/variable inversion stays as an input bubble.
- Bubble radius is fixed at 4 SVG units; never derive it from row spacing.
- Equation text must never drive symbol/card width. Width comes from SVG
  geometry only; equation titles wrap/clamp inside that width and expose the
  full equation via `title`.
- Check actual screenshots/pixels for symbol work. Numeric tests catch geometry
  regressions, but visual badness has shown up before syntax/tests failed.

### Key behaviors to preserve

- **`when` filter uses implication, not equality**: `when="A"` matches `"A & B"` and `"A & !B"` but not `"A | B"`. Any changes to `bool_implies` must preserve this semantics.
- **Parse-time cell filter** (`cells=[...]` in `parse_file`) skips group bodies entirely via `skip_group_body()` — it is not a post-parse filter.
- **`timing_tables()` / `to_polars()` search pins inside buses and bundles** in addition to top-level cell pins (see the nested loops in `Document::timing_tables`).
- **3D table flattening**: values stored row-major across `(index_1 × index_2 × index_3)`; `table_position()` reconstructs `(i, j, k)`. The Polars column is `depth`, not `index_3_pos`.
- **`internal_power` is energy, not power**: the `rise_power`/`fall_power` table values are switching *energy* in joules. Scale = `voltage_unit × current_unit × time_unit` (`Document::energy_unit_joules`, exposed as `doc.energy_unit_joules()`); ASAP7 = `1e-15` (fJ). Extract via `internal_power_tables()` / `to_polars(kind="power")` (rows carry `related_pg_pin`; reuses `table_position`/`TimingTableData`). **Non-propagating energy** (input pin switches, no output switch) lives in input-pin `internal_power` groups with no `related_pin` and a 1-D table over input transition — these are included, with `related_pin` null.
- **One top-level group**: a valid library is a single `library` group. A second top-level group (typically a `cell` orphaned by a premature `library` close) is reported as an `unbalanced braces` error pointing at that group, not skipped — see the stray-`}` guard in `Parser::parse_document`. The ASAP7 SIMPLE-group libs ship with this exact defect (spurious `}` after `XOR2xp5r`, dropping `NAND5xp2R`).
- **CCSN noise data** lives *inside* `timing()` groups as `ccsn_first_stage` / `ccsn_last_stage`, parsed into `CcsnStageData`/`CcsnStage` (not the `tables` list) via `parse_ccsn_stage_body` — captures the scalars (`is_inverting`, `miller_cap_*`, `stage_type`, …), the 2-D `dc_current` IV surface, and `output_voltage_rise`/`_fall` vector sets (reuses `parse_timing_table_body`'s `.vectors`). `propagated_noise_*` is **deferred** (4-D, would need an `index_4` on the table primitive). Exposed as `arc.ccsn_stages()`.
- **GIL released during heavy work**: `parse_file`, `LibraryIndex::open`, `LibraryIndex::cell`, and `LibraryIndex::cell_source` run their pure-Rust parse/extract inside `py.detach(...)` (pyo3 0.28's renamed `allow_threads`) and convert `io::Error`/`ParseError` to `PyErr` **outside** the closure (error enums `ParseLoadError`/`CellLookupError`/`CellSourceError`). This lets many libraries parse concurrently; keep new heavy entry points GIL-free the same way.
- **Flat table values are row-major with `index_1` slowest**: `values[i * len(index_2) + j]` for 2-D, `values[i * len(index_2) * len(index_3) + j * len(index_3) + k]` for 3-D. `table_position()` and `flat_index()` must stay consistent with this; `TimingTable::lookup`/`value_at` are the supported way to read a point.
- **Axis order is per-table, not fixed**: which axis is the load and which is the transition comes from the table's `lu_table_template` and differs between libraries. `resolve_axis_vars` fills `TimingTableData::axis_vars` after the whole library body is parsed (templates may be declared after the cells that use them). `liberty_tools/comparison.py` aligns grids by that role — keep both sides in sync.
- **The writer is surgical**: `Document::to_string`/`write` re-read the original source and rewrite only the attributes changed through `Cell::set_attribute`/`Pin::set_attribute`; everything else stays byte for byte. The parser therefore records byte spans (`cell_spans`) and a source hash. Never re-emit a document from the parsed model — the model is lossy (comments, unknown groups, formatting) and would silently drop data.
- **Only the last edit of an attribute counts**: the edit log keeps every edit, but `apply_edits_to_source` walks it backwards and keeps the first occurrence of each `(cell, pin, container, key)` — the last edit made. Applying two edits to the same span would make the second land inside the first one's text.
- **The writer patches the entity the caller addressed**: a cell edit lands in the cell group, a pin edit in that pin's group. A pin edit that names its container (`bus`/`bundle`) is patched inside that container; a pin edit without one is patched in the only pin group of that name, and `resolve_pin_group` refuses (`ValueError`) when the name matches more than one group — never patch an arbitrary one.
- **Attribute values are written unquoted only when they match `^[A-Za-z0-9_+.\-]+$`**: every other value is written as a quoted string with `\` and `"` escaped (`render_attribute_value`), so the file parses back to exactly the value that was set. The rule is a safe pattern, not a blacklist of dangerous characters.
- **Attribute edits need an owning document**: `set_attribute` on a cell/pin that is not attached to a document raises `RuntimeError` rather than accepting an edit that could never be written back.
- **The compare module is `liberty_tools/comparison.py`**: the package re-exports it, so `lt.compare` is the *function* and `liberty_tools.comparison.Comparison` is the class. Keep the re-export (and the `.pyi` re-export) in place — the package attribute must stay the function.
- **Compare reports what it cannot compare, it never guesses**: a table whose unit dimension (time, capacitive load, voltage) differs between the two libraries is reported in `Comparison.unit_mismatches` as a `UnitMismatch`; a table whose axis roles are known on both sides but do not correspond is reported in `Comparison.axis_role_mismatches` as an `AxisRoleMismatch` (matching positionally would compare different quantities); a table whose `values` length does not match its grid is reported in `Comparison.malformed_tables` as a `MalformedTable`. None of them is diffed or interpolated. `Comparison.has_differences` is true for any of those, and `summary()` counts every kind of difference.
- **`value_at` raises `IndexError` for a negative position**: a negative `row`/`col`/`depth` is out of range like any other position, so it is reported as an `IndexError` and not as a conversion overflow. `lookup()` matches *axis values* (not grid positions) and returns `None` for a coordinate that is not an axis value; `value_at()` is the by-grid-index accessor.
- **`skip_tables=True` is the structural parse.** It keeps cells, pins, arcs
  and scalars identical but skips floating-point table bodies at the byte
  level; the lexer's skipper honors strings and comments and still updates
  line/column and the source hash. Table lists come back empty. Measured on
  the IHP sg13g2 standard-cell library: 23.8 ms → 15.3 ms (36% faster) with
  identical structural output.
- **Minimum pulse width has two forms**: the `min_pulse_width_high`/`_low` pin attributes and a `min_pulse_width` timing arc with `rise_constraint`/`fall_constraint` tables. `pin.min_pulse_width()` normalizes both and reports which form it read in `MinPulseWidth.source`; the attribute form wins.

## LEF package (`lef_tools/`)

`lef_tools` owns the cell LEF parser and the shared tokenizer:

```
core/src/lef/lexer.rs     streaming LEF/DEF tokenizer (also used by `core/src/def.rs`)
core/src/lef/cell_lef.rs  MACRO / PIN / PORT / RECT model in microns
lef_tools/           Python wrapper and `lef-tools` CLI
```
### Key behaviors to preserve

- **LEF coordinates stay in microns.** `parse_lef` never applies a DEF's
  `UNITS DISTANCE MICRONS`; DEF-derived outputs stay in database units (DBU).
- **Unsupported port geometry is flagged, not dropped.** `POLYGON`, `PATH` and
  `VIA` set `port.has_unsupported_geometry` while the port's `RECT`s are still
  returned. Rects are normalized so `x0 <= x1` and `y0 <= y1`, but negative
  and out-of-boundary values are preserved — power rails routinely extend past
  the macro boundary.
- **Duplicate macros: the last definition wins**, and every duplicated name is
  reported once by `library.duplicate_macro_names()`.
- **Errors name the input.** Malformed LEF raises `ValueError` naming file,
  macro and pin; unreadable files raise an `OSError` subclass; unknown
  `library.macro()` / `macro.pin()` names raise `KeyError`.
- **`__version__` is the installed distribution version, with no git fallback.**
  CalVer releases (`YYYY.M.D`, same-day `YYYY.M.D.N`) ship the release version
  on HEAD until the next release, so `version.py` reads installed metadata and
  nothing else.

## DEF package (`def_tools/`)

`def_tools` owns the DEF pipeline and uses `crate::lef` for macro sizes:

```
core/src/def.rs           DEF parser, streaming reads, binary cache
src/def.rs                DEF pyclasses/pyfunctions and the Arrow/Parquet writer (CSV/table writers are core's `write_*_impl`)
def_tools/cli.py          click CLI (`def-tools <command>`)
def_tools/instances.py    instance bbox dumper (`def-tools instances`, CSV/Parquet/table in microns)
def_tools/sdc.py          clock roots for `clock-instances` (adapter over `sdc_tools.clocks`)
def_tools/clock_roles.py  Liberty cell clock roles (through pins, endpoints, clock gates)
def_tools/clock_data.py   clock/data classification backends and net/pin -> instance resolution
def_tools/otw.py          OTW (Open Timing Windows) reader/writer; parser in `src/otw.rs`
def_tools/io_config.py    storage benchmark / IO policy config
tests/gen/design_gen.py   synthetic LEF/DEF/SDC design generator
```

### Key behaviors to preserve

- **No design files are committed.** Tests generate inputs with
  `tests/gen/design_gen.py`; assertions compare against the generator's
  `manifest.json`, never against values recomputed in the test — a parser bug
  and a generator bug must not cancel out. Details in
  `docs/test-collateral.md`.
- **Two fixture kinds are not interchangeable.** Inline synthetic DEF/LEF
  written to `tmp_path` for one construct; generated designs for scale, cell
  mix and section proportions. `tests/test_real_world_def.py` and
  `tests/test_perf.py` use the generator. Tests marked `perf` are skipped
  without `--runperf`.
- **Instance bboxes need LEF macro sizes.** `instance_bboxes` calls
  `crate::lef::parse_library` for `SIZE`/`ORIGIN` and converts to the DEF's
  database units; it does not expose the LEF classes.
- **Instance columns are conditional on their inputs.** The canonical six
  (`instance,base_cell,llx,lly,urx,ury`) need only DEF+LEF; `is_seq` exists
  only when a Liberty library is supplied (`ff`/`latch` groups) and `is_clock`
  only when a clock source is supplied (`--clock-cells`, `--clock-data`,
  `--twf`). Selecting a column whose input is absent is an error, never a
  silent false column. `is_seq` parses each
  library with `skip_tables=True`, one thread per library; only structural
  fields are needed.
- **`is_clock` sources are backends that answer, not formats.**
  `def_tools/clock_data.py` defines the seam: a backend maps one file to a
  role (`c`/`d`/`u`/`z`) per net, pin or instance (`ClockData`), and this repo
  resolves clock objects to instances. Built-ins are `otw` and `names`; any
  other name (`twf` included) is a plug-in via the
  `open_eda_file_tools.clock_data` entry-point group, the
  `OPEN_EDA_CLOCK_DATA_BACKENDS` env var, or `register_backend`. Built-in
  names cannot be replaced.
- **No vendor TWF reader lives in this repo.** There is no open TWF format;
  vendor readers are plugged in from outside. `--twf PATH` is only shorthand
  for `--clock-data twf:PATH`. The open format here is OTW (`docs/otw.md`,
  `src/otw.rs`), designed from scratch — do not shape it after a vendor file.
- **OTW groups are independent, and the reader depends on it.** The body is
  `n` groups and `i` groups (`i <instance>` then its `p` lines); `w` lines
  belong to the object above them. `parse_otw` (`src/otw.rs`) cuts the input
  at lines starting with `n`/`i`/`e` and parses the blocks with rayon, one
  `Sink` per block, merged in file order. Never add a record that refers to
  another group or that makes group order significant — that needs a new
  format version. `docs/otw.md` is the normative spec; change it with the
  reader.
- **OTW body numbers are fixed-point decimals** (no exponent); `real` with an
  exponent is allowed only in the header and `freq`. The reader validates
  decimals by byte scan and never converts window values.
- **The OTW `end <nets> <pins> <windows>` trailer is mandatory** and checked:
  no `end` means truncated, wrong counts or trailing records are errors.
- **An OTW file is one analysis corner.** Header values (`corner`, `process`,
  `voltage`, `temperature`) are single-valued; multi-corner data is one file
  per corner, never best/typ/worst triples in one file.
- **Backend missing and source malformed are distinct, and neither yields a
  column.** `ClockDataBackendError` exits 3, `ClockDataFormatError` exits 4;
  `is_clock` is never written as all-false for an unreadable source. Clock
  objects absent from the DEF and instances whose macro has no Liberty cell
  are reported (`ClockInstances.unmatched_*`, `.unclassified`), never guessed.
- **One role per object; clock dominates.** An OTW object is declared once.
  A backend whose source has several records per object must report `c` when
  any record is a clock record (real per-edge files tag every clock net as
  data too).
- **Clock nets mark their drivers, including sequential endpoints.**
  `clock_object_instances` in `core/src/def.rs` also marks explicitly
  selected output and clock-through pins. An endpoint's input never marks
  it. Names compare with DEF backslash escapes removed.
- **The binary cache is versioned.** Magic headers and `CACHE_VERSION` guard
  the on-disk format; components, raw net records, a fixed-offset net index, a
  name index and connectivity live in separate files so single-net queries read
  only matching records. Bump `CACHE_VERSION` when the layout changes.
- **Heavy work releases the GIL** via `py.detach(...)`; batch and parallelize
  in Rust (rayon), never by loading a whole DEF into Python.
- **All parallel native work runs inside `crate::threads::install`** (or
  `install_for`). That is what makes `set_threads(n)` / `--threads N` a real
  CPU cap: the default is all logical CPUs, and the limit works even after
  rayon's global pool has started (`RAYON_NUM_THREADS` does not). A new
  `par_iter` outside it silently ignores the user's limit. CLIs call
  `set_threads` before any native work, Liberty parsing included.
  `install`/`install_for` are re-entrant: on a thread that already belongs to
  one of our pools they run the work as is, so a nested install keeps the
  outer budget and never starts a second pool. Hence every `pub` helper that
  runs rayon work installs itself (`parse_components`, `render_rows`,
  `process_net_record_batch`, ...) with no cost when the caller already did.
- **The core crate has three visibility tiers.** Documented API (plain `pub`,
  listed in `docs/ai-api.md` section 8); items the bindings use but that are
  not supported API (`#[doc(hidden)] pub`, free to change in any release); and
  everything else, `pub(crate)` or private. A new item starts in the third
  tier; promote it only when the bindings or the documented API need it, and
  a type that appears in a documented signature or field is documented too.
- **Scan and parse overlap only within the limit.** `for_each_batch`
  (`core/src/def.rs`) runs the sequential scanner on its own thread one batch ahead
  and gives the parallel part `n - 1` workers; with `n = 1` nothing overlaps.
- **Output never depends on batching or thread count.** Batches parse in
  parallel chunks and are reassembled in record order; the first error in
  record order is the one reported. Long connection lists split only at a
  `(` that follows `)` (`split_connection_list`), never inside quotes.
- **The tokenizer scans slices, not bytes.** `core/src/lef/lexer.rs` finds record
  ends with memchr and word ends with a byte-class table; records the caller
  does not need are consumed with `skip_until_semicolon` (no copy), and a
  `-` record outside the sections a pass reads is skipped whole, so names
  like `NETS` inside VIAS/PINS records are never read as section keywords.
  The connectivity pass keeps only a net's connection list
  (`read_until_plus_or_semicolon`), never its routing.
- **Connectivity is flat and id-based** (`core/src/connectivity.rs`). Instance
  ids `0..component_count` follow COMPONENTS order (duplicates keep both
  entries; nets resolve to the last definition); names seen only in nets get
  later ids in first-seen order; `PORT = u32::MAX` is `( PIN name )`. Adjacency
  lists entries in net order, then connection order, and is built sequentially.
  `+ SYNTHESIZED` connections are accepted. A single COMPONENTS section after
  NETS/SPECIALNETS is fine (their records are held until it ends); a second
  COMPONENTS section, once nets were resolved, is an error naming the byte, in
  the DEF-backed readers and in cache creation alike. Clock trace, clock-object resolution
  and `clock-instances` all run on it; do not add per-connection strings.
- **All route geometry comes from one state machine.** `parse_net_shapes_record`
  (`core/src/def.rs`) drives the direct scan, the Python iterator and the cache
  query. `read_route_point` resolves `*` against the previous point (an error
  when there is none), optional extension values, `VIRTUAL` (previous point
  updated, no shape) and `MASK`; `RECT ( dx dy dx dy )` deltas and net-level
  `+ RECT layer pt pt` both yield `shape_type = "RECT"` rows with normalized
  corners and `width = 0`; after a via the layer is unknown until the next
  `NEW`/status keyword, and those segments are skipped and counted. Route
  pieces the reader cannot place are counted (`after_via`, `polygon`,
  `subnet` for subnets that carry wiring), never attributed to a guessed
  layer; the CLI warns when any count is non-zero, and the writers/iterator
  return/expose the counts. Net-level `+ SHAPE`, `+ STYLE` and `+ MASK`
  continue the current path; a status followed by a layer starts one; every
  other net-level keyword ends it. A status with no layer (DEF 5.8
  `+ FIXED + RECT ...`) starts no path: it is the `route_status` of the next
  `+ RECT`/`+ POLYGON`/`+ VIA` only, and a `+ RECT` without one is `ROUTED`,
  as in the DEF reference parser (`def.y`). Bare words outside a path are
  ignored. Coordinate arithmetic is checked: out-of-range values are errors
  naming the DEF byte, never wrapped.
- **`__version__` is the installed distribution version, with no git fallback.**
  CalVer releases (`YYYY.M.D`, same-day `YYYY.M.D.N`) ship the release version
  on HEAD until the next release, so `version.py` reads installed metadata and
  nothing else.

## SDC package (`sdc_tools/`)

`sdc_tools` extracts the clocks an SDC creates:

```
core/src/sdc/       tclsh >= 8.6, safe interpreter, stub prelude
core/src/sdc.rs     create_clock / create_generated_clock records
sdc_tools/clocks.py Python wrapper and clock dataclass
sdc_tools/clock_tree.py  forward trace from clock roots through DEF connectivity
sdc_tools/cli.py     `sdc-tools clocks` / `sdc-tools clock-cells`
```

### Key behaviors to preserve

- **SDC runs in a real Tcl interpreter, never a subset parser.** The file is
  evaluated in `interp create -safe`; SDC commands are stubs that record their
  arguments, and a `proc unknown` inside the slave records every unimplemented
  command and returns `{}`, so `set_clock_groups`, `set_false_path` and vendor
  commands never abort a run.
- **Tcl >= 8.6 is an environment prerequisite.** `TCLSH` overrides discovery;
  otherwise `tclsh8.6`, then `tclsh`. There is no static-parser fallback.
- **Collections stay symbolic**: `port:clk`, `pin:div/Q`, `net:clk`,
  `cell:u1`. Resolving names against DEF/LEF/Liberty is a separate pass.
- **The Tcl prelude emits length-prefixed UTF-8 fields.** Keep it Tcl
  8.6-compatible and preserve symbolic collections as separate endpoints.
- **Clock-tree cells come from connectivity.** `def::clock_tree_cells` seeds
  nets from SDC roots and follows combinational cells from input to output.
  Endpoint inputs stop the trace. An endpoint that drives a clock net is
  included. Each instance's nets are expanded on the first visit, so the
  queue stays O(nets + connections) here and in `traverse_clock_instances`.
- **One Liberty clock-role model, shared.** `def_tools/clock_roles.py`
  (`CellClockRole`) feeds both the SDC trace and clock-data resolution. An
  endpoint is a non-ICG cell with `ff`/`latch` or a `clock : true` pin
  (memories). An integrated clock gate (`clock_gating_integrated_cell`) is
  *not* an endpoint: its only clock-through input is its clock pin, so the
  trace passes CK→GCK and a clock on the enable/test pin never marks it.
  `is_seq` is unchanged (ICGs have a latch, so they are `is_seq`).
- **The trace is pruned to clock pins by default.** The backward pass in
  `def::clock_tree_cells` keeps paths reaching clock pins. Sequential
  drivers on explicit clock roots are also kept. `prune=False` or
  `--no-prune` keeps every touched candidate.
- **Liberty metadata parses one thread per library.** `cell_clock_roles` parses
  each library with `ThreadPoolExecutor`; `parse_file` releases the GIL, so the
  set parses concurrently. Later libraries win for duplicate cell names.
- **The safe interpreter hides `exec`, `open`, `source`, and `load`.**

## PEX package (`pex_tools/`)

`pex_tools` reads SPEF and DSPF:

```
src/pex.rs             parser: header/name map, then Rayon over net blocks
pex_tools/__init__.py  Python API (`header`, `summaries`, `histogram`, `parse`)
pex_tools/cli.py       `pex-tools summary|histogram|nets`
docs/pex/              format notes; MIT Parser-SPEF docs; `local/` is gitignored
```

### Key behaviors to preserve

- **Header and `*NAME_MAP` are sequential; net blocks are parallel.** Every
  `*D_NET`/`*R_NET` block is an independent Rayon task; results keep source
  order. `mode="stream"` buffers one block at a time for a low-memory pass.
- **SPEF and DSPF share the section skeleton.** DSPF is distinguished by
  connection attributes (`*C x y`, `*L`, `*S`, `*D cell`) and `$` comments;
  both are parsed.
- **`$` extensions are preserved verbatim** on caps, resistors and
  connections (`ext`), so layer/coordinate metadata is not lost. `//` and
  `/* ... */` comments are accepted.
- **Names resolve through `*NAME_MAP`** using the header delimiter; unresolved
  tokens stay as written.
- **Physical (`*D_PNET`/`*R_PNET`) and reduced-RSPF sections are skipped**;
  they are not silently interpreted as electrical nets.

## Liberty spec reference

The authoritative Liberty syntax/semantics reference is `Liberty_User_Guides_and_Reference_Manual_Suite_Version_2017.06.pdf` (1458 pages, gitignored — local only). When parser behavior is ambiguous, consult it instead of guessing.

The PDF's printed TOC uses chapter-relative page numbers (e.g. `2-3`), which the Read tool can't use. Two gitignored sidecars map topics to **absolute** PDF pages (`pypdf` is a dev dependency; regenerate if missing):

- `liberty_spec_toc.txt` — section title → absolute page, from the PDF outline. Grep this first for "where is the section on X". Regenerate: walk `PdfReader(pdf).outline`, use `get_destination_page_number(item)+1`.
- `liberty_spec.txt` — full text dump for exhaustive keyword sweeps. Pages are `\f`-separated, so awk record number = absolute page. Regenerate: `pdftotext -layout <pdf> liberty_spec.txt`.

```bash
grep -i bus_naming_style liberty_spec_toc.txt                                   # section → page
awk -v RS='\f' 'tolower($0) ~ /bus_naming_style/ {print "page " NR}' liberty_spec.txt  # every mention → page
```

Then `Read` the PDF at that page range (`pages: "N-M"`, ≤20 pages/request) to see tables/figures rendered.
