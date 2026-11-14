# AI Agent API & CLI Reference

This document is the single-page reference for AI coding agents consuming
`open-eda-file-tools`. It covers all five Python libraries (`liberty_tools`,
`lef_tools`, `def_tools`, `sdc_tools`, `pex_tools`) and all six command-line
entry points (`liberty-tools`, `liberty-format`, `liberty-view`, `lef-tools`,
`def-tools`, `sdc-tools`, `pex-tools`).

---

## 1. Quick Decision Table

| Task | Best Entry Point | Key Flag / Option | Output / Return |
| --- | --- | --- | --- |
| Parse full Liberty `.lib` / `.lib.gz` | `liberty_tools.parse_file(path, cells=...)` | `cells=["..."]` skips unselected cells at byte level | `LibertyDocument` |
| Parse Liberty structure only (pins, `ff`/`latch`, arcs — no float tables) | `liberty_tools.parse_file(path, skip_tables=True)` | `skip_tables=True` (36% faster, skips table bodies) | `LibertyDocument` (table lists empty) |
| Lazy random-access cell lookup on huge `.lib` | `liberty_tools.LibraryIndex.open(path)` | `.cell(name)` parses one cell on demand | `LibraryIndex` |
| Extract timing or switching-energy tables to Polars | `doc.to_polars(kind="timing" \| "power", **filters)` | `when="..."` uses Boolean implication | `polars.DataFrame` |
| Compare two Liberty libraries | `liberty_tools.compare(a, b, ...)` | Aligns grids by axis role, never guesses | `Comparison` |
| Format a Liberty file transparently | `liberty-tools format FILE` (or `liberty-format FILE`) | `--in-place`, `--check`, `-o PATH` | Formatted `.lib` |
| Parse cell LEF macros, pins, ports, rects | `lef_tools.parse_lef(paths, macros=...)` | Coordinates always in **LEF microns** | `LefLibrary` |
| Dump placed DEF instances + bboxes (CSV, Parquet, table) | `def-tools instances --lef ... --def ...` | `--format csv\|parquet\|table`, `--lib`, `--clock-cells` / `--clock-data` / `--twf`, `--columns` | Coordinates in **microns** (`DBU / UNITS DISTANCE MICRONS`) |
| Stream placed DEF instance bboxes in Python | `def_tools.instance_bboxes(lefs, def_path)` | Streaming Rust iterator | `InstanceBBoxIter` (coordinates in **DEF DBU**) |
| Query routed net shapes for one net + layer | `def-tools net-shapes --def ... --net VDD --layer met1` | `--cache-dir` for repeated queries | CSV in **DEF DBU** |
| Extract SDC clocks (`create_clock` / `create_generated_clock`) | `sdc_tools.clocks(sdc_path)` or `sdc-tools clocks` | Evaluates in safe `tclsh` >= 8.6 with stubs | `list[Clock]` / TSV / JSON |
| Positively identify clock-tree cells from SDC + DEF + Liberty | `sdc_tools.clock_cells(def_path, libs, sdc_path)` or `sdc-tools clock-cells` | Forward trace from clock roots; through ICGs, stops at `ff`/`latch`/memory, pruned to cells that reach a clock pin | `ClockCells` / instance list |
| Identify clock-tree cells from STA clock/data roles (OTW or plug-in backend) + DEF + Liberty | `def_tools.clock_data.clock_instances(spec, def_path, roles)` or `def-tools instances --clock-data [BACKEND:]PATH` | Net- and pin-based; exit 3 = backend missing, 4 = malformed | `ClockInstances` / `is_clock` column |
| Read / write OTW (open timing windows) | `def_tools.otw.read_otw(path)` / `write_otw(path, ...)` | Spec: `docs/otw.md`; no vendor TWF reader ships | `OtwFile` |
| Summarize SPEF/DSPF parasitics or build capacitance histogram | `pex_tools.summaries(path)` / `pex_tools.histogram(path)` / `pex-tools` | `mode="auto"` (parallel Rayon) or `mode="stream"` (low-memory) | `list[PexSummary]` / `PexHistogram` |
| Parse full SPEF/DSPF conns, caps, resistors, and `$` extensions | `pex_tools.parse(path)` | Preserves `$layer=`, `$x=`, `$y=`, and DSPF `*C x y` | `PexFile` |

---

## 2. End-to-End Multi-Tool Recipe: Instance Table with `is_seq` and `is_clock`

To produce a Parquet or CSV instance table with bounding boxes (in microns),
`is_seq` (from Liberty `ff`/`latch`), and `is_clock` (positive forward trace
from SDC clock roots through DEF connectivity):

### CLI pipeline

```bash
# 1. Trace clock-tree distribution cells (buffers/inverters/clock gates) from SDC roots:
uv run sdc-tools clock-cells \
  --sdc design.sdc \
  --def design.def.gz \
  --lib stdcell.lib \
  --output clock_cells.txt

# 2. Dump placed instances with both is_seq and is_clock columns:
uv run def-tools instances \
  --lef "pdk/*.lef" \
  --def design.def.gz \
  --lib stdcell.lib \
  --clock-cells clock_cells.txt \
  --format parquet \
  --output instances.parquet
```

### Python orchestrator

```python
from pathlib import Path
import sdc_tools
import def_tools
from def_tools.instances import default_instance_columns, sequential_macros

lefs = [Path("stdcell.lef")]
libs = [Path("stdcell.lib")]
def_path = Path("design.def.gz")
sdc_path = Path("design.sdc")

seq_macros = sorted(sequential_macros(libs))
clk_cells = list(sdc_tools.clock_cells(def_path, libs, sdc_path).cells)
columns = default_instance_columns(has_sequential=True, has_clock=True)

rows, row_groups = def_tools.write_instance_parquet(
    lefs,
    def_path,
    "instances.parquet",
    batch_size=131_072,
    columns=columns,
    sequential=seq_macros,
    clock=clk_cells,
)
```

### `is_clock` from STA clock/data roles instead of an SDC trace

```bash
# design.otw.gz: OTW (docs/otw.md), net-based or pin-based. --lib is required for net/pin sources.
uv run def-tools instances --lef "pdk/*.lef" --def design.def.gz \
  --lib stdcell.lib --clock-data design.otw.gz --output instances.csv
# Vendor TWF: only through a plug-in backend named `twf` (none ships here):
OPEN_EDA_CLOCK_DATA_BACKENDS="twf=my_pkg.twf_reader:read" \
  uv run def-tools instances ... --lib stdcell.lib --twf design.twf.gz
# Exit codes: 3 = backend not installed, 4 = source malformed, 1 = other error.
```

---

## 3. `liberty_tools` — Exact Python API

```python
import liberty_tools as lt

# Parse a .lib or .lib.gz file.
# - cells=[...]: skip unselected cell bodies at parse time.
# - skip_tables=True: byte-skip all floating-point table bodies (36% faster) when
#   only pins, directions, attributes, ff/latch, or timing-arc scalars are needed.
lt.parse_file(
    path: str | Path,
    cells: Sequence[str] | None = None,
    *,
    skip_tables: bool = False,
) -> lt.LibertyDocument

# Lazy cell-indexed reader (~0.15s open on a 97 MB library):
index = lt.LibraryIndex.open(path: str | Path) -> lt.LibraryIndex
index.library_name -> str
index.cell_names() -> list[str]
index.num_cells() -> int
index.cell(name: str) -> lt.Cell
index.cell_source(name: str) -> str
index.templates() -> dict[str, list[str | None]]
index.attributes() -> list[tuple[str, str]]
index.energy_unit_joules() -> float | None

# Document properties & methods:
doc.library_name -> str
doc.voltage_unit -> str | None
doc.current_unit -> str | None
doc.time_unit -> str | None
doc.capacitive_load_unit -> str | None
doc.capacitive_load_multiplier -> float | None
doc.leakage_power_unit -> str | None
doc.default_max_transition -> float | None
doc.default_fanout_load -> float | None
doc.nom_voltage -> float | None
doc.nom_temperature -> float | None
doc.cells() -> list[str]
doc.cell(name: str) -> lt.Cell
doc.bus_types() -> list[str]
doc.bus_type(name: str) -> lt.BusType
doc.templates() -> dict[str, list[str | None]]
doc.attributes() -> list[tuple[str, str]]
doc.get(key: str) -> str | None
doc.operating_conditions() -> list[tuple[str, list[tuple[str, str]]]]
doc.operating_condition(name: str) -> list[tuple[str, str]]
doc.energy_unit_joules() -> float | None
doc.driver_waveforms() -> list[lt.TimingTable]
doc.timing_tables(**filters) -> list[dict[str, object]]
doc.internal_power_tables(**filters) -> list[dict[str, object]]
doc.to_polars(kind: Literal["timing", "power"] = "timing", **filters) -> polars.DataFrame
doc.to_string() -> str
doc.write(path: str | Path) -> None

# Cell properties & methods:
cell.name -> str
cell.area -> float | None
cell.pins() -> list[str]
cell.pin(name: str) -> lt.Pin
cell.buses() -> list[str]
cell.bus(name: str) -> lt.Bus
cell.bundles() -> list[str]
cell.bundle(name: str) -> lt.Bundle
cell.attributes() -> list[tuple[str, str]]
cell.get(key: str) -> str | None
cell.set_attribute(key: str, value: str | float | int) -> None
cell.set_area(value: float) -> None
cell.cell_leakage_power -> float | None
cell.cell_leakage_power_watts -> float | None
cell.cell_footprint -> str | None
cell.dont_touch -> str | None
cell.dont_use -> str | None
cell.dynamic_currents() -> list[lt.DynamicCurrent]
cell.ff() -> lt.Ff | None
cell.latch() -> lt.Latch | None
cell.leakage_powers() -> list[lt.LeakagePower]
cell.pg_pins() -> list[lt.PgPin]

# Pin properties & methods:
pin.name -> str
pin.direction -> str | None
pin.function -> str | None
pin.three_state -> str | None
pin.power_down_function -> str | None
pin.attributes() -> list[tuple[str, str]]
pin.get(key: str) -> str | None
pin.set_attribute(key: str, value: str | float | int) -> None
pin.capacitance -> float | None
pin.capacitance_farads -> float | None
pin.rise_capacitance -> float | None
pin.rise_capacitance_farads -> float | None
pin.fall_capacitance -> float | None
pin.fall_capacitance_farads -> float | None
pin.max_transition -> float | None
pin.max_transition_seconds -> float | None
pin.min_pulse_width_high -> float | None
pin.min_pulse_width_high_seconds -> float | None
pin.min_pulse_width_low -> float | None
pin.min_pulse_width_low_seconds -> float | None
pin.min_pulse_width() -> lt.MinPulseWidth | None
pin.timing_arcs(related_pin=None, timing_type=None, when=None) -> list[lt.TimingArc]
pin.internal_power(related_pin=None, related_pg_pin=None, when=None) -> list[lt.InternalPower]
pin.function_expr() -> lt.BooleanExpression | None
pin.three_state_expr() -> lt.BooleanExpression | None
pin.power_down_function_expr() -> lt.BooleanExpression | None

# TimingArc & TimingTable:
arc.related_pin -> str | None
arc.timing_type -> str | None
arc.when -> str | None
arc.sdf_cond -> str | None
arc.timing_sense -> str | None
arc.tables() -> list[str]
arc.table(name: str) -> lt.TimingTable
arc.when_expr() -> lt.BooleanExpression | None
arc.sdf_cond_expr() -> lt.BooleanExpression | None
arc.ccsn_stages() -> list[lt.CcsnStage]

table.name -> str
table.index_1 -> list[float]
table.index_2 -> list[float]
table.index_3 -> list[float]
table.values -> list[float]   # flat row-major: index_1 slowest, index_3 fastest
table.template -> str | None
table.axis_roles() -> list[str | None]
table.axis_role(axis: int) -> str | None   # 1-based: 1, 2, or 3
table.lookup(index_1=None, index_2=None, index_3=None, **roles) -> float | None
table.value_at(row: int, col: int, depth: int | None = None) -> float
table.vectors() -> list[lt.TimingTable]
table.to_polars() -> polars.DataFrame

# Library Comparison:
diff = lt.compare(
    a, b, *, cells=None, cell=None, pin=None, table=None,
    interpolate=False, grid=None, tolerance=1e-9,
) -> lt.Comparison
diff.has_differences -> bool
diff.entries -> list[lt.CompareEntry]
diff.matched -> list[lt.CompareEntry]
diff.mismatched -> list[lt.CompareEntry]
diff.unmatched -> list[lt.CompareEntry]
diff.only_in_a -> list[lt.ArcKey]
diff.only_in_b -> list[lt.ArcKey]
diff.unit_mismatches -> list[lt.UnitMismatch]
diff.axis_role_mismatches -> list[lt.AxisRoleMismatch]
diff.malformed_tables -> list[lt.MalformedTable]
diff.summary() -> dict[str, object]
diff.to_polars() -> polars.DataFrame
```

### Critical Liberty Invariants

1. **Axis roles are per-table (`table.axis_roles()`)**: one library stores
   `index_1 = input_net_transition`, another stores `index_1 =
   total_output_net_capacitance`. Query by role (`table.lookup(load=...,
   transition=...)`) instead of assuming a fixed axis order.
2. **Flat table values are row-major**: `index_1` varies slowest, `index_3`
   fastest. Use `table.lookup()` (by coordinate value) or `table.value_at()`
   (by 0-based grid index).
3. **`when` filter is Boolean implication**: `when="A"` matches `"A & B"` and
   `"A & !B"`, but not `"A | B"` or arcs without a `when` attribute.
4. **`internal_power` is switching energy in joules**: multiply table values by
   `doc.energy_unit_joules()`.

---

## 4. `lef_tools` — Exact Python API

```python
import lef_tools

library = lef_tools.parse_lef(
    paths: str | Path | Sequence[str | Path],
    *,
    macros: Sequence[str] | None = None,
) -> lef_tools.LefLibrary

library.macro_names() -> list[str]
library.macros() -> list[lef_tools.LefMacro]
library.macro(name: str) -> lef_tools.LefMacro          # raises KeyError if absent
library.duplicate_macro_names() -> list[str]            # last definition wins

macro.name -> str
macro.width_microns -> float
macro.height_microns -> float
macro.origin_x_microns -> float
macro.origin_y_microns -> float
macro.pin_names() -> list[str]
macro.pins() -> list[lef_tools.LefPin]
macro.pin(name: str) -> lef_tools.LefPin                # raises KeyError if absent

pin.name -> str
pin.direction -> str | None                             # lowercase: "input", "output", "inout"
pin.use -> str | None                                   # lowercase: "signal", "power", "ground", "clock"
pin.ports() -> list[lef_tools.LefPort]

port.rects() -> list[lef_tools.LefRect]
port.has_unsupported_geometry -> bool                   # True when POLYGON/PATH/VIA encountered

rect.layer -> str
rect.x0_microns -> float                                # normalized so x0 <= x1, y0 <= y1
rect.y0_microns -> float
rect.x1_microns -> float
rect.y1_microns -> float
```

---

## 5. `def_tools` — Exact Python API

```python
import def_tools

# Streaming iterators (coordinates in DEF database units, integer DBU):
for bbox in def_tools.instance_bboxes(lefs: Sequence[str | Path], def_path: str | Path):
    bbox.instance -> str
    bbox.macro_name -> str
    bbox.status -> str          # "PLACED" | "FIXED" | "COVER" (UNPLACED skipped)
    bbox.orient -> str          # "N" | "S" | "E" | "W" | "FN" | "FS" | "FE" | "FW"
    bbox.llx -> int             # DBU
    bbox.lly -> int             # DBU
    bbox.urx -> int             # DBU
    bbox.ury -> int             # DBU
    bbox.as_tuple() -> tuple[str, str, str, str, int, int, int, int]

for shape in def_tools.net_shapes(def_path: str | Path, net: str, layer: str):
    shape.net -> str
    shape.section -> str        # "NETS" | "SPECIALNETS"
    shape.route_status -> str
    shape.shape_type -> str
    shape.layer -> str
    shape.width -> int          # DBU
    shape.llx -> int            # DBU
    shape.lly -> int            # DBU
    shape.urx -> int            # DBU
    shape.ury -> int            # DBU

# Native writers (coordinates in MICRONS = DBU / UNITS DISTANCE MICRONS):
# columns: optional list of (source_name, output_name) pairs.
# Sources: "instance", "base_cell", "status", "orient", "is_seq", "is_clock", "llx", "lly", "urx", "ury".
# "is_seq" requires sequential=[macro_names...]; "is_clock" requires clock=[instance_names...].
def_tools.write_instance_bbox_csv(
    lefs, def_path, output: str | Path | None,
    columns=None, sequential=None, clock=None,
) -> int

def_tools.write_instance_table(
    lefs, def_path, output: str | Path | None,
    columns=None, sequential=None, clock=None,
) -> int

def_tools.write_instance_parquet(
    lefs, def_path, output: str | Path, batch_size: int,
    columns=None, sequential=None, clock=None,
) -> tuple[int, int]   # (rows_written, row_groups)

# One-call resolution of the is_seq / is_clock inputs (what the CLI does):
from def_tools.instances import instance_flag_inputs
sequential, clock, warnings = instance_flag_inputs(
    def_path, libs,                 # libs parsed once (skip_tables=True) for both columns
    clock_cells=None,               # Path: instance-name list            } alternatives;
    clock_data=None,                # "[BACKEND:]PATH", e.g. "x.otw.gz"   } more than one
    twf=None,                       # Path: shorthand for "twf:PATH"      } -> ValueError
) -> tuple[list[str] | None, list[str] | None, list[str]]   # None = input not supplied

# Liberty clock roles (shared by sdc_tools.clock_cells and clock-data resolution):
from def_tools.clock_roles import cell_clock_roles, CellClockRole
roles = cell_clock_roles(libs) -> dict[str, CellClockRole]
role.inputs -> tuple[str, ...]      # clock-through inputs (ICG: clock pin only)
role.outputs -> tuple[str, ...]     # (ICG: clock_gate_out_pin if flagged)
role.endpoint -> bool               # ff / latch / memory / clock:true pin, and not an ICG
role.clock_pins -> tuple[str, ...]  # pins a clock is delivered to
role.clock_gate -> bool             # clock_gating_integrated_cell present
role.sequential -> bool             # ff or latch group (the is_seq definition; true for ICGs)

# Clock/data classification backends (def_tools.clock_data):
from def_tools.clock_data import (
    ClockData, ClockInstances, read_clock_data, resolve_clock_instances, clock_instances,
    register_backend, backends, parse_source,
    ClockDataError, ClockDataBackendError, ClockDataFormatError,
)
data = read_clock_data("design.otw.gz" | "otw:PATH" | "names:PATH" | "<plugin>:PATH") -> ClockData
data.backend -> str; data.source -> Path
data.nets -> Mapping[str, str]                    # net -> role ("c" | "d" | "u" | "z")
data.pins -> Mapping[tuple[str, str], str]        # (instance, pin) -> role; instance "-" = port
data.instances -> Mapping[str, str]               # instance -> role
data.domains -> Mapping[str | tuple[str, str], tuple[str, ...]]   # optional: object -> clock names (not used for is_clock)
data.clock_nets() / data.clock_pins() / data.clock_instances()   # objects with role "c"
result = resolve_clock_instances(data, def_path, roles) -> ClockInstances
result = clock_instances(spec, def_path, roles)   # read + resolve
result.cells -> tuple[str, ...]                   # sorted DEF instance names
result.unmatched_nets / result.unmatched_pins     # clock objects absent from the DEF
result.unclassified -> tuple[str, ...]            # on a clock object, macro not in Liberty: NOT in cells
result.warnings() -> list[str]
# Errors: ClockDataBackendError (.exit_code 3, backend not installed),
#         ClockDataFormatError (.exit_code 4, malformed / empty source),
#         ClockDataError (.exit_code 1: no --lib for nets/pins, or no clock object exists in the DEF).
# Plug-in: reader(path: Path) -> ClockData, registered via register_backend(name, reader),
#   entry-point group "open_eda_file_tools.clock_data", or env OPEN_EDA_CLOCK_DATA_BACKENDS
#   ("name=pkg.module:reader,..."). Built-ins "otw" and "names" cannot be replaced.
# Backend contract: one role per object; any clock record makes the object "c".

# OTW (def_tools.otw), formal spec in docs/otw.md:
from def_tools.otw import read_otw, write_otw, write_otw_windows_csv, OtwClock, OtwWindow
otw = read_otw(path) -> OtwFile     # ValueError "file:line: ..." when malformed
otw.design -> str; otw.clocks -> tuple[OtwClock, ...]   # OtwClock(name, period, rise_edge, fall_edge, attributes=())
otw.time_unit / otw.cap_unit / otw.res_unit -> float | None   # seconds / farads / ohms per unit
otw.slew_thresholds -> tuple[float, float] | None             # percent of swing
otw.attributes -> tuple[tuple[str, str], ...]                 # other header lines (corner, program, ...)
otw.windows -> int                                            # count of w lines
otw.nets -> dict[str, str]; otw.pins -> dict[tuple[str, str], str]   # object -> role
otw.domains -> dict[str | tuple[str, str], tuple[int, ...]]   # clock indices: clocks= attr, else window clocks
otw.domain_names(key) -> tuple[str, ...]
otw.object_attributes -> dict[key, dict[str, str]]            # freq (Hz), load (cap_unit range), value (0|1 for z),
                                                              # drive_rise / drive_fall (res_unit range), free keys
# OtwWindow(clock: int | None, edge: "r" | "f" | "-", role, values: 8-tuple of float | None,
#           rise_res=None, rise_slack=None, fall_res=None, fall_slack=None,   # (min, max) ranges
#           attributes=())    # values = rise_early, rise_late, fall_early, fall_late, then 4 slews
write_otw(path, *, design="", time_unit=None, cap_unit=None, res_unit=None, slew_thresholds=None,
          clocks=(), attributes=(), nets=None, pins=None, windows=None,
          domains=None, object_attributes=None, precision=6) -> None   # ".gz" suffix gzips
# Text grammar (docs/otw.md): `n <net> <role>`; `i <instance>` then `p <pin> <role>` (instance `-` = ports);
#   `w <clock|-> <r|f|-> <role> <8 decimals or ->`; trailing key=value attrs; mandatory `end <nets> <pins> <windows>`.
#   Body numbers are fixed-point decimals (no exponent); precision = max fractional digits written.
# Native reader parses group-aligned blocks in parallel (rayon); honors RAYON_NUM_THREADS.
write_otw_windows_csv(path, output: str | None) -> int     # one CSV row per window

# Binary DEF cache for repeated net/clock queries:
def_tools.write_def_cache(def_path: str | Path, cache_dir: str | Path) -> None
def_tools.write_net_shapes_csv(def_path, net, layer, output) -> None
def_tools.write_net_shapes_csv_from_cache(cache_dir, net, layer, output) -> None
```

---

## 6. `sdc_tools` — Exact Python API

Requires system `tclsh` >= 8.6 on `PATH` (or `TCLSH=/path/to/tclsh`). Evaluates
SDC in a Tcl safe interpreter (`interp create -safe`) with stubbed SDC commands.

```python
import sdc_tools

# Extract created clocks in file order:
for clk in sdc_tools.clocks(sdc_path: str | Path):
    clk.name -> str
    clk.period -> float | None
    clk.waveform -> tuple[float, ...]
    clk.sources -> tuple[str, ...]          # symbolic: ("port:clk", "pin:div/Q", ...)
    clk.generated_from -> str | None
    clk.divide_by -> int | None
    clk.multiply_by -> int | None
    clk.edges -> tuple[float, ...]
    clk.invert -> bool

# Forward-trace clock-tree distribution cells from SDC roots through DEF connectivity:
# Uses Liberty (parsed concurrently with skip_tables=True) for pin directions and
# ff/latch/memory endpoints and clock gates. Traces through integrated clock gates
# (clock pin -> gated clock; never via the enable pin). Never uses macro-name patterns.
# prune=True (default): report only cells from which a clock pin is reachable, so the
# datapath cone of a divider flop's output is not reported. prune=False: every cell touched.
result = sdc_tools.clock_cells(
    def_path: str | Path,
    libs: Sequence[Path | str],
    sdc_path: str | Path,
    *,
    prune: bool = True,
) -> sdc_tools.ClockCells
result.clocks -> tuple[sdc_tools.Clock, ...]
result.cells -> tuple[str, ...]             # sorted clock-tree instance names
```

---

## 7. `pex_tools` — Exact Python API

Reads SPEF and DSPF (`.spef`, `.spef.gz`, `.dspf`, `.dspf.gz`).
`mode="auto"` parses net blocks in parallel with Rayon; `mode="stream"` buffers
one net block at a time for a ~5× lower memory footprint.

```python
import pex_tools

# Header only (fast, does not scan nets):
hdr = pex_tools.header(path: str | Path) -> pex_tools.PexHeader
hdr.standard -> str
hdr.design -> str
hdr.t_unit -> str
hdr.c_unit -> str
hdr.r_unit -> str
hdr.l_unit -> str
hdr.divider -> str
hdr.delimiter -> str
hdr.bus_delimiter -> str
hdr.ports -> list[tuple[str, str]]

# Per-net summaries (no element lists materialized):
for net in pex_tools.summaries(path: str | Path, mode: str = "auto"):
    net.name -> str
    net.total_cap -> float
    net.ground_cap -> float
    net.coupling_cap -> float
    net.cap_count -> int
    net.res_count -> int
    net.res_total -> float

# Capacitance histogram computed in Rust:
hist = pex_tools.histogram(path: str | Path, bins: int = 20, mode: str = "auto") -> pex_tools.PexHistogram
hist.min -> float
hist.max -> float
hist.bounds -> list[float]      # length = bins + 1
hist.counts -> list[int]        # length = bins

# Full parse (conns, caps, resistors, DSPF *C coordinates, and $ extensions):
pex = pex_tools.parse(path: str | Path, keep_entries: bool = True, mode: str = "auto") -> pex_tools.PexFile
pex.header -> pex_tools.PexHeader
for net in pex.nets:
    net.name -> str
    net.total_cap -> float
    net.ground_cap -> float
    net.coupling_cap -> float
    net.ext -> str | None
    net.conns -> list[pex_tools.PexConn]    # .name, .direction, .attrs, .x, .y
    net.caps -> list[pex_tools.PexCap]      # .id, .node1, .node2 (None=ground), .value, .ext
    net.res -> list[pex_tools.PexRes]       # .id, .node1, .node2, .value, .ext
```
