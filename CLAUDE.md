# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

`open-eda-file-tools` hosts EDA data-file tooling. The repository ships three
tools, each its own Python distribution:

- `liberty_tools` / `liberty-tools` — Liberty `.lib` parser, writer, viewer and
  formatter (`liberty-view`, `liberty-format`), at the repository root.
- `lef_tools` / `lef-tools` — cell LEF parser and queries (`lef-tools`), in
  `lef_tools/`.
- `def_tools` / `def-tools` — DEF parser, binary cache and design queries
  (`def-tools`), in `def_tools/`.

## Repository Layout

The repository root is the Liberty tool's project (`liberty_tools`, Rust
backend in `src/`, tests in `tests/`). Every other tool is a self-contained
project in its own directory at the root — currently `lef_tools/` and
`def_tools/`, with more planned (VCD, …). Each tool directory has its own
`pyproject.toml`, distribution, version, Rust crate, `tests/` and
command-line entry points — see `user_guide/tools.qmd`. Do not add one
format's code to another tool's package.

```
liberty_tools/, viewer/, liberty_format/, logic2svg/
                           Liberty tool: package, Rust `src/`, tests, frontend
lef_tools/                 Cell LEF project root (own pyproject and venv)
  Cargo.toml               Rust crate `lef-tools-native` (cdylib + rlib)
  rust/src/lexer.rs        Shared LEF/DEF tokenizer
  rust/src/cell_lef.rs     LEF MACRO / PIN / PORT / RECT parser
  src/lef_tools/           Python wrapper and CLI
def_tools/                 DEF project root (own pyproject and venv)
  Cargo.toml               Rust crate `def-tools-native`; links `lef-tools-native`
  rust/src/lib.rs          DEF parser, binary cache, geometry kernels
  src/def_tools/           Python wrapper, CLI, SDC reader, IO configurator
  tests/                   pytest suite and the synthetic design generator
```

## Commands

```bash
# Liberty (repository root)
uv sync --dev
uv run maturin develop --release   # required after any Rust change

# Tests
uv run pytest -q                   # Python tests (testpaths = tests/)
cargo fmt --all --check            # Rust format check, whole workspace
cargo check --workspace            # Rust type check, whole workspace

# Single test
uv run pytest tests/test_extraction_api.py::test_1d_2d_and_3d_table_extraction -q

# Lint / type check
uv run ruff check .
uv run ty check
```

LEF and DEF each have their own project and virtualenv:

```bash
cd lef_tools
uv sync --dev                      # installs deps and builds the extension
uv run maturin develop --release   # after any Rust change
uv run pytest -q
uv run ruff check . && uv run ty check

cd ../def_tools
uv sync --dev
uv run pytest -q
uv run ruff check . && uv run ty check

# Rust unit tests, from the repository root:
cargo test -p lef-tools-native -p def-tools-native
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

When the user says "look at the grab" or references a current screen grab, view the most recently modified image file in `/mnt/c/Users/myles/Pictures/Screenshots`.

## Architecture

This is a **maturin/PyO3 hybrid**: all parsing and data storage lives in Rust (`src/lib.rs`); Python is a thin wrapper.

```
src/lib.rs                 Rust: lexer, parser, boolean evaluator, PyO3 #[pyclass] types
liberty_tools/__init__.py  Python wrapper classes (LibertyDocument, Cell, Pin, …)
liberty_tools/comparison.py  Python library comparison (arc matching, grid alignment)
liberty_tools/__init__.pyi Type stubs
tests/                     Pytest integration tests (inline fixture strings, no large files)
```

### Rust layer (`src/lib.rs`)

Single file containing:
- **`Lexer`** — byte-level streaming tokenizer; handles `#`, `//`, `/* */` comments, quoted strings, backslash line continuation
- **`Parser`** — recursive descent; builds `*Data` structs (plain Rust) by consuming tokens
- **`BoolParser` / `BoolExpr` / `bool_implies`** — Liberty Boolean expression parser and implication checker for `when` filters; uses brute-force truth-table enumeration
- **`#[pyclass]` types** (`Document`, `Cell`, `Pin`, `Bus`, `Bundle`, `BusType`, `TimingArc`, `TimingTable`) — own `Vec<*Data>` and expose methods to Python; registered in `_native` module
- **`parse_file` `#[pyfunction]`** — entry point; opens `.lib` or `.lib.gz`, constructs parser with optional cell filter

There are two parallel type hierarchies: `*Data` (plain Rust structs, no PyO3 overhead) used during parse, and `#[pyclass]` types that wrap/clone them when returned to Python. This avoids PyO3 GIL overhead during bulk parse.

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
- **Minimum pulse width has two forms**: the `min_pulse_width_high`/`_low` pin attributes and a `min_pulse_width` timing arc with `rise_constraint`/`fall_constraint` tables. `pin.min_pulse_width()` normalizes both and reports which form it read in `MinPulseWidth.source`; the attribute form wins.

## LEF package (`lef_tools/`)

`lef_tools` owns the cell LEF parser and the shared tokenizer:

```
lef_tools/Cargo.toml            crate `lef-tools-native` (cdylib + rlib)
lef_tools/rust/src/lexer.rs     streaming LEF/DEF tokenizer (also linked by DEF)
lef_tools/rust/src/cell_lef.rs  MACRO / PIN / PORT / RECT model in microns
lef_tools/src/lef_tools/        Python wrapper and `lef-tools` CLI
```

The crate is both the extension module (`lef_tools._lef`) and an rlib that
`def_tools` links for the tokenizer and macro `SIZE`/`ORIGIN`. The Python
packages stay independent.

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
- **`__version__` is the installed distribution version inside this repo.**
  `version.py` consults git only when the tool's own project directory is a git
  root; monorepo tags and commit counts must not leak into it.

## DEF package (`def_tools/`)

`def_tools` owns the DEF pipeline and links `lef-tools-native`:

```
def_tools/Cargo.toml            crate `def-tools-native`
def_tools/rust/src/lib.rs       DEF parser, streaming reads, binary cache
def_tools/src/def_tools/cli.py  click CLI (`def-tools <command>`)
def_tools/src/def_tools/sdc.py  SDC `create_clock` / generated-clock reader
def_tools/src/def_tools/io_config.py  storage benchmark / IO policy config
def_tools/tests/gen/design_gen.py     synthetic LEF/DEF/SDC design generator
```

### Key behaviors to preserve

- **No design files are committed.** Tests generate inputs with
  `tests/gen/design_gen.py`; assertions compare against the generator's
  `manifest.json`, never against values recomputed in the test — a parser bug
  and a generator bug must not cancel out. Details in
  `def_tools/docs/test-collateral.md`.
- **Two fixture kinds are not interchangeable.** Inline synthetic DEF/LEF
  written to `tmp_path` for one construct; generated designs for scale, cell
  mix and section proportions. `tests/test_real_world_def.py` and
  `tests/test_perf.py` use the generator. Tests marked `perf` are skipped
  without `--runperf`.
- **Instance bboxes need LEF macro sizes.** `instance_bboxes` calls
  `lef_tools_native::parse_library` for `SIZE`/`ORIGIN` and converts to the
  DEF's database units; it does not expose the LEF classes.
- **The binary cache is versioned.** Magic headers and `CACHE_VERSION` guard
  the on-disk format; components, raw net records, a fixed-offset net index, a
  name index and connectivity live in separate files so single-net queries read
  only matching records. Bump `CACHE_VERSION` when the layout changes.
- **Heavy work releases the GIL** via `py.detach(...)`; batch and parallelize
  in Rust (rayon), never by loading a whole DEF into Python.
- **`__version__` is the installed distribution version inside this repo.**
  `version.py` consults git only when the tool's own project directory is a git
  root; monorepo tags and commit counts must not leak into it.

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
