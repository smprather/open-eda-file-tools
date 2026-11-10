# def-tools

Parse and query DEF (Design Exchange Format) files.

`def-tools` is part of the
[open-eda-file-tools](https://github.com/smprather/open-eda-file-tools)
project and lives in the `def_tools/` directory. The directory keeps its own
Python package, Rust crate and development virtualenv because its backend is a
separate extension module; it is a library and CLI of the one project, which
shares a single version across all tools. Cell LEF parsing lives in
`lef-tools`; the DEF backend links it in Rust for macro sizes, so the Python
packages stay independent.

## Install

```bash
cd def_tools
uv sync
uv run maturin develop --release
```

## Offline / read-only runs

`uv run` is the development route: it writes `.venv`, the uv cache and the
cargo target directory, and installs the package editable. For a read-only,
air-gapped checkout, build once with cargo and assemble the package in the
directory you run from:

```bash
mkdir -p /tmp/run && cd /tmp/run
/path/to/open-eda-file-tools/def_tools/scripts/run_example_offline.sh \
  dump_instances.py --lef design.lef --def design.def.gz --output instances.parquet
```

The helper keeps `CARGO_HOME`, `CARGO_TARGET_DIR` and the assembled `py/`
package under `$PWD`, builds with `--offline --locked`, and disables bytecode
writes. It also repairs the `zstd-sys` build-script `OUT_DIR`, whose read-only
header copies break a second build that reuses the target directory (see
`BUILD.md`). Interpreter selection is pinned: the helper reads
`.python-version`, prefers the loadout `def-tools` venv interpreter, then
`python3.14`/`python3.15`, then `python3`, and exports `PYO3_PYTHON` so the
extension is built for the interpreter that runs it. `PYTHON=/path/to/python3.14`
overrides the search, and a too-old interpreter is an error rather than a
fallback. Once the package is installed, `def-dump-instances` is the
PATH-proof equivalent: its console-script shebang hard-references the owning
interpreter.

`uv run --no-editable` is the minimal fix when the tree is read-only but the uv
cache is warm. Wheels are not published, so air-gapped machines still need the
vendored checkout and a Rust toolchain.

For the complete offline runbook — Engineering Loadout's `rust`/`portable-python`
supply the toolchain, and the rebuild/verify checklist plus the AlmaLinux 8
Docker proof are in `BUILD.md` — see
[Offline builds](../BUILD.md#offline-builds).

## Instance Bounding Boxes

Write placed instance bounding boxes as CSV:

```bash
uv run def-tools instances --lef sky130.lib.lef --def design.def.gz --output instances.csv
```

Plain `.def` and gzipped `.def.gz` inputs are supported. CSV columns are
`instance,macro,status,orient,llx,lly,urx,ury`. Coordinates are DEF database
units.

For Parquet (default), an aligned table, or renamed columns, use the
`def-dump-instances` console script. It is installed with the package and its
shebang hard-references the owning interpreter, so the `python3` on `PATH` is
irrelevant:

```bash
def-dump-instances --lef sky130.lib.lef --def design.def.gz --output instances.parquet
def-dump-instances --format table --lef sky130.lib.lef --def design.def.gz --output instances.table
def-dump-instances --format csv --columns instance=instance_name,llx=llx_um \
    --lef sky130.lib.lef --def design.def.gz --output instances.csv
```

`python -m def_tools.dump_instances` and `examples/dump_instances.py` expose the
same options; the example refuses to run under an interpreter older than 3.14 or
one without the package, instead of failing with an import traceback.

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

All three take an optional `columns` list of `(source, output_name)` pairs in
output order; the default is the six canonical columns. Source names are
`instance`, `base_cell`, `llx`, `lly`, `urx`, `ury`; renaming is just the second
half of the pair:

```python
columns = [("instance", "instance_name"), ("base_cell", "cell"), ("llx", "llx_um")]
write_instance_table(["sky130.lib.lef"], "design.def.gz", "instances.table", columns=columns)
```

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
