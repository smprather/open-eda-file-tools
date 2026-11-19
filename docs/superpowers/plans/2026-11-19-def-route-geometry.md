# DEF Route Geometry for `net-shapes` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `net-shapes` reads every routed net an OpenROAD or Innovus DEF writes, instead of failing on the first `*` coordinate, and never turns non-route values into shapes.

**Architecture:** All route geometry comes from one function, `parse_net_shapes_record` in `src/def.rs`, used by the direct scan, the Python iterator and the cache query. It becomes a small state machine over route tokens: points with `*` and extension values, `RECT` patches, `VIRTUAL` points, `MASK`, vias, `TAPER`/`STYLE`, and the net-level `+` keywords. Pieces whose layer cannot be known without technology data are counted and reported, never attributed to a guessed layer.

**Tech Stack:** Rust (PyO3 0.28, rayon), Python 3 click CLI, pytest.

**Spec:** No spec document. Requirements come from the 2026-11-19 findings below and the LEF/DEF 5.8 Language Reference, sections "Regular Wiring Statement" (NETS) and "Special Wiring Statement" (SPECIALNETS). The reference is not in the repo; Task 1 Step 1 checks the rules below against it.

## Background (2026-11-19)

- On a routed OpenROAD DEF (`~/openlane2-cva6/runs/cva6/final/def/cva6_top.def`, 118 MB, IHP sg13g2), `def-tools net-shapes` on any signal net fails with `invalid route point x: '*' near DEF byte 58645379`. Power nets work.
- Constructs counted in that file: `*` coordinates on nearly every route, 164,484 `RECT ( dx1 dy1 dx2 dy2 )` patches, 522 three-value points `( x y 0 )`, 74 `NEW layer TAPER` lines. There are no `VIRTUAL`, `MASK` or mid-path vias (`( x y ) via12 ( x y2 )`). Innovus writes mid-path vias, which today silently put the next segment on the pre-via layer.
- Today, net-level `+` keywords other than the route statuses and `SHAPE` are ignored, so the parenthesized values of `+ VPIN ... ( x y ) ( x y )` or of a special `+ RECT layer pt pt` are read as route points on the previous layer.

## Global Constraints

- Build with `uv run maturin develop --release` after any Rust change, then test with `uv run --no-sync pytest -q`. Plain `uv run` can reinstall a stale cached wheel.
- Gates before every commit: `cargo test`, `cargo fmt --all --check`, `cargo clippy --release` (warning count must not rise above 26), `uv run --no-sync ruff check .`, `uv run --no-sync ty check`.
- Documentation synchronization mandate (`CLAUDE.md`): any change to a CLI flag, output, Python function or behavior updates `docs/ai-api.md`, `docs/cli.md`, `docs/def.md`, `liberty_tools/_native.pyi` and `CLAUDE.md` in the same commit.
- No design files are committed. Tests use inline DEF text written to `tmp_path`.
- Coordinates in `net-shapes` output stay in DEF database units.
- Parallel work stays inside `crate::threads::install`; output must not depend on `--threads`.
- Indent with spaces only.
- DEFs without the constructs this plan adds must produce byte-identical output: `scripts/def_equivalence.py` against the commit before Task 1 reports `differences: 0` on a generated design.

## Review Focus

- A `*` in the first point of a path (no previous point): error naming the DEF byte, not a panic or a silent 0. (Task 1 test `test_star_without_previous_point_is_an_error`.)
- `+ VPIN name LAYER l ( x y ) ( x y ) PLACED ( x y ) N` after a route: its points never become shapes. (Task 3 test `test_net_level_keywords_end_the_path`.)
- An Innovus-style path that continues after a via: the following segments are not reported on the pre-via layer, and are counted. (Task 2 test `test_segments_after_a_via_are_skipped_and_counted`.)
- The cache query (`--cache-dir`) returns the same rows as the direct scan for the new constructs. (Task 4 test `test_cache_and_direct_queries_agree_on_new_constructs`.)
- Negative `RECT` deltas and reversed segment direction give normalized bboxes (`llx <= urx`, `lly <= ury`). (Task 2 test `test_rect_patch_rows`.)

## Decisions this plan makes (veto before Task 1 if you disagree)

1. `*` repeats the previous point's coordinate within the same path; a path starts at `ROUTED`/`FIXED`/`COVER`/`NOSHIELD`/`SHIELD`/`NEW`.
2. An extension value `( x y ext )` extends the segment past that point by `ext` along the segment axis, for every segment that ends at that point. `*` as an extension repeats the previous point's extension. Diagonal segments ignore extensions. Existing width handling is unchanged: regular-net widths stay 0, since the width comes from technology LEF, which this path does not read.
3. A `RECT ( dx1 dy1 dx2 dy2 )` patch becomes one row with `shape_type = "RECT"` and `width = 0`. `x1,y1` / `x2,y2` hold its lower-left / upper-right corners, which are also its bbox. The path's previous point is unchanged.
4. A special-net `+ RECT layer ( x1 y1 ) ( x2 y2 )` becomes the same kind of row, with `route_status = ""`.
5. A via (any other bare word after a point) is not reported, as today. After it, the layer is unknown until the next `NEW` or status keyword. Segments in that stretch are skipped and counted as `after_via`.
6. `+ POLYGON` is skipped and counted as `polygon`, but only when its layer is the queried one. `+ SUBNET` wiring is skipped and counted as `subnet` (one per subnet). `+ VIA` is skipped and not counted. `after_via` and `subnet` count regardless of the queried layer, since the skipped pieces' layer is unknown. Counts cover only records of the queried net.
7. `write_net_shapes_csv` and `write_net_shapes_csv_from_cache` return `dict[str, int]` with keys `after_via`, `polygon` and `subnet` (today they return `None`). The CLI prints one warning line when any count is non-zero.

---

### Task 1: Route points (`*`, extension values, `VIRTUAL`, `MASK`)

**Files:**
- Modify: `src/def.rs` — `parse_net_shapes_record` (around line 1962), `make_net_shape` (around line 2170)
- Test: `tests/test_net_shapes_routes.py` (create)

**Interfaces:**
- Produces (private to `src/def.rs`, used by Tasks 2–3):
  - `struct RoutePoint { x: i64, y: i64, ext: Option<i64> }`
  - `fn read_route_point(tokens: &mut ByteTokens<'_>, previous: Option<RoutePoint>, offset: u64) -> Result<RoutePoint, String>`: call after the `(` token; reads `x|*`, `y|*`, an optional `ext|*`, then `)`.
  - `struct RouteContext<'a> { net: &'a str, section: &'static str, status: &'static str, shape_type: &'a str, layer: &'a str, width: i64 }`
  - `fn make_net_shape(context: &RouteContext<'_>, start: RoutePoint, end: RoutePoint) -> NetShape`, replacing the 10-argument version (one clippy warning fewer).

- [ ] **Step 1: Check the grammar.** Read the two wiring sections of the LEF/DEF 5.8 reference. Confirm: `*` semantics; extension value range and meaning; `VIRTUAL ( x y )` draws no wire; `MASK maskNum` precedes a point, via or `RECT`; `RECT` deltas are relative to the previous point; via arrays `DO numX BY numY STEP stepX stepY` (special wiring); `+ SYNTHESIZED` inside a connection. Wherever the reference disagrees with "Decisions this plan makes", the reference wins: change the decision and the tests below before writing code.

- [ ] **Step 2: Write the failing tests** in `tests/test_net_shapes_routes.py` (imports: `pytest`, `def_tools`, `from click.testing import CliRunner`, `from def_tools.cli import main`, `from liberty_tools import _native`). Add a helper `write_def(tmp_path, nets: str, specialnets: str = "") -> Path` that wraps the given NETS/SPECIALNETS bodies in a minimal DEF (VERSION, DESIGN, `UNITS DISTANCE MICRONS 1000`, `COMPONENTS 2` with `u1`/`u2`, the sections, `END DESIGN`). Section counts are not checked by the reader, so `NETS 1 ;` / `SPECIALNETS 1 ;` will do; omit an empty section. Add `rows(design, net, layer) -> list[tuple]` that returns `(shape_type, width, x1, y1, x2, y2, llx, lly, urx, ury)` for each shape from `def_tools.net_shapes`.

```python
ROUTE = """
  - a ( u1 Y ) ( u2 A )
    + ROUTED met2 ( 100 200 ) ( 300 * ) ( * 500 )
    NEW met3 ( 300 500 0 ) ( 300 900 40 )
    NEW met2 ( 0 0 ) VIRTUAL ( 50 * ) ( 50 70 )
    NEW met2 ( 10 10 ) MASK 2 ( 20 * ) ;
"""

def test_star_repeats_previous_coordinate(tmp_path):
    design = write_def(tmp_path, ROUTE)
    assert rows(design, "a", "met2") == [
        ("", 0, 100, 200, 300, 200, 100, 200, 300, 200),
        ("", 0, 300, 200, 300, 500, 300, 200, 300, 500),
        ("", 0, 50, 0, 50, 70, 50, 0, 50, 70),
        ("", 0, 10, 10, 20, 10, 10, 10, 20, 10),
    ]

def test_extension_value_extends_segment_end(tmp_path):
    design = write_def(tmp_path, ROUTE)
    assert rows(design, "a", "met3") == [("", 0, 300, 500, 300, 900, 300, 500, 300, 940)]

def test_star_without_previous_point_is_an_error(tmp_path):
    design = write_def(tmp_path, "  - b ( u1 Y ) + ROUTED met2 ( * 5 ) ( 10 5 ) ;\n")
    with pytest.raises(RuntimeError, match=r"'\*' with no previous point near DEF byte \d+"):
        list(def_tools.net_shapes(design, "b", "met2"))

def test_openroad_route_from_the_failing_design(tmp_path):
    # Shape of the record that failed on cva6 (net _000000_).
    design = write_def(tmp_path, """
  - n0 ( u1 D ) ( u2 X ) + USE SIGNAL
      + ROUTED Metal3 ( 824640 895020 ) ( 830400 * )
      NEW Metal2 ( 824640 895020 ) ( * 897960 )
      NEW Metal1 ( 830400 895020 ) Via1_XY ;
""")
    assert rows(design, "n0", "Metal2") == [("", 0, 824640, 895020, 824640, 897960, 824640, 895020, 824640, 897960)]
    assert rows(design, "n0", "Metal3") == [("", 0, 824640, 895020, 830400, 895020, 824640, 895020, 830400, 895020)]
```

- [ ] **Step 3: Run the tests and see them fail.** Run: `uv run --no-sync pytest tests/test_net_shapes_routes.py -q`. Expected: FAIL, with `invalid route point x: '*'` for the star tests and a row mismatch for the extension test.

- [ ] **Step 4: Implement** `read_route_point`, `RouteContext` and the new `make_net_shape` in `src/def.rs`, and use them in `parse_net_shapes_record`. `VIRTUAL` reads the next point with `read_route_point` (it must be followed by `(`) and makes it the previous point without a shape. `MASK` consumes one number. The `*`-with-no-previous error text is `"'*' with no previous point near DEF byte {offset}"`.

- [ ] **Step 5: Run the tests and see them pass.** Run `uv run maturin develop --release && uv run --no-sync pytest tests/test_net_shapes_routes.py tests/test_instances.py -q`. Expected: all pass (`tests/test_instances.py` holds the existing net-shape expectations).

- [ ] **Step 6: Commit.**

```bash
git add src/def.rs tests/test_net_shapes_routes.py
git commit -m "net-shapes: read '*' coordinates, extension values, VIRTUAL and MASK"
```

### Task 2: `RECT` patches, vias, `TAPER`/`TAPERRULE`/`STYLE`

**Files:**
- Modify: `src/def.rs` — `parse_net_shapes_record`, `process_net_record_batch` (around line 1845)
- Test: `tests/test_net_shapes_routes.py`

**Interfaces:**
- Consumes: `RoutePoint`, `read_route_point`, `RouteContext`, `make_net_shape` (Task 1).
- Produces:
  - `#[derive(Default, Clone, Copy, PartialEq, Eq, Debug)] struct SkippedRoutes { after_via: u64, polygon: u64, subnet: u64 }`, with `fn add(&mut self, other: SkippedRoutes)`.
  - `fn parse_net_shapes_record(record: &NetRecord, target_net: &[u8], target_layer: &[u8]) -> Result<(Vec<NetShape>, SkippedRoutes), String>`
  - `fn process_net_record_batch(batch: &[NetRecord], net: &[u8], layer: &[u8]) -> Result<(Vec<NetShape>, SkippedRoutes), String>`. Callers that do not report yet ignore the second value until Task 4.

- [ ] **Step 1: Write the failing tests.** Assert on rows only; the counts become visible in Task 4.

```python
VIAS = """
  - b ( u1 Y ) ( u2 A )
    + ROUTED met2 TAPER ( 0 0 ) ( 100 * ) via12 ( 100 50 ) ( 200 * )
    NEW met1 TAPERRULE rule1 ( 0 0 ) RECT ( 30 40 -10 -20 )
    NEW met1 STYLE 2 ( 5 5 ) ( 5 15 ) via23 N ;
"""

def test_segments_after_a_via_are_skipped_and_counted(tmp_path):
    design = write_def(tmp_path, VIAS)
    assert rows(design, "b", "met2") == [("", 0, 0, 0, 100, 0, 0, 0, 100, 0)]

def test_rect_patch_rows(tmp_path):
    design = write_def(tmp_path, VIAS)
    assert rows(design, "b", "met1") == [
        ("RECT", 0, -10, -20, 30, 40, -10, -20, 30, 40),
        ("", 0, 5, 5, 5, 15, 5, 5, 5, 15),
    ]

def test_rect_without_previous_point_is_an_error(tmp_path):
    design = write_def(tmp_path, "  - c ( u1 Y ) + ROUTED met1 RECT ( 0 0 1 1 ) ;\n")
    with pytest.raises(RuntimeError, match="RECT with no previous point"):
        list(def_tools.net_shapes(design, "c", "met1"))
```

  `test_segments_after_a_via_are_skipped_and_counted` gains its count assertion (`after_via == 2`) in Task 4.

- [ ] **Step 2: Run the tests and see them fail.** Same command as Task 1 Step 3. Expected: FAIL. Today the `RECT` deltas are read as a malformed point, and the post-via segments come out as `met2` rows.

- [ ] **Step 3: Implement.** In the path state, keep `layer: Option<&[u8]>` (`None` = unknown after a via) next to `previous: Option<RoutePoint>`. Token rules once a path has started:
  - `RECT` reads four integers in parentheses. With no previous point it is an error: `"RECT with no previous point near DEF byte {offset}"`.
  - `TAPER` is skipped. `TAPERRULE` and `STYLE` skip one token. These are only legal right after the layer name; accept them anywhere in a path rather than rejecting.
  - Any other bare word after a point is a via. Consume an orientation token if the next token is one of `N S E W FN FS FE FW`. Consume `DO n BY n STEP n n` if the next token is `DO`. Then set the layer to unknown.
  - A segment whose layer is unknown increments `after_via` instead of producing a row.

- [ ] **Step 4: Run the tests and see them pass.** Run `uv run maturin develop --release && uv run --no-sync pytest tests/test_net_shapes_routes.py tests/test_instances.py -q`. Expected: all pass.

- [ ] **Step 5: Commit.** `git commit -m "net-shapes: RECT patches, via layer changes, TAPER and STYLE"`

### Task 3: Net-level `+` keywords

**Files:**
- Modify: `src/def.rs` — `parse_net_shapes_record`
- Test: `tests/test_net_shapes_routes.py`

**Interfaces:**
- Consumes: Task 2's `SkippedRoutes` and path state.

- [ ] **Step 1: Write the failing tests.**

```python
SPECIAL = """
  - VDD ( * VDD )
    + ROUTED met1 100 + SHAPE STRIPE + STYLE 1 ( 0 0 ) ( 1000 0 ) via12_a DO 2 BY 1 STEP 50 0 ( 1000 300 )
    NEW met2 200 ( 0 0 ) ( 0 500 )
    + RECT met3 ( 20 30 ) ( 10 10 )
    + POLYGON met3 ( 0 0 ) ( 10 0 ) ( 10 10 )
    + VIA via23_b ( 5 5 ) ( 6 6 )
    + USE POWER ;
"""

def test_special_net_keywords(tmp_path):
    design = write_def(tmp_path, "", specialnets=SPECIAL)
    assert rows(design, "VDD", "met1") == [("STRIPE", 100, 0, 0, 1000, 0, 0, -50, 1000, 50)]
    assert rows(design, "VDD", "met2") == [("", 200, 0, 0, 0, 500, -100, 0, 100, 500)]
    assert rows(design, "VDD", "met3") == [("RECT", 0, 10, 10, 20, 30, 10, 10, 20, 30)]

def test_net_level_keywords_end_the_path(tmp_path):
    design = write_def(tmp_path, """
  - c ( u1 Y ) ( u2 A )
    + ROUTED met1 ( 0 0 ) ( 10 0 )
    + VPIN vp LAYER met1 ( 0 0 ) ( 9 9 ) PLACED ( 1 1 ) N
    + SUBNET s1 ( u1 A ) ROUTED met1 ( 0 0 ) ( 0 99 ) ;
""")
    assert rows(design, "c", "met1") == [("", 0, 0, 0, 10, 0, 0, 0, 10, 0)]
```

  The `+ RECT` row also asserts `route_status == ""`: check it with `def_tools.net_shapes(...)[0].route_status`.

- [ ] **Step 2: Run the tests and see them fail.** Expected: FAIL. The VPIN and SUBNET points produce extra `met1` rows, and `+ RECT` is read as a path on `met2`.

- [ ] **Step 3: Implement** the `+` keyword table:
  - Path-starting statuses (`ROUTED FIXED COVER NOSHIELD`, and `SHIELD net`) work as today.
  - `SHAPE type`, `STYLE n` and `MASK n` continue the current path.
  - `RECT layer pt pt` emits a Decision-4 row on that layer (when it matches) and ends the path.
  - `POLYGON layer pts…` consumes its points, counts `polygon`, and ends the path.
  - `VIA name [orient] pts…` consumes its points and ends the path.
  - `SUBNET` counts `subnet` and skips every token up to the next net-level `+`.
  - Every other keyword ends the path (status and layer become `None`), so its parenthesized values are skipped.

- [ ] **Step 4: Run the tests and see them pass**, including `tests/test_instances.py`.

- [ ] **Step 5: Commit.** `git commit -m "net-shapes: net-level + keywords never become route points"`

### Task 4: Report skipped route pieces

**Files:**
- Modify: `src/def.rs` — `NetShapeIter` (around line 334), `write_net_shapes_csv` (around line 507), `write_net_shapes_csv_from_cache` (around line 525), `write_net_shapes_csv_stream`, `write_net_shapes_records_csv`
- Modify: `def_tools/cli.py` — `net_shapes` command
- Modify: `liberty_tools/_native.pyi`, `docs/ai-api.md`, `docs/cli.md`, `docs/def.md`, `CLAUDE.md`
- Test: `tests/test_net_shapes_routes.py`

**Interfaces:**
- Consumes: `process_net_record_batch -> (Vec<NetShape>, SkippedRoutes)` (Task 2).
- Produces (Python):
  - `write_net_shapes_csv(def_path, net, layer, output=None) -> dict[str, int]`
  - `write_net_shapes_csv_from_cache(cache_dir, net, layer, output=None) -> dict[str, int]`
  - `NetShapeIter.skipped -> dict[str, int]`: a getter, counts so far.
  - All three use the keys `"after_via"`, `"polygon"` and `"subnet"`.

- [ ] **Step 1: Write the failing tests.**

```python
def test_writers_return_skip_counts(tmp_path):
    design = write_def(tmp_path, VIAS)
    counts = _native.write_net_shapes_csv(design, "b", "met2", tmp_path / "s.csv")
    assert counts == {"after_via": 2, "polygon": 0, "subnet": 0}

def test_iterator_exposes_skip_counts(tmp_path):
    design = write_def(tmp_path, "", specialnets=SPECIAL)
    shapes = def_tools.net_shapes(design, "VDD", "met1")
    list(shapes)
    assert shapes.skipped == {"after_via": 1, "polygon": 0, "subnet": 0}  # the polygon is on met3
    shapes = def_tools.net_shapes(design, "VDD", "met3")
    list(shapes)
    assert shapes.skipped == {"after_via": 1, "polygon": 1, "subnet": 0}

def test_cli_warns_about_skipped_pieces(tmp_path):
    design = write_def(tmp_path, VIAS)
    result = CliRunner().invoke(main, ["net-shapes", "--def", str(design), "--net", "b", "--layer", "met2"])
    assert result.exit_code == 0
    assert "warning: net-shapes skipped route pieces: after_via=2 polygon=0 subnet=0" in result.stderr

def test_cache_and_direct_queries_agree_on_new_constructs(tmp_path):
    design = write_def(tmp_path, ROUTE + VIAS, specialnets=SPECIAL)
    _native.write_def_cache(design, tmp_path / "cache")
    for net, layer in (("a", "met2"), ("b", "met1"), ("VDD", "met3")):
        direct, cached = tmp_path / "d.csv", tmp_path / "c.csv"
        assert _native.write_net_shapes_csv(design, net, layer, direct) == \
            _native.write_net_shapes_csv_from_cache(tmp_path / "cache", net, layer, cached)
        assert direct.read_bytes() == cached.read_bytes()
```

- [ ] **Step 2: Run the tests and see them fail.** Expected: FAIL. The writers return `None`, and the iterator has no `skipped`.

- [ ] **Step 3: Implement.** Sum `SkippedRoutes` across batches and return it as a `dict` built outside `py.detach`. Add the `skipped` getter to `NetShapeIter`. The CLI prints the warning line to stderr only when a count is non-zero. Update the `.pyi` signatures and the docs: the `net-shapes` output columns, `RECT` rows, the skip counts and their causes, and the regular-net width limitation (Decision 2).

- [ ] **Step 4: Run the full gates.** Run `uv run maturin develop --release`, `uv run --no-sync pytest -q`, `cargo test`, `cargo fmt --all --check`, `cargo clippy --release 2>&1 | grep -c '^warning'` (expect ≤ 26), `uv run --no-sync ruff check .`, `uv run --no-sync ty check`. Expected: all pass.

- [ ] **Step 5: Commit.** `git commit -m "net-shapes: report route pieces it cannot place"`

### Task 5: Acceptance on real and generated designs

**Files:** none changed unless a check fails.

- [ ] **Step 1: Equivalence on a design without the new constructs.** Build the reference from the commit before Task 1 (see `scripts/def_equivalence.py` docstring), generate `uv run --no-sync python -m tests.gen.design_gen -o /tmp/gen -n 200000 --no-gzip`, then run `uv run --no-sync python scripts/def_equivalence.py /tmp/ref-target/release/lib_native.so /tmp/gen`. Expected: every `net_shapes` line `ok`. The script also compares instance and cache outputs; any `DIFF` there is a regression. Note: the writers' result now differs on purpose (a dict instead of `None`), so expect exactly the `net_shapes_* result` lines to show `DIFF` and their `bytes` lines `ok`; list that in the commit message.

- [ ] **Step 2: The routed design.** If `~/openlane2-cva6/runs/cva6/final/def/cva6_top.def` exists, run `uv run --no-sync def-tools net-shapes --def <that file> --net _050000_ --layer Metal2`. Expected: exit 0 and at least one row. Then run `--net _000000_ --layer Metal3` and check the output contains `_000000_,NETS,ROUTED,,Metal3,0,824640,895020,830400,895020`.

- [ ] **Step 3: Speed.** `uv run --no-sync python scripts/def_bench.py <cva6 dir>`. Expected: `net-shapes signal` has no error and runs within 20% of `net-shapes power` (0.02 s on 2026-11-19).
