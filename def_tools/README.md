# def-tools

Parse and query DEF (Design Exchange Format) files.

`def-tools` is part of the
[open-eda-file-tools](https://github.com/smprather/open-eda-file-tools)
repository and lives in the `def_tools/` subdirectory. It keeps its own
distribution, version, Rust crate and virtualenv. It is split out of the
former `lefdef-tools` project. Cell LEF parsing lives in `lef-tools`; the DEF
backend links it in Rust for macro sizes, so the Python packages stay
independent.

## Install

```bash
cd def_tools
uv sync
uv run maturin develop --release
```

## Instance Bounding Boxes

Write placed instance bounding boxes as CSV:

```bash
uv run def-tools instances --lef sky130.lib.lef --def design.def.gz --output instances.csv
```

Plain `.def` and gzipped `.def.gz` inputs are supported. CSV columns are
`instance,macro,status,orient,llx,lly,urx,ury`. Coordinates are DEF database
units.

Python API:

```python
from def_tools import instance_bboxes

for bbox in instance_bboxes(["sky130.lib.lef"], "design.def"):
    print(bbox.instance, bbox.llx, bbox.lly, bbox.urx, bbox.ury)
```

## Aligned Instance Table

`write_instance_table(lefs, def_path, output)` writes the same rows as an
aligned text table with micron coordinates and returns the number of rows:

```python
from def_tools import write_instance_table

rows = write_instance_table(["sky130.lib.lef"], "design.def.gz", "instances.table")
print(rows)
```

The native writer sizes the name columns with a components-only pass, then
streams the rows without loading the design into memory.
`write_instance_bbox_csv(lefs, def_path, output)` writes the same rows as CSV
and returns the row count.
`write_instance_parquet(lefs, def_path, output, batch_size)` writes
zstd-compressed Parquet and returns `(rows, row_groups)`.

## Example

`examples/dump_instances.py` dumps a DEF's placed instances as Parquet
(default), CSV or an aligned table.

## Net Shape Query

Write routed shape bounding boxes for one net and layer:

```bash
uv run def-tools net-shapes --def design.def.gz --net VDD --layer met1 --output vdd_met1.csv
```

CSV columns are
`net,section,route_status,shape_type,layer,width,x1,y1,x2,y2,llx,lly,urx,ury`.
Path segments are emitted as rectangular DBU bounding boxes using the DEF
route width.

## Binary DEF Cache

For repeated queries on huge `.def.gz` files, convert once into an indexed
binary cache directory:

```bash
uv run def-tools cache create --def design.def.gz --output design.ldefcache
uv run def-tools net-shapes --cache-dir design.ldefcache --net VDD --layer met1 --output vdd_met1.csv
```

The cache stores components, raw net records, a fixed-offset net record index,
a net-name index, and parsed connectivity in separate files. This lets
single-net shape queries read only matching records and lets clock queries skip
DEF tokenization.

## Clock Tree Query

Extract clock roots from SDC `create_clock` / `create_generated_clock` commands
and walk DEF connectivity through cells whose macro names contain `clk` by
default:

```bash
uv run def-tools clock-instances --sdc base.sdc --cache-dir design.ldefcache --output clocks.csv
```

Use `--clock-cell-pattern <text>` to add different clock-buffer naming
patterns.

## IO Configuration

Benchmark storage tiers and generate a config for automatic staging/cache
policy:

```bash
uv run def-tools configure-io \
  --target /scratch/$USER \
  --target /dev/shm \
  --target /project/tmp \
  --output def-tools-io-config.toml
```

The configurator writes sequential read/write measurements, filesystem type,
free space, ranked scratch directories, RAM directories, and cache defaults. It
uses Flexible I/O Tester (`fio`) when available and valid, otherwise a built-in
sequential benchmark.
