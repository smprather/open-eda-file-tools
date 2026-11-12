# open-eda-file-tools

`open-eda-file-tools` is a home for open EDA data-file tooling. Its first
package is `liberty-tools`, a fast Python library for parsing and querying
Synopsys Liberty `.lib` timing models: it streams Liberty files through a Rust
backend and exposes a Python API for cells, pins, timing arcs, timing tables,
and Polars dataframe extraction.

More EDA data-file tools join it in this repository: `lef-tools` parses cell
LEF libraries and `def-tools` parses and queries DEF designs, both Rust-backed.
A VCD toggle-count profiler and others are still planned.

The rendered documentation is at
<https://smprather.github.io/open-eda-file-tools/>.

The Liberty implementation focuses on standard-cell timing, power, and CCS data:

- streaming parse from `.lib` and `.lib.gz` paths
- low-memory Rust-backed indexing with on-demand cell parsing (`LibraryIndex`)
- cell, pin, and timing arc lookup
- bus, bundle, and Liberty `type(...)` bus definition lookup
- 1D, 2D, and 3D timing table extraction to Python rows or Polars
- lookup of a table value by axis coordinate, by axis role or by grid index
- `internal_power` extraction (switching energy in joules)
- CCS data: `output_current_*` waves, CCSP dynamic current, and CCS-noise
  (CCSN) stage data (`dc_current`, `output_voltage_*`)
- Boolean-aware `when` filtering
- uniform attribute access (`get`) and unit-aware scalar accessors
- writing a parsed document back out, preserving everything you did not change
- comparing two libraries with arc matching and grid alignment
- the GIL is released during parsing, so many libraries can be opened in
  parallel threads
- browser viewer for navigating cells, source text, timing/power tables,
  CCS/CCSN waves, and combinational logic symbols

For LLM/agent-oriented API usage, see the
[AI API guide](https://smprather.github.io/open-eda-file-tools/user-guide/ai-api.html).
For the command line tools, see the
[CLI reference](https://smprather.github.io/open-eda-file-tools/user-guide/cli.html).
The package also ships `py.typed` and `.pyi` stubs for coding tools.

## LEF Tools

The `lef_tools` library and `lef-tools` CLI parse cell LEF libraries into typed
macros, pins, ports and rectangles. All geometry stays in LEF microns.

```bash
uv run lef-tools macros --lef sky130_fd_sc_hd.lef
```

```python
from lef_tools import parse_lef

library = parse_lef(["cells.lef"], macros=["sky130_fd_sc_hd__buf_1"])
macro = library.macro("sky130_fd_sc_hd__buf_1")
print(macro.width_microns, macro.height_microns)

for pin in macro.pins():
    print(pin.name, pin.direction, pin.use)
```

## DEF Tools

The `def_tools` library and `def-tools` CLI parse and query DEF designs:

| Command | What it does |
|---|---|
| `instances` | Dump placed instance bounding boxes (in microns) as CSV, Parquet, or an aligned table; adds `is_seq` when `--lib` is supplied and `is_clock` when `--clock-cells` is supplied. |
| `net-shapes` | Emit routed shape bboxes for one net and layer, from a DEF or a cache. |
| `cache create` | Convert a DEF into an indexed binary cache directory. |
| `clock-instances` | Walk DEF connectivity from SDC clock roots. |
| `configure-io` | Benchmark storage targets and write an IO policy config. |

```bash
uv run def-tools instances --lef design.lef --def design.def.gz --output instances.csv
uv run def-tools instances --format parquet --lef "libs/*.lef" --def design.def.gz --output instances.parquet
```

```python
from def_tools import instance_bboxes

for bbox in instance_bboxes(["sky130.lef"], "design.def.gz"):
    print(bbox.instance, bbox.llx, bbox.lly, bbox.urx, bbox.ury)
```

DEF coordinates stay in database units; plain and gzipped inputs are both
read. The [LEF guide](https://smprather.github.io/open-eda-file-tools/user-guide/lef.html)
and [DEF guide](https://smprather.github.io/open-eda-file-tools/user-guide/def.html)
cover the APIs, CLIs, binary DEF cache and storage configurator.

## SDC Tools

The `sdc_tools` library and `sdc-tools` CLI return the clocks an SDC creates.
The file runs in a real Tcl interpreter (>= 8.6) with stubbed SDC commands, so
variables, loops and flow-generated Tcl work as authored:

```bash
uv run sdc-tools clocks --sdc design.sdc
uv run sdc-tools clock-cells --sdc design.sdc --def design.def.gz --lib cells.lib
```

```text
clock	period	waveform	sources	generated_from	divide_by	multiply_by
clk	10	0 5	port:clk
```

`clock-cells` traces forward from each clock root through DEF connectivity and
prints the clock-tree instances, which feed `def-tools instances
--clock-cells` to produce the `is_clock` column.

## Project Status

The parser is already useful for large timing libraries. The whole project
has one version (see
[VERSIONING_PLAN.md](https://github.com/smprather/open-eda-file-tools/blob/main/VERSIONING_PLAN.md)):
CalVer `YYYY.M.D`, with every tool released together from one tag.

On the local 96.8 MB reference Liberty file, the release extension parses the
whole library into an in-memory document in about 1.5 seconds (~150 MB peak
RSS). The lazy `LibraryIndex` used by the viewer opens the same file in about
0.15 seconds and parses individual cells on demand.

## Repository Layout

The repository is a single project (`open-eda-file-tools`) with a single
`pyproject.toml`, `Cargo.toml`, and version. The Python libraries live in
`liberty_tools/`, `lef_tools/`, `def_tools/`, `viewer/`, and `liberty_format/`;
the shared Rust backend lives in `src/` (`src/lib.rs`, `src/lef/`, `src/def.rs`),
and the unified test suite lives in `tests/`. The
[Tools guide](https://smprather.github.io/open-eda-file-tools/user-guide/tools.html)
lists the tools and the steps for adding one. Shared build, test and release
conventions are in
[BUILD.md](https://github.com/smprather/open-eda-file-tools/blob/main/BUILD.md),
[VERSIONING_PLAN.md](https://github.com/smprather/open-eda-file-tools/blob/main/VERSIONING_PLAN.md)
and
[CLAUDE.md](https://github.com/smprather/open-eda-file-tools/blob/main/CLAUDE.md).

## Install From GitHub

Until wheels are published, install directly from the repository:

```bash
python -m pip install "git+https://github.com/smprather/open-eda-file-tools.git"
```

For projects managed by `uv`:

```bash
uv add "open-eda-file-tools @ git+https://github.com/smprather/open-eda-file-tools.git"
```

This package contains a Rust extension, so source installs need:

- Python 3.14 or newer
- Rust toolchain with `cargo`
- a working compiler toolchain for your platform

## Develop Locally

Clone and build the editable extension:

```bash
git clone https://github.com/smprather/open-eda-file-tools.git
cd open-eda-file-tools
uv sync --dev
uv run maturin develop --release
```

Run tests:

```bash
uv run pytest -q
cargo fmt --all --check
cargo check
cargo test
```

The repository does not include large Liberty reference files. If you have a
local sample named `generic80_ss_125c_1p116v_0p84v.lib` in the repo root, the
large-file smoke tests will run; otherwise they are skipped.

Run the local browser viewer:

```bash
uv run liberty-view dev.lib
```

The viewer serves all frontend assets locally, including Plotly. See
[CLI reference](https://smprather.github.io/open-eda-file-tools/user-guide/cli.html)
for the full command line reference of
`liberty-view` and `liberty-format`. It renders:

- scalar tables as compact tables
- 1D tables as line plots
- 2D tables as a heatmap plus a lower plot that toggles between 3D surface
  and 2D wave families
- 3D/CCS tables as clickable grids that open wave plots
- CCSN `dc_current` as current and derived resistance surfaces in 3D mode
- output-pin Boolean functions as compact logic symbols

## Basic Usage

```python
import liberty_tools as lt

doc = lt.parse_file("generic80_ss_125c_1p116v_0p84v.lib")

print(doc.library_name)
print(doc.cells()[:10])

cell = doc.cell("and2d102srdh")
print(cell.area)
print(cell.pins())

pin = cell.pin("y")
arcs = pin.timing_arcs(related_pin="a")

for arc in arcs:
    print(arc.related_pin, arc.timing_type, arc.when, arc.tables())
```

## Timing Tables

Get one timing table from an arc:

```python
arc = doc.cell("and2d102srdh").pin("y").timing_arcs(related_pin="a")[0]
table = arc.table("cell_rise")

print(table.index_1)
print(table.index_2)
print(table.values)
```

### Flat Value Ordering

A table's `values` list is **flat**, in row-major order over the table's axes:
`index_1` is the **slowest** axis, `index_3` the **fastest**. For a 2-D table
with `len(index_1) = 2` and `len(index_2) = 3`:

```text
values[i * len(index_2) + j]   # i = position on index_1, j = position on index_2
```

For a 3-D table the same holds with `index_3` varying fastest:

```text
values[i * len(index_2) * len(index_3) + j * len(index_3) + k]
```

The long-form dataframes use the same convention: the `row` column indexes
`index_1`, `col` indexes `index_2` and `depth` indexes `index_3`.

### Looking Up A Value

Use `lookup()` instead of re-deriving the flat index. Give the coordinate on
each axis either by the axis's value or by the axis's physical variable:

```python
table.lookup(index_1=0.108, index_2=0.0966)   # -> float | None
table.lookup(load=0.108, transition=0.0966)    # by axis role
table.value_at(row=0, col=1)                   # by grid index
```

`lookup()` matches **axis values**, not grid positions: it returns `None`
when the coordinate given is not one of the table's axis values (it never
clamps and never raises for an off-grid point). Every axis of the table needs
a coordinate; a missing one raises `ValueError`. `value_at()` is the
by-grid-index accessor: it raises `IndexError` when the position is outside
the table, a negative position included.

### Axis Roles

Which axis is the load and which is the transition is a property of the
table's lookup template, and it differs between libraries. Ask the table:

```python
table.axis_roles()      # ["input_net_transition", "total_output_net_capacitance"]
table.axis_role(1)      # "input_net_transition"
table.index_1_variable  # "input_net_transition"
table.template          # "delay_template_7x7_x1"
```

The same information is on the document:

```python
doc.templates()["delay_template_7x7_x1"]
# ["input_net_transition", "total_output_net_capacitance", None]
```

Do not assume a fixed axis order: one library stores `index_1 =
input_net_transition`, another stores `index_1 =
total_output_net_capacitance`. Using the wrong one does not raise — it
silently reads the wrong cell.

### Extracting Rows

Extract timing table rows across the document:

```python
rows = doc.timing_tables(
    cell="and2d102srdh",
    pin="y",
    related_pin="a",
    table="cell_rise",
)
```

Convert directly to Polars:

```python
df = doc.to_polars(cell="and2d102srdh", table="cell_rise")
print(df)
```

The timing dataframe uses long-form rows with columns such as:

- `cell`
- `pin`
- `related_pin`
- `timing_type`
- `when`
- `table`
- `index_1`
- `index_2`
- `index_3`
- `row`
- `col`
- `depth`
- `value`

## Scalar Attributes And Units

The document, the cell and the pin expose their simple and complex
attributes as `(name, value)` string pairs, and the same `get()` accessor
works on all three:

```python
doc.get("default_max_transition")     # -> str | None
cell.get("cell_leakage_power")        # -> str | None
pin.get("capacitance")                # -> str | None
```

Buses and bundles have no attributes of their own — they carry a name, a
direction, a function and their member pins. The bus *type* is the one
exception: `bus_type.attributes()` returns a `dict[str, str]` rather than a
list of pairs, and `bus_type.get(key)` looks a key up in it.

Typed accessors return numbers instead, in the unit the library declares:

```python
pin.capacitance            # float | None, in the library's capacitive_load_unit
pin.capacitance_farads     # float | None, in farads
pin.max_transition         # float | None, in the library's time_unit
pin.max_transition_seconds # float | None, in seconds
cell.cell_leakage_power    # float | None, in the library's power unit
cell.cell_leakage_power_watts
doc.nom_voltage            # float | None, in the library's voltage_unit
```

The plain name carries the library's unit; the suffixed name carries SI.
Which unit the library uses is on the document:

```python
doc.time_unit              # "1ns"
doc.capacitive_load_unit   # "ff"
doc.capacitive_load_multiplier  # 1.0
doc.voltage_unit           # "1V"
doc.current_unit           # "1mA"
doc.leakage_power_unit     # "1pW"
doc.energy_unit_joules()   # the SI scale of internal_power energy values
```

## Minimum Clock Pulse Width

Two conventions express the same quantity, and a file may use either:

```text
pin (CLK) {
  min_pulse_width_high : 0.09117187499999997;   # attribute form
  min_pulse_width_low  : 0.7999999999999999;
}
```

```text
pin (CLK) {
  timing () {
    related_pin : "CLK";
    timing_type : min_pulse_width;              # timing group form
    rise_constraint (...) ;
    fall_constraint (...) ;
  }
}
```

`min_pulse_width()` normalizes both:

```python
width = cell.pin("CLK").min_pulse_width()
if width is not None:
    print(width.high, width.low, width.source)   # source: "attribute" | "timing_group"
```

The attribute form wins when a file uses both. For the timing group form the
reported number is the largest value of the constraint table — the tightest
limit over the tabulated clock transitions. Note that the two forms measure
slightly different things: the attribute form is the minimum *high/low phase*
width, the timing group form the minimum width *per clock transition*. The
`source` field says which one you are looking at.

## Writing A Liberty File

A parsed document can be written back out. The write is *surgical*: the
original text is kept byte for byte and only the attributes you changed are
rewritten, so comments, spacing and groups this library does not model all
survive.

```python
doc = lt.parse_file("cell.lib")
doc.cell("mycell").set_attribute("area", "59.8752")
doc.cell("mycell").pin("A").set_attribute("capacitance", 0.0044)

doc.write("out.lib")       # a .gz path is written gzip-compressed
text = doc.to_string()     # the same text as a string
```

`set_attribute()` adds the attribute when it is not there yet. Setting
`area` also updates `cell.area`. Writing refuses to touch a file whose
source changed since it was parsed.

The writer rules:

- Only the **last edit of an attribute** counts: setting the same attribute
  twice writes the value of the second call.
- The writer patches **the cell or the pin the caller addressed**. A pin
  name that matches more than one group of the cell, and that carries no
  container information, makes the **write fail** (`ValueError`) instead of
  patching an arbitrary one; reach the pin through its bus or bundle
  (`doc.cell("c").bus("B").pin("P")`) to say which one you mean.
- An attribute value is written **unquoted only when it matches
  `^[A-Za-z0-9_+.\-]+$`**; every other value is written as a quoted string
  with `\` and `"` escaped, so that the file parses back to exactly the
  value that was set.
- `set_attribute()` on a **pin** updates the typed pin fields immediately:
  `direction`, `function`, `three_state` and `power_down_function` (and the
  `*_expr()` accessors built on them) see the new value without a re-parse.

## Comparing Two Libraries

`compare()` matches the timing arcs of two documents, aligns their grids by
axis role and reports the differences on the common points:

```python
a = lt.parse_file("ours.lib")
b = lt.parse_file("pdk.lib")

diff = lt.compare(a, b, cells=["mycell"])
for entry in diff.entries:
    print(entry.arc, entry.table, entry.index_1, entry.index_2,
          entry.value_a, entry.value_b, entry.delta)

print(diff.summary())
print(diff.only_in_a)   # arcs that only exist in the first document
print(diff.only_in_b)
```

The comparison API lives in the module `liberty_tools.comparison`; the package
re-exports it, so `lt.compare` is the *function* and
`liberty_tools.comparison.Comparison` is the class.

A table the comparison cannot compare *honestly* is reported, never diffed:

```python
diff.unit_mismatches       # tables whose unit dimension differs between the libraries
diff.axis_role_mismatches  # tables whose axis roles are known on both sides but differ
diff.malformed_tables      # tables whose values do not fill their grid
diff.duplicate_arcs        # arcs whose identity is not unique
diff.duplicate_cells       # cell names that occur more than once in a document
diff.unmatched             # points with a value on only one side
diff.has_differences       # True when the two libraries differ in any way
```

- A table whose **unit dimension differs** between the two libraries — a
  time, a capacitive load, a voltage or a current — is reported as a
  `UnitMismatch`, not diffed: its numbers would measure the same quantity in
  different units.
- A table whose **axis roles are known on both sides but do not
  correspond** is reported as an `AxisRoleMismatch`, not matched
  positionally: matching by position would compare different quantities and
  produce plausible, wrong deltas.
- A table whose **`values` length does not match its grid** is reported as a
  `MalformedTable`, not interpolated: its values cannot be lined up with the
  points they belong to.
- A document that holds **several cell groups with the same name** is
  malformed: only the first group of a name can be addressed, so that group
  is the one that is compared. The repetition is reported in
  `diff.duplicate_cells` instead of being swallowed. The scan covers the
  whole document, even when `cells=` or `cell=` narrows the comparison to
  other cells, so a targeted comparison can still report a duplicated cell
  name.

Those three classes — `UnitMismatch`, `AxisRoleMismatch` and
`MalformedTable` — live in `liberty_tools.comparison` and are re-exported
from the package (`lt.UnitMismatch`, `lt.AxisRoleMismatch`,
`lt.MalformedTable`).

Arcs are matched by pin, related pin, timing type and timing sense; within
such a group the `when` conditions are matched by logical equivalence, so
`"A & B"` matches `"B & A"`. Tables are matched by name and their grids are
aligned by axis role, so the load axis of one library is compared against the
load axis of the other even when the two files store them in a different
order.

Points that have a value on only one side — missing from the other grid,
non-finite there, or dropped because the other side's table is malformed —
are reported with the missing side left as `None` and listed in
`diff.unmatched`; a point that has no value on either side is no difference
and is not listed. The reason a table was not compared is reported next to
it, in `diff.malformed_tables`, `diff.unit_mismatches` and
`diff.axis_role_mismatches`. Tables whose grids have a different number of
axes cannot be intersected or interpolated; they are reported in
`diff.shape_mismatches`.
With `interpolate=True` the second document is interpolated onto the first
document's grid instead:

```python
diff = lt.compare(a, b, cell="mycell", interpolate=True)
```

## Boolean `when` Filtering

`when` filters are parsed as Boolean expressions, not string matches. A query
matches an arc when the arc condition implies the requested condition.

```python
pin.timing_arcs(when="A")
doc.to_polars(when="A", table="cell_rise")
```

For a cell with inputs `A`, `B`, and `C`, querying `when="A"` matches arcs whose
conditions require `A=1`, regardless of the other pins:

- `A & B` matches
- `A & !B` matches
- `!A` does not match
- `A | B` does not match, because it can be true when `A=0`

Supported operators include `!`, `~`, postfix `'`, `&`, `*`, `|`, `+`, `^`, and
parentheses.

## Selective Parsing

For large libraries, restrict indexing to selected cells:

```python
doc = lt.parse_file(
    "generic80_ss_125c_1p116v_0p84v.lib",
    cells=["and2d102srdh", "and2d12sr"],
)
```

The parser still streams through the input file, but it only materializes data
for the requested cells.

## Current API

```python
lt.parse_file(path, cells=None) -> LibertyDocument
lt.compare(a, b, *, cells=None, cell=None, interpolate=False) -> Comparison

lt.UnitMismatch        # a table not compared: the unit dimension differs
lt.AxisRoleMismatch    # a table not compared: the axis roles do not correspond
lt.MalformedTable      # a table not compared: its values do not fill its grid

diff.entries
diff.only_in_a
diff.only_in_b
diff.shape_mismatches
diff.unit_mismatches
diff.axis_role_mismatches
diff.malformed_tables
diff.duplicate_arcs
diff.duplicate_cells
diff.matched
diff.mismatched
diff.unmatched
diff.has_differences
diff.max_abs_delta()
diff.summary()
diff.to_polars()

doc.library_name
doc.cells()
doc.cell(name)
doc.timing_tables(...)
doc.internal_power_tables(...)
doc.to_polars(kind="timing", ...)   # or kind="power"
doc.get(key)                        # library attribute value
doc.templates()                     # template name -> axis variable names
doc.operating_conditions()
doc.default_max_transition
doc.default_fanout_load
doc.nom_voltage
doc.nom_temperature
doc.capacitive_load_unit
doc.capacitive_load_multiplier
doc.leakage_power_unit
doc.energy_unit_joules()
doc.to_string()                     # the Liberty text, with edits applied
doc.write(path)                     # write it out

cell.name
cell.area
cell.pins()
cell.pin(name)
cell.buses()
cell.bus(name)
cell.bundles()
cell.bundle(name)
cell.get(key)
cell.set_attribute(key, value)
cell.set_area(value)
cell.cell_leakage_power
cell.cell_leakage_power_watts
cell.cell_footprint
cell.dont_touch
cell.dont_use

bus.name
bus.direction
bus.function
bus.bus_type
bus.pins()
bus.pin(name)

bundle.name
bundle.direction
bundle.function
bundle.members
bundle.pins()
bundle.pin(name)

doc.bus_types()
doc.bus_type(name)

pin.name
pin.direction
pin.function
pin.timing_arcs(related_pin=None, timing_type=None, when=None)
pin.internal_power()
pin.get(key)
pin.set_attribute(key, value)
pin.capacitance
pin.capacitance_farads
pin.rise_capacitance
pin.fall_capacitance
pin.max_transition
pin.max_transition_seconds
pin.min_pulse_width_high
pin.min_pulse_width_low
pin.min_pulse_width()               # both forms of the minimum pulse width

arc.related_pin
arc.timing_type
arc.when
arc.tables()
arc.table(name)
arc.ccsn_stages()                   # CCS-noise stage data (CCSN libs)

table.name
table.index_1
table.index_2
table.index_3
table.values                        # flat, row-major, index_1 slowest
table.template
table.axis_roles()
table.axis_role(n)
table.index_1_variable
table.lookup(index_1=..., index_2=..., load=..., transition=...)
table.value_at(row, col, depth=None)
table.to_polars()
```

## Viewer Notes

For 2D table views, the heatmap is always shown. The lower visualization
defaults to the 3D surface and can switch to 2D waves grouped by either table
axis. In 2D wave mode, each trace is one fixed axis value and the y-axis is the
table value. CCSN `dc_current` keeps its special dual-surface current/resistance
display in 3D mode; 2D mode plots only the original current table values.

Logic symbols are generated from Liberty Boolean expressions. Inversion bubbles
are intentionally small and prefer gate outputs: if an inverted subexpression is
itself a gate, the bubble is drawn on that source gate output rather than on the
receiving gate input. Equation labels are constrained to the symbol width and do
not determine the overall symbol size; the full equation is available as hover
text in the viewer.

## Notes

The Python package used for initial behavior comparison,
`liberty-parser`, is not part of the runtime dependency set. It remains useful
as a reference during development, but `liberty-tools` uses its own Rust parser.
