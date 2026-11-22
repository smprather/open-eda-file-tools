# Liberty tools performance implementation plan

**Goal.** Reduce parse time, interactive latency and peak memory across the Rust and Python Liberty APIs, formatter, viewer, comparison engine, writer and downstream clock tools. Preserve the observable results of the public entry points, with the two explicit corrections described in Tasks 3 and 4.

**Design.** Fix repeated work where it originates. Read the input in blocks, recognize skipped groups with the same lexical rules as the parser, share parsed data across Python handles, and construct results in the representation the caller requests. Keep viewer state scoped to the request, selection or server process that owns it.

**Stack and baseline.** Rust core, PyO3 0.28, Python 3.14, Polars, FastAPI and browser JavaScript. Review baseline `ed884cc62445cb6222a7d80c715f23487ffbfa85`. The requirements and measurements from the November 20 review are included here. The [handoff](2026-11-20-liberty-performance-handoff.md) records the starting state and first execution commands. For task-by-task execution the applicable Superpowers workflows are executing-plans or subagent-driven-development.

## Evidence and acceptance criteria

These are local measurements. “Prototype” means an isolated experiment. Rerun baseline and candidate under the same build profile on the same machine. Use exact outputs, operation counts and growth curves for regression tests. Wall-clock values below are comparison points.

| Workload | Baseline and prototype evidence | Acceptance signal |
|---|---|---|
| Full parse, IHP SG13G2 typical 1.20 V, 25 °C, 84 cells, 1,720,231 bytes | 32 KiB refill changed plain 24.8 → 18.4 ms and gzip 70.2 → 20.2 ms. A decoder-side buffer alone gave 68.5 → 25.8 ms on another run. | Bulk reads on both inputs, matching inventories, values, spans, source hash and errors. |
| Select one cell, same IHP file | Token skip 18.7 ms, byte skip 12.7 ms. The initial prototype failed on a postfix apostrophe. | Faster selection and a correct following cell on the apostrophe fixture. |
| Structural parse and attribute projection | `dev.lib`'s buffer retained 196 power-current vectors and 2,950 values with tables disabled. For 84 indexed IHP cells, attribute projection alone took 14.1 → 12.0 ms, excluding index open. | Zero retained waveform samples in structural mode, identical scalar attributes, sequential flags and cell order. |
| Viewer | `DFFHQNx1_ASAP7_6t_R` tree took 29 parses and 69.2 ms, request-local reuse 1 parse and 3.2 ms. Four concurrent cold callers opened four indexes on a generated 31.8 MB file. | One parse per tree, one successful open per process under concurrent requests, equivalent JSON. |
| Python traversal and clock roles | All-cell area traversal on 4k, 8k and 16k two-pin cells took 25.7, 105.8 and 410.3 ms. Structural parsing itself took 9.5, 19.4 and 41.8 ms. On IHP, computing all pin roles took 2.07 ms versus 0.51 ms for sequential flags alone, excluding parsing. | Near-linear traversal and no table-size-dependent clone for scalar reads. Measure complete CLI costs as well as traversal. |
| DataFrame extraction | IHP's 52,442 timing points gave a 4.6 MiB frame and a 126 MiB increase in peak RSS. Generated 2,000 cells × 49 points gave a 5.9 MiB frame and about a 218 MiB increase. | Eliminate per-point Python dictionaries on the frame path. Record lower peak RSS plus matching schema and contents. |
| Formatter | A 4,888,906-byte generated library with 100k cells produced 1,600,006 tokens per side. Reindent took ~535 ms. Building both token lists took another ~412 ms and brought peak RSS to ~292 MiB. | Exact verification uses memory proportional to the largest active tokens and read buffers. Measure reindent and end-to-end RSS separately. |
| Comparison | On `dev.lib`, 3,912 points took 17.85 → 12.08 ms with equal-grid matching. On a 1,024-sample axis, 108 → 2.10 ms. Reversed 128-arc, seven-input minterm groups took 280 → 6.89 ms with cached keys. One-pass summary took 1.65 → 0.36 ms on 3,912 entries. | Equal ordered entries, diagnostics and summaries. Count condition builds per distinct expression and benchmark long axes. |
| Bulk editing | One length-changing area edit per cell, each cell with a 2 KiB comment. Rendering 500/1k/2k/4k edits took 10/26/76/258 ms. Forward assembly took 9/15/26/53 ms. | Exact bytes, near-linear assembly and preserved handling of duplicate names and missing targets. |
| Browser and combo builder | Source annotation on 2k/4k/8k sibling vectors took 21/70/293 ms in Node. The bundled Plotly script is 4.56 MB. Skipping nested rename regex checks on 16 gzip passes took 126 → 108 ms. | Near-linear source annotation, charts load on demand, bounded retained source and equal decompressed combo output. |

## Execution order and compatibility

Start with Task 1. Tasks 2 → 3 → 4 are the parser sequence. Task 5 is an independent early viewer win. Task 6 supplies shared handles for subsequent Python traversal and export work. Tasks 7–11 can be reviewed separately. Task 12 follows the server and tree behavior established in Task 5. Task 13 is independent. Finish with Task 14. Rebuild the native extension after Rust changes before running Python tests.

- Keep `core` usable as an offline Rust library. PyO3, Arrow and Polars integration belongs in the binding crate or Python package. Use existing vendored dependencies.
- Preserve file order and duplicates in document listings. Full-document named lookup and writer target the first cell of a name. The lazy index currently lists a name once in first-seen order and stores the last range. Pin and bus ownership and detached-handle errors are observable.
- Preserve ordered comparison results, axis-role alignment, float tolerance, source byte offsets, CRLF, and unchanged-file verification. The indexed scanner has intentional tolerance for stray closing braces that the full parser rejects.
- Check the repo's documentation and stub synchronization instructions in `CLAUDE.md` when changing a documented behavior or interface. Use spaces for code indentation.

## Implementation tasks

The [implementation handoff](2026-11-20-liberty-performance-handoff.md) records the verified change set, measurements, and optional skips.

### Task 1 — Establish comparable workloads

- [x] Add a small opt-in runner in `scripts/liberty_bench.py` following `scripts/def_bench.py`. Use a temporary directory for generated inputs and JSON output for results. Accept a workload selector and optional input path. Keep large cases outside default pytest. Add each benchmark alongside its owning task so the first change stays small.
- [x] Start with checked-in `dev.lib`, a gzip copy of the same bytes, and generated cells named with fixed-width numbers. Include the local IHP file when available at `/home/mylesp/pdk/IHP-Open-PDK/ihp-sg13g2/libs.ref/sg13g2_stdcell/lib/sg13g2_stdcell_typ_1p20V_25C.lib`. For other machines, synthetic and checked-in cases must run independently.
- [x] Record revision, native module path, build profile, input digest and byte counts, workload size, Python/Polars versions, iterations and machine. Warm up once, measure at least five runs for timings and report the median. For memory, launch a fresh worker per case, import Polars before sampling its baseline, record process high-water RSS before and after, and report frame size separately. Exclude fixture generation, loading, or parsing only when the metric is explicitly labeled that way.
- [x] Capture a release baseline before changing the parser. Store command lines and results with the implementation review. Generate expected fixture contents independently or compare against the baseline on identical inputs. Use exact row order, optional values, raw bytes and diagnostics where a digest could hide the nature of a failure.

**Reproduction inputs.** For traversal, generate one library with fixed-width unique cell names, area 1, one input and one output. For frame memory, give each of 2,000 cells one timing arc with one 7×7 table. For formatter memory, use 100k minimal one-input cells in a single library. For comparison, generate all 128 minterms of seven inputs in a single pin/timing-type/sense group, reverse the second library's declaration order, and attach a two-point table to each arc. For bulk edits, grow each area from `1` to `1.12345`. For browser annotation, generate sibling `vector` groups under the same template and time the pure JS annotation function. Keep a small version of each fixture for output checks.

### Task 2 — Read Liberty input in blocks

**Files.** `core/src/liberty.rs` (`Lexer::{fill_peek,peek,peek_next,bump}` and `open_input`), Rust lexer tests, `tests/test_writer.py`.

- [x] Add a Rust reader that returns short, irregular chunks and counts read sizes. Compare the same token kinds, text, positions, end offsets and final source hash under tiny chunks and normal reads. Put quotes, escaped newlines, CRLF, comment delimiters and EOF across refill boundaries. Include an injected I/O error.
- [x] Replace the single-byte read/queue with a 32 KiB refill buffer and cursor. Carry unread lookahead across refills. Advance the source hash and positions exactly once for each consumed byte. Keep streaming file and decoder support. Use the shared lexer for parsing, token comparison and indexed cell parsing.
- [x] Run `cargo test -p open-eda-file-tools-core --offline --locked`, rebuild the extension and run the writer, structural and index suites. Time plain and gzip full parse on identical bytes. Confirm the reader makes block-size requests and the unchanged document renders byte-for-byte.

### Task 3 — Make group skipping agree with tokenization

**Files.** `core/src/liberty.rs` (`Lexer::next_token`, `skip_braces`, `index_cells`, `match_braces`, `skip_to_semicolon`, `Parser::parse_library_body`), `tests/test_index.py`, `tests/test_library_index.py`, `tests/test_structural_parse.py`.

- [x] Add this regression fixture before changing the fast path. Full parse must return both names. Indexed source for `keep` must be exactly its cell group, and a selected parse for `keep` must return area 1.

  ```text
  library (t) {
    cell (omit) { pin (Z) { function : A'; } }
    cell (keep) { area : 1; }
  }
  ```

- [x] Model token boundaries in the byte scanner. A quote starts a string where the lexer would start a token. A postfix apostrophe within a bare word belongs to that word. Mirror the lexer's rules for escaped bare-word bytes, quoted strings, continuations, comments and braces. Apply the rule to the in-memory header/cell scan as well as the streaming group skipper. Keep accurate trailing offsets, line/column reporting and the source hash for selected-document writes.
- [x] Differentially check cell names, ranges and selected contents on plain/gzip fixtures with quoted braces, comment markers in strings, punctuation adjacent to words, empty groups, nested groups, late templates and valid cells after an excluded cell. Test malformed unterminated strings/comments, unexpected EOF, and each entry point's existing policy for stray braces.
- [x] Switch the excluded-cell branch to the verified byte skipper. Benchmark choosing the first, last and absent name in a many-cell file. Keep the required whole-stream scan for late declarations and source verification.

### Task 4 — Skip CCS power-vector data and project indexed attributes

**Files.** `core/src/liberty.rs` (`parse_pg_current_body`, `LibraryIndex::cell_attributes`, `CellAttributesIter`), `core/tests/core_api.rs`, `tests/test_structural_parse.py`, `tests/test_ccsn.py`.

- [x] Test a `dynamic_current → switching_group → pg_current → vector` fixture. Full parse must expose all samples. Structural parse must keep group count, conditions and PG pin identity and return an empty vector list. Check that a subsequent group still parses and that writer verification works on that document. Cover existing NLDM, CCS timing and CCSN behavior too.
- [x] In `parse_pg_current_body`, take the byte-skip path for `vector` when `skip_tables` is set. In the Rust indexed attributes API, add a cell-attribute projection that collects direct simple/complex attributes and sequential presence, including `ff_bank` and `latch_bank`. Avoid constructing and discarding complete pin, arc and waveform objects. Keep full structural parsing for callers asking for pins/arcs.
- [x] Preserve the existing attribute iterator's per-cell errors and ability to continue to the next cell. For subgroups whose syntax the structural parser validates today, share grammar traversal with a discard sink as needed to preserve errors. Compare projected output and errors to the baseline, including malformed children and unknown groups. Benchmark open and extraction separately. Run the Rust core API tests and the structural Python suites.

### Task 5 — Open the viewer index once and parse each tree once

**Files.** `viewer/server.py`, `viewer/__main__.py`, `viewer/data.py`, `src/lib.rs` and `liberty_tools/_native.pyi` for private shape metadata, new `tests/test_liberty_viewer.py`.

- [x] Test four concurrent calls to cold `get_data()` with an instrumented loader and synchronized starts. Expect one load and the same returned object. Test a failed load followed by a successful retry. Protect lazy initialization with a process-local lock and a second check inside it, publishing only a fully initialized object. Make readiness polling lightweight so repeated one-second probes cannot queue expensive metadata serializations. Preserve reload, browser opening, and exit-on-close behavior.
- [x] Count actual `doc.cell(name)` calls in `LibertyData.cell_tree(name)`. Pass already-resolved power groups through tree construction. Expose cheap private per-table metadata containing name, dimensionality and vector presence if needed. Use the same highest-populated-axis rule as `_ndim`, including a missing middle axis. Derive the ylabel from existing label/unit logic. Collapse only the same sibling groups as today. Avoid resolving table values during this step.
- [x] Compare old and new tree payloads, including paths, legends, container owners, mixed-dimension branches, CCS, conditional/default power and bus pins. Check `/api/table` still returns the selected data. Cache repeated `_fmt_bool` results with a bounded cache keyed by the raw expression, including the existing invalid-text fallback. Time both cells in `dev.lib`. Assert one native cell parse per tree and that the tree path never calls the full table payload method.
- [x] Reuse the requested table's flat values for reshaping and scalar access. For repeated cell-list requests, avoid cloning every native cell name on each page by retaining a per-index ordered name list in the viewer. Preserve substring filtering, pagination, total counts and first-seen index order. Measure a populated index with repeated filtered requests.

### Task 6 — Index document lookups and avoid cloning timing trees

**Files.** `src/lib.rs` (PyO3 document/cell/pin/arc/table handles and edit application), `def_tools/clock_roles.py`, `def_tools/instances.py`, `liberty_tools/_native.pyi`. Tests in `tests/test_extraction_api.py`, `tests/test_writer.py`, `tests/test_clock_data.py`, `tests/test_dump_instances.py` and `tests/test_sdc.py`.

- [x] Build a first-occurrence name-to-position map when converting the parsed document into a Python handle. Retain ordered storage for `cells()` and all-match traversal. Check duplicate-name behavior for both named and filtered multi-row APIs before sharing lookup machinery.
- [x] Give handles shared ownership of immutable cell data, using child indices/paths for each pin, bus, bundle, timing arc, power group and table. Keep editable scalar overlays and the shared edit log separate from immutable timing data. Indexed detached cells must remain readable after the index is dropped. Python list getters still return independently usable values.
- [x] Test fresh handles after edits, the handle that made an edit, already-issued sibling handles, read-only detached cells, and access after deleting the parent Python object. Pin mirrors, units and duplicate same-named pins in different containers must match the baseline. Scope edit replay to the addressed cell/pin and retain ordering of last writes. Benchmark `doc.cell(n).area`, held-handle reads, and pin/arc navigation with and without large CCS tables.
- [x] In `cell_clock_roles`, let a worker reduce each structural document to its small role mapping and release the document before returning. Merge in library input order, preserving the configured thread cap, first named group within each full document, and later-library overrides. For `sequential_macros` and instance dumps using only sequential flags, extract just `ff`/`latch` presence. Return both true and false classifications from workers so a later combinational cell can override an earlier sequential one. Keep the full role computation shared for requests needing clock-data resolution. Verify full/flag-only agreement on IHP, missing libraries, clock gates and reversed duplicate definitions.

### Task 7 — Export Polars frames column-wise

**Files.** `src/lib.rs` (filtered table traversal and single-table rows), `liberty_tools/__init__.py`, `liberty_tools/_native.pyi`, `tests/test_extraction_api.py`, `tests/test_smoke.py`. Use the existing Arrow dependency in the binding crate if needed.

- [x] Establish frame equality against the public list-of-dicts methods using `polars.testing.assert_frame_equal`. Cover timing, power, scalar and 1D/2D/3D tables, late non-null values, entirely null optional columns, no rows, duplicate cell names, bus/bundle members, every filter and invalid kinds/keywords. Test the table-level `to_polars()` separately. Current document no-match output is `(0, 0)`. In the `dev.lib` buffer, `when` and `index_3` are `Null`, `row/col/depth` are `Int64`.

  ```python
  filters = {"cell": "BUFx2_ASAP7_6t_R"}
  expected = pl.DataFrame(doc.timing_tables(**filters), infer_schema_length=None)
  assert_frame_equal(doc.to_polars(**filters), expected)
  assert doc.to_polars(cell="absent").shape == (0, 0)
  ```
- [x] Extract one ordered traversal over matching tables. Feed the existing row APIs and the new column builder from it. Preserve the exact key order, `related_pg_pin` versus `timing_type`, row-major `table_position`, original values and `when` implication semantics. Have the standalone table path borrow its axes/values instead of cloning a synthetic timing table.
- [x] Implement a private binding returning Arrow IPC bytes built in bounded record batches. Enable the existing vendored `arrow/ipc` feature on the binding crate if necessary. Decode with native Polars `read_ipc_stream` and an in-memory buffer. Keep heavy Rust work GIL-free. Preserve the legacy empty-frame shape and all-null dtypes using per-column presence information across the whole result. The IPC buffer itself scales with output size. Measure its coexistence with the final frame and keep transport copies explicit. The local environment has Polars and no PyArrow.
- [x] Measure filtered and whole-document time and subprocess peak RSS on both real and generated workloads. Inspect a 7×7 table by hand to catch swapped indices. Confirm list-returning methods, schema equality and offline builds. Document measured memory and the new conversion path.

### Task 8 — Verify formatter transparency incrementally

**Files.** `core/src/liberty.rs` (`lex_all`, `Lexer`), `src/lib.rs`, `liberty_tools/_native.pyi`, `liberty_format/__init__.py`, `liberty_tools/cli.py`, `tests/test_format.py`, `tests/test_cli.py`.

- [x] Add a private native comparison of two token streams that reports equality and both token counts. Compare token kind/value, including the distinction between a quoted word and a symbol with the same character. Continue validating both inputs after a mismatch to retain count and malformed-input diagnostics and their existing precedence. Keep the explicit `tokenize` and `tokenize_str` APIs available.
- [x] Use that comparison in `format_text`. Test the same text on both sides, an early and late mismatch, missing/extra final tokens, malformed tails, multiline strings, postfix Boolean syntax, CRLF and idempotence. Verify exact `TransparencyError` behavior on a forced mismatch. Measure token verification by itself and end-to-end.
- [x] Address the remaining whole-file cost in the CLI. Extract a line-oriented reindent routine carrying quote/comment/blank-line state. Validate against existing `reindent(text)` byte output, including trailing newline and Unicode whitespace rules. Stage output in a temporary file for large plain/gzip input, compare normalized input and staged output with streaming lexers, then honor stdout, `-o`, `--check` or `--in-place`. Preserve current encoding/newline and gzip-output conventions, permissions and symlink behavior for supported destinations. Failed verification must leave the destination untouched and stdout empty. Keep the string API usable without staging files. Re-profile Python line scanning after removing token-list allocation before considering a native port.

### Task 9 — Match timing grids and conditional arcs once

**Files.** `liberty_tools/comparison.py` (`_compare_table`, `_grid_points`, `_points_only_in`, `_match_when_conditions`), `tests/test_compare.py`, `tests/test_table_lookup.py`.

- [x] After unit, shape, malformed-table and axis-role checks, recognize identical finite increasing axes in aligned order. Take the direct path only if the tolerance is suitable and neighboring coordinates cannot match each other under the exact existing tolerance formula. Enumerate row-major indices once. For ambiguous, descending, duplicate, nonfinite or unequal axes retain first-in-file tolerance matching in both directions. Yield Cartesian products incrementally on the general path. Keep interpolation on its current branch.
- [x] Within each arc group, cache each raw condition's canonical key once. Use separate tagged keys for canonical expressions and raw fallbacks (including missing conditions). Bucket B candidates in encounter order and pop the first available match. Emit logical matches followed by raw matches to preserve existing ordering. Keep missing arcs and duplicate diagnostics ordered.
- [x] Differentially compare complete `Comparison` objects, including entry ordering, on equal/permuted 3D grids, nonfinite values, near-equal neighbors, `interpolate=True`, CCS vectors, reordered minterms, equivalent text spellings and invalid conditions. Confirm duplicate-cell detection still covers the entire document on targeted comparisons. Benchmark `dev.lib`, 512/1,024 samples and 64/128 reversed minterms with parsing outside the timer.

### Task 10 — Reduce Boolean and comparison reporting overhead

**Files.** `core/src/liberty.rs` (`WhenFilter`, `bool_implies`), `src/lib.rs` (`BooleanExpression` and filter callers), `liberty_tools/comparison.py` (`Comparison`), `tests/test_boolean_expression.py`, `tests/test_compare.py`.

- [x] Keep syntax and variable-cap checks at expression construction. Compute canonical/support/minterms together on first use and minimized display text on first request for it. Cache in the immutable native expression. Equality and hash still agree by canonical representation. Verify all getters, constants, strings, caps and errors in different access orders.
- [x] Memoize actual-condition implication results for the lifetime of a given query. Use variable positions and bit assignments in the enumerator to eliminate per-assignment sets and string clones. Preserve false on unparsable actual conditions, query errors and the current supported-variable behavior. Compare with the original truth table for combinations of AND/OR/XOR, constants, disjoint support and postfix negation. Benchmark 1 and 16 repeats of a 12-input condition. This retains exponential worst-case enumeration.
- [x] Compute `summary()` counts, max absolute delta and difference flag in one pass over entries plus the structural diagnostics. Short-circuit `has_differences` and stream `max_abs_delta`. Keep list-returning properties and reflect mutations of the public result lists on the next call. Build `Comparison.to_polars()` from columns under its existing explicit schema. Test empty, equal, mismatched, unmatched and diagnostics-only results, plus a result mutated after its first summary.

### Task 11 — Render bulk edits in source order

**Files.** `core/src/liberty.rs` (`apply_edits_to_source`, `patch_cell_text`), `src/lib.rs` (writer shims), `tests/test_writer.py`.

- [x] Keep the last edit per cell/pin/container/key in the existing winning-edit order. Walk recorded cell spans in source order, consume each edited name on its first occurrence, copy each unchanged interval once, append its patched group and finally the tail. Report any unresolved edit target. Retain the cheap no-edit case.
- [x] Preserve `patch_cell_text`'s ordering for multiple insertions at the same offset, overlap detection and ambiguous pin errors. Profile many edits within one cell separately. If repeated inner replacements dominate, apply the validated inner patches in a forward pass with explicit equal-offset ordering. Keep source hash/length checks before emitting output, including same-path writes and gzip.
- [x] Compare rendered bytes against expected text for longer/shorter replacements, comments, CRLF, duplicate cells, same-named pins under different owners, repeated setters, insertions, unreachable targets and changed source. Time edit setup, rendering, and final file I/O separately at 500–4,000 edits. Check peak RSS as the forward pass holds input and output together.

### Task 12 — Bound browser work and load charts on demand

**Files.** `viewer/static/app.js`, `viewer/static/index.html`, `viewer/data.py`, new `tests/test_liberty_viewer_frontend.py` using Node as in `tests/test_symbol_render.py`.

- [x] Replace the per-annotation scan of all template blocks with a single ordered pass maintaining the active template stack. Honor strings, escapes, comments and nested groups. Preserve innermost-template selection, line text, source paths and the collapse/highlight order. Compare rendered annotations on sibling and nested CCS vectors, truncated source and braces inside strings. Time 2k, 4k and 8k siblings.
- [x] Retain source for the active cell and bound recently visited sources by total size. Tie tree lifetimes to visible/selected cells, with debug state reflecting the currently displayed tree. Preserve symbol re-selection, rapid filtering and navigation. Guard against an older asynchronous response overwriting a newer selection. Test eviction and repeated visits, including the one-million-character cap and accurate reported original length.
- [x] Remove the synchronous Plotly script from the shell. Load the existing local asset through one shared promise when a view first requests a plot. Cover library driver waveforms, multi-table, heatmap/surface, CCS/CCSN and click-to-show-wave entry points. Allow retry after load failure and ignore obsolete renders after navigation. Verify in a browser that metadata, tree and scalar views work before a plot request and the first chart works on a cold load. Keep the standalone `logic2svg` renderer and its sample outputs as regression references. Its layout showed no material bottleneck in the review.

### Task 13 — Reduce combo-builder work for repeated libraries

**Files.** `scripts/build_combo_lib.py` (`stream_body`), new `tests/test_build_combo_lib.py`.

- [x] Run the rename regex only when a suffix is requested and the existing brace depth is zero. Keep streaming, source order, gzip level, blank lines, name-suffix numbering, and the documented ASAP7 stray-close handling.
- [x] Compare decompressed output byte-for-byte against a golden for two input files and multiple copies. Exercise a long nested table, optional leading whitespace and a stray library close. Parse the combined result and check the complete cell-name sequence. Benchmark the 16-pass gzip case with compression level fixed. Compressed byte equality is sensitive to gzip header timestamps.

### Task 14 — Verify the integrated release build

- [x] Rebuild and test from the finished revision. Use the checkout's interpreter for pytest immediately after the native build. These commands start at the repository root.

  ```bash
  cargo fmt --all --check
  cargo test --workspace --locked --offline
  uv run maturin develop --release
  .venv/bin/python -c 'from liberty_tools import _native; print(_native.__file__)'
  .venv/bin/python -m pytest -q
  uv run --no-sync ruff check .
  uv run --no-sync ty check
  node --check viewer/static/app.js
  node --check viewer/static/symbol.js
  ```

- [x] Rerun the full benchmark matrix in release mode and record baseline/candidate timings, RSS, object/read counts and output checks per workload. Recheck standalone core consumers and both formatter entry points. Exercise a browser session from cold open through tree, source, symbols and each chart family. Report skipped optional fixtures by name.
- [x] Update `docs/liberty.md`, `docs/ai-api.md`, relevant examples in `README.md` and architectural notes in `CLAUDE.md` where behavior or measurements changed. Update `docs/cli.md` and stubs for any affected interface. Replace stale benchmark claims with workload, build profile and measured result. Record the verified revision and outstanding items in the handoff.
