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

### Hierarchical DEF instances

Pass a top DEF with `--def` and repeat `--block-def MACRO=PATH` for each macro
whose contents live in another DEF. Match the macro name used in
`COMPONENTS`. The child file's `DESIGN` statement may use a different name.
Both the top and the children accept plain DEF or gzip input.

```bash
def-tools instances --lef "pdk/*.lef" --def top.def \
  --block-def cpu=cpu.def.gz --block-def cache=cache.def \
  --format parquet --output instances.parquet
```

The dump emits each placed block, followed by its descendants in DEF file
order. Instance paths use `/`, such as `cpu0/cache/ff0`. Bounding boxes and the
optional `orient` column are relative to the top design. Placement status
comes from the row's own source DEF. A child DEF's `DIEAREA` bounding rectangle
defines the mapped block's footprint and anchors its placement. Leaf cells
use their LEF sizes. Unplaced components have no bounding box and are skipped.
Cyclic block mappings and incomplete `COMPONENTS` sections are errors.

In Python, use `def_tools.read_def_hierarchy(lefs, def_path, block_defs)` and
iterate `hierarchy.instances()`. In Rust, use `def::read_def_hierarchy` and
`DefHierarchy::instances`. Both APIs return integer bounding boxes at the
hierarchy's common DBU scale, the least common multiple of the input DEF
scales. Read the scale from `hierarchy.dbu_per_micron` in Python or
`hierarchy.dbu_per_micron()` in Rust. The CLI outputs microns. For hierarchical
`--clock-cells` input, name descendants by their full instance paths.
Net and pin classification uses the top DEF's connectivity.

#### Four-CVA6 example

The routed `~/openlane2-cva6/cva6.def.gz` has a
`1823600 × 1842320` DBU die at 1,000 DBU/µm. This top DEF places four
copies with 50 µm margins and gaps.

```bash
cat > cva6_quad.def <<'END_DEF'
VERSION 5.8 ;
DIVIDERCHAR "/" ;
BUSBITCHARS "[]" ;
DESIGN cva6_quad ;
UNITS DISTANCE MICRONS 1000 ;
DIEAREA ( 0 0 ) ( 3797200 3834640 ) ;
COMPONENTS 4 ;
  - cva6_sw cva6_top + FIXED ( 50000 50000 ) N ;
  - cva6_se cva6_top + FIXED ( 1923600 50000 ) N ;
  - cva6_nw cva6_top + FIXED ( 50000 1942320 ) N ;
  - cva6_ne cva6_top + FIXED ( 1923600 1942320 ) N ;
END COMPONENTS
END DESIGN
END_DEF
pdk=~/pdk/IHP-Open-PDK/ihp-sg13g2/libs.ref
def-tools instances --threads 4 \
  --lef "$pdk/sg13g2_stdcell/lef/sg13g2_stdcell.lef" "$pdk/sg13g2_io/lef/sg13g2_io.lef" \
  --def cva6_quad.def --block-def "cva6_top=$HOME/openlane2-cva6/cva6.def.gz" \
  --output cva6_quad.csv
```

The fixture produces 1,206,204 rows, with four block rows and 301,550
placed descendants per block.

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
- A clock net marks the instances that **drive** it, including sequential
  drivers such as divider flip-flops.
- A clock pin marks its instance through an output or a clock-through
  input. A sequential endpoint qualifies through its output only. For an
  integrated clock gate, its clock input qualifies.
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

The commands that write rows (`instances`, `net-shapes`, `cache create`) stream:
memory does not grow with design size. The connectivity commands (clock
trace, `clock-instances` and clock-data resolution) hold the whole design's
connectivity, as flat integer arrays: 724 MB peak RSS on the 2M-instance
synthetic design, 91 MB on cva6. The scan that
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
| `instances` CSV | 0.55 s | 0.07 s |
| `instances` Parquet | 0.46 s | 0.08 s |
| `instances` table | 1.06 s | 0.14 s |
| `net-shapes` (one signal net) | 0.08 s | 0.02 s |
| `cache create` | 1.01 s | 0.24 s |
| clock trace (`sdc-tools clock-cells` core) | 1.31 s | 0.11 s |

The table format scans the DEF twice (column widths first). The clock trace
runs on a flat id-based connectivity (`core/src/connectivity.rs`): names are
stored once, connections are integer pairs. The table figures come from
`def_bench.py`. A separate old-versus-new probe (one run of each build on the
same file) measured the previous per-string layout at 2.52 s on the synthetic
design (peak RSS 1923 MB) and 0.16 s on cva6 (159 MB), against 1.30 s / 724 MB
and 0.12 s / 91 MB for the flat layout; the 0.01 s differences from the table
are run-to-run noise between the two tools.

A DEF has one COMPONENTS section. A single COMPONENTS section after
NETS/SPECIALNETS is fine (their records are held until it ends); a second one,
once nets were resolved, is an error naming its DEF byte. `cache create`
refuses the same input with the same error.

## Python API

The [Rust core API](ai-api.md#8-rust-core-open-eda-file-tools-core--plain-rust-no-python)
also provides `def::scan_clock_nets_and_instances`, which streams placed
bboxes and reads connections for selected regular clock nets in one pass.
It uses LEF sizes when available and a square four-site marker from the
smallest positive DEF `ROW` step for unknown macros. A usable row step is
required when any placed macro has no LEF size.

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
