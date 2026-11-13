# DEF Tools (`def_tools` / `def-tools`)

`def_tools` parses and queries DEF (Design Exchange Format) designs (plain
`.def` and `.def.gz`) through the shared Rust backend.

- Python iterators (`instance_bboxes`, `net_shapes`) return integer **DEF database units (DBU)**.
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
| `is_clock` | `--clock-cells PATH` | `true` when the instance is in the clock-tree instance list (produced by `sdc-tools clock-cells`) |

Requesting `is_seq` or `is_clock` in `--columns` without supplying its backing
input is an error, never a silent `false` column.

## `def-tools net-shapes` and Binary Cache

```bash
# Direct streaming query
uv run def-tools net-shapes --def design.def.gz --net VDD --layer met1 -o vdd_met1.csv

# Indexed binary cache for repeated queries on large designs
uv run def-tools cache create --def design.def.gz -o design.ldefcache
uv run def-tools net-shapes --cache-dir design.ldefcache --net VDD --layer met1 -o vdd_met1.csv
```

## Python API

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
