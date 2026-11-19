# DEF Tools (`def_tools` / `def-tools`)

`def_tools` parses and queries DEF (Design Exchange Format) designs (plain
`.def` and `.def.gz`) through the shared Rust backend.

- Python iterators (`instance_bboxes`, `net_shapes`) return integer **DEF database units (DBU)**.
  An error ends the iteration: it raises `RuntimeError` once, then the iterator is exhausted.
- Instance table/CSV/Parquet writers (`def-tools instances`, `write_instance_*`) convert coordinates to **microns** (`DBU / UNITS DISTANCE MICRONS`).

## `def-tools instances`

Dump placed instance bounding boxes (in microns) as CSV (default), Parquet, or
an aligned text table:

```bash
# Default CSV (6 columns: instance,base_cell,llx,lly,urx,ury in microns)
uv run def-tools instances \
  --lef design.lef --def design.def.gz \
  --output instances.csv

# Variadic/glob --lef, Parquet output, conditional is_seq and is_clock columns
uv run def-tools instances \
  --lef "pdk/*.lef" \
  --def design.def.gz \
  --lib stdcell.lib \
  --clock-cells clock_cells.txt \
  --format parquet \
  --output instances.parquet

# Custom column selection and renaming
uv run def-tools instances \
  --lef design.lef --def design.def.gz \
  --columns instance=inst_name,base_cell=cell,status,orient,llx=llx_um \
  --output instances.csv
```

### Conditional Columns

The instance dumper only emits columns supported by the inputs provided:

| Column | Available when | Meaning |
| --- | --- | --- |
| `instance,base_cell,llx,lly,urx,ury` | `--lef` + `--def` (always) | Default canonical 6 columns (coordinates in microns) |
| `status,orient` | `--lef` + `--def` (via `--columns`) | Placement status (`PLACED`/`FIXED`/`COVER`) and orientation |
| `is_seq` | `--lib LIBERTY` | `true` when the instance's macro has an `ff` or `latch` group in Liberty (parsed structurally with `skip_tables=True`) |
| `is_clock` | `--clock-cells PATH`, `--clock-data [BACKEND:]PATH` or `--twf PATH` | `true` when the instance is a clock-tree cell according to that source (see below) |

Requesting `is_seq` or `is_clock` in `--columns` without supplying its backing
input is an error, never a silent `false` column.

### Clock sources for `is_clock`

Three alternative inputs; supply one.

| Option | Source | Needs |
| --- | --- | --- |
| `--clock-cells PATH` | Instance names, one per line or a JSON list (e.g. from `sdc-tools clock-cells`) | — |
| `--clock-data [BACKEND:]PATH` | A clock/data classification source read by a backend | `--lib` for net/pin sources |
| `--twf PATH` | Shorthand for `--clock-data twf:PATH` | a plug-in `twf` backend |

```bash
uv run def-tools instances --lef "pdk/*.lef" --def design.def.gz \
  --lib stdcell.lib --clock-data design.otw.gz --output instances.csv
```

**Backends.** A backend reads one file and gives every net, pin or instance it
describes a role: `c` clock, `d` data, `u` unconstrained, `z` constant. Built
in: `otw` ([OTW](otw.md), this project's open timing-window format; chosen
automatically for `.otw` / `.otw.gz`) and `names` (the `--clock-cells` list).
There is no open TWF format, so **no vendor TWF reader ships here**; a `.twf`
file maps to the backend name `twf`, which must be plugged in.

**Plugging in a backend** (for example a vendor TWF reader kept outside this
repository). A backend is a callable `reader(path: Path) -> ClockData`:

```python
from pathlib import Path
from def_tools.clock_data import ClockData

def read(path: Path) -> ClockData:
    # nets: {net: role}; pins: {(instance, pin): role}; instances: {instance: role}
    return ClockData("twf", path, nets={"clk_leaf_3": "c"}, pins={("u_icg", "CK"): "c"})
```

Make it available by any of:

- an entry point in the `open_eda_file_tools.clock_data` group of an installed
  package: `twf = "my_package.twf_reader:read"`;
- the environment variable
  `OPEN_EDA_CLOCK_DATA_BACKENDS="twf=my_package.twf_reader:read"`
  (comma-separated, for builds without package metadata);
- `def_tools.clock_data.register_backend("twf", read)` from Python.

The built-in names `otw` and `names` cannot be replaced. Raise `ValueError`
from a reader for a malformed file.

**Semantics every backend shares.**

- *One role per object.* If the source holds several records for one object
  (one per launching clock edge is common, and then every clock net also has
  data records), the backend applies the precedence rule: **any clock record
  makes the object `c`**.
- *Net -> instances.* A clock net marks the instances that **drive** it,
  except flip-flops, latches and memories (a divider flop driving a generated
  clock is a clock source, not a clock-tree cell).
- *Pin -> instance.* A clock pin marks its instance when the instance is not
  such an endpoint and the pin is an output or a clock-through input — for an
  integrated clock gate (`clock_gating_integrated_cell`) the clock pin, never
  the enable or test pin.
- *Aggregation.* An instance is a clock cell when any one of its nets or pins
  marks it. Net-based and pin-based sources are both accepted and may be
  mixed; pin-based is recommended (no connectivity inference, and it tells a
  gate's clock pin from its enable).
- *Names.* Compared with DEF backslash escapes removed.

**Failure behaviour.** `is_clock` is never written from a source that could
not be read:

| Exit code | Meaning |
| --- | --- |
| `3` | The requested backend is not installed (`ClockDataBackendError`). |
| `4` | The backend ran but the source is malformed or describes no object (`ClockDataFormatError`). |
| `1` | Anything else: unreadable file, net/pin source without `--lib`, or a source none of whose clock objects exist in the DEF. |

Warnings (stderr, exit code 0): clock objects missing from the DEF, and
instances on clock objects whose macro has no Liberty cell — those are left
unmarked and counted, never guessed.

`def-tools clock-data --source ... --report objects|windows|instances`
inspects a source or prints the resolved instance list.

## `def-tools net-shapes` and Binary Cache

```bash
# Direct streaming query
uv run def-tools net-shapes --def design.def.gz --net VDD --layer met1 -o vdd_met1.csv

# Indexed binary cache for repeated queries on large designs
uv run def-tools cache create --def design.def.gz -o design.ldefcache
uv run def-tools net-shapes --cache-dir design.ldefcache --net VDD --layer met1 -o vdd_met1.csv
```

`net-shapes` reads every routed net an OpenROAD or Innovus DEF writes, not
just vendor-simple ones. Coordinates are integer DBU. `*` repeats the
previous point's coordinate; three-value points carry a wire extension that
widens the bbox past the segment endpoint; `VIRTUAL` points update the
previous point without producing a shape; `MASK`, `TAPER`, `TAPERRULE` and
`STYLE` are consumed. Each `RECT ( dx1 dy1 dx2 dy2 )` patch becomes a
`shape_type = "RECT"` row with `width` 0 and its normalized corners in
`x1,y1,x2,y2`; so does a net-level `+ RECT layer pt pt`, whose
`route_status` comes from a bare status right before it (DEF 5.8
`+ FIXED + RECT ...`) and is `ROUTED` otherwise. Regular-net wire `width` is
always 0 — widths come from technology LEF, which this command does not read.

Pieces the reader cannot place on a layer are skipped and counted, never
attributed to a guessed layer: segments after a via as `after_via` (the
via's other routing layer needs via definitions), `+ POLYGON` patches on
the queried layer as `polygon`, and each `+ SUBNET` that carries wiring as
`subnet`. Net-level `+` keywords (`USE`, `VPIN`, `WEIGHT`, …) end the
current path so their parenthesized values never become route points, and
route keywords outside a path (a `+ PROPERTY` value such as `RECT`) are
ignored. A coordinate or bbox edge outside the 64-bit range is an error
naming the DEF byte. When any count is non-zero
the CLI prints one warning line to stderr; `write_net_shapes_csv` /
`write_net_shapes_csv_from_cache` return the counts as a dict, and the
`def_tools.net_shapes` iterator exposes them via `.skipped`. Cache queries
return identical rows and counts to direct scans.

## Performance and CPU use

All DEF commands stream: memory does not grow with design size. The scan that
splits the file into records is sequential and runs on its own thread, one
batch ahead of the parallel parse; records are skipped without being copied
when a command does not need them (`net-shapes` reads only the matching net,
the clock trace keeps only each net's connection list, not its routing).
Very long connection lists, such as power nets that list every cell, are
split and parsed in parallel.

`--threads N` (or `def_tools.set_threads(N)`) limits a run to N CPUs; the
default is all logical CPUs. The scan thread counts inside the limit, and with
N = 1 nothing overlaps. Output is identical for every N.

Measured on a Ryzen 9 5900X (12 cores / 24 threads), files in page cache:

| Command | 458 MB synthetic, 2M instances | 118 MB routed cva6, 302k instances |
| --- | --- | --- |
| `instances` CSV | 0.53 s | 0.08 s |
| `instances` Parquet | 0.46 s | 0.08 s |
| `instances` table | 1.06 s | 0.15 s |
| `net-shapes` (one signal net) | 0.08 s | 0.02 s |
| `cache create` | 1.24 s | 0.25 s |
| clock trace (`sdc-tools clock-cells` core) | 2.66 s | 0.16 s |

The table format scans the DEF twice (column widths first). The clock trace
holds every connection as two strings; that memory layout, not the scan, is
now its main cost.

## Python API

The same readers are also available as a plain Rust rlib with no Python
dependency: `open-eda-file-tools-core` — `def::scan_instance_bboxes`,
`def::dbu_per_micron`, `def::scan_net_shapes`, `lef::macro_sizes`,
`liberty::cell_attributes` / `liberty::LibraryIndex`. See
[docs/ai-api.md §8](../ai-api.md).

```python
import def_tools

# Streaming iterators (DBU integers):
for bbox in def_tools.instance_bboxes(["cells.lef"], "design.def.gz"):
    print(bbox.instance, bbox.macro_name, bbox.status, bbox.orient, bbox.llx, bbox.lly, bbox.urx, bbox.ury)

for shape in def_tools.net_shapes("design.def.gz", "VDD", "met1"):
    print(shape.net, shape.layer, shape.llx, shape.lly, shape.urx, shape.ury)

# Native micron writers:
rows = def_tools.write_instance_bbox_csv(["cells.lef"], "design.def.gz", "instances.csv")
rows = def_tools.write_instance_table(["cells.lef"], "design.def.gz", "instances.table")
rows, row_groups = def_tools.write_instance_parquet(["cells.lef"], "design.def.gz", "instances.parquet", 131_072)
```
