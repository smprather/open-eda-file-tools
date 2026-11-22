# Command Line Reference

`open-eda-file-tools` installs six command-line entry points from a single
package and version:

| Command | Purpose |
| --- | --- |
| `liberty-tools format` (`liberty-format`) | Reformat a Liberty (`.lib` / `.lib.gz`) file by brace depth, provably without changing its token stream. |
| `liberty-view` | Serve the local browser viewer (and HTTP JSON API) for a Liberty library. |
| `lef-tools` | Query cell LEF macro dimensions, pins, ports, and rectangles (in microns). |
| `def-tools` | Dump placed instance bboxes (CSV/Parquet/table in microns), query routed net shapes, build binary DEF caches, and benchmark storage IO. |
| `sdc-tools` | Extract SDC clocks via a sandboxed Tcl >= 8.6 interpreter and trace clock-tree cells through DEF connectivity. |
| `pex-tools` | Summarize SPEF/DSPF parasitics, compute capacitance histograms, and rank nets by capacitance. |

Every command supports `-h` / `--help` and `--version`.

---

## `liberty-tools format` (and `liberty-format`)

```text
Usage: liberty-tools format [OPTIONS] FILE
       liberty-format [OPTIONS] FILE
```

| Option | Default | Meaning |
| --- | --- | --- |
| `-o, --output PATH` | stdout | Write formatted output to `PATH`. |
| `-i, --in-place` | off | Rewrite `FILE` in place (not supported for `.gz`). |
| `--check` | off | Exit 0 if already formatted, 1 if it would reformat. |
| `--indent INTEGER` | `2` | Spaces per `{ }` nesting level. |


Both entry points stream the input into temporary files, reindent by line,
and verify the two token streams incrementally before writing any output.
Gzip input is decompressed for verification. An output path receives plain
UTF-8 text even if it ends in `.gz`.

---

## `liberty-view`

```text
Usage: liberty-view [OPTIONS] [LIBERTY_FILE]
```

| Option | Default | Meaning |
| --- | --- | --- |
| `-p, --port INTEGER` | `8000` | First TCP port to try; scans forward if busy. |
| `--host TEXT` | `127.0.0.1` | Bind address. |
| `--open-browser / --no-open-browser` | `--open-browser` | Open a browser tab when ready. |
| `--exit-on-close / --no-exit-on-close` | `--exit-on-close` | Exit shortly after the browser tab closes. |

The viewer opens one index per process and parses cells on demand. Its HTTP
endpoints include the following.

- `GET /api/ready` warms the index and returns an empty HTTP 204 response.
- `GET /api/meta` returns library metadata, templates, and driver waveforms.
- `GET /api/cells?filter=<substr>&offset=0&limit=200` returns a filtered cell page.
- `GET /api/cells/{cell}` returns pins, buses, bundles, arcs, and table metadata.
- `GET /api/cells/{cell}/source` returns cell source up to one million characters,
  with the original character count.
- `GET /api/table?cell=...&pin=...&group=...&arc_index=...&table=...` returns full table
  values. Include `container=bus:<name>` or `container=bundle:<name>` for members.


---

## `lef-tools`

```bash
lef-tools macros --lef cells.lef [--macro NAME ...]
lef-tools show --lef cells.lef --macro sky130_fd_sc_hd__buf_8
```

- `macros` emits TSV: `macro  width_um  height_um  origin_x_um  origin_y_um`.
- `show` prints one macro's dimensions, pins, ports, and layer rectangles in microns.

---

## `def-tools`

### `def-tools instances`

```bash
def-tools instances \
  --lef "pdk/*.lef" \
  --def design.def.gz \
  [--block-def MACRO=PATH ...] \
  [--lib stdcell.lib ...] \
  [--clock-cells clock_cells.txt | --clock-data [BACKEND:]PATH | --twf PATH] \
  [--format csv|parquet|table] \
  [--columns SPEC] \
  [--batch-size 131072] \
  [--max-memory 1GiB] \
  [--threads N] \
  [--output PATH]
```

- `--lef` accepts multiple files or glob patterns after a single `--lef` flag or repeated `--lef` flags.
- `--block-def MACRO=PATH` expands a placed macro using a child DEF. Repeat to link nested blocks. The dump includes block rows and descendant paths with top-level coordinates. See the [hierarchical DEF example](def.md#hierarchical-def-instances).
- Coordinates (`llx, lly, urx, ury`) are in **microns** (`DBU / UNITS DISTANCE MICRONS`).
- Default columns are `instance,base_cell,llx,lly,urx,ury`, plus `is_seq` when `--lib` is given and `is_clock` when `--clock-cells`, `--clock-data` or `--twf` is given.
- `--columns` selects and renames columns (`instance=inst_name,base_cell=cell,status,orient,is_seq,is_clock,llx=llx_um`). Requesting `is_seq` without `--lib` or `is_clock` without a clock input is an error.
- `--clock-cells`, `--clock-data` and `--twf` are alternatives. `--clock-data [BACKEND:]PATH` reads a clock/data source through a backend: `otd` (built in, [otd.md](otd.md); inferred from `.otd`/`.otd.gz`), `names` (an instance list), or a plug-in. Net/pin sources need `--lib`. `--twf PATH` is shorthand for `--clock-data twf:PATH`; vendor TWF is **not** read by this project and needs a plug-in `twf` backend.
- `--threads N` caps CPU use at N; the default is all logical CPUs. It covers every step of the command, Liberty parsing and clock-data resolution included. The same option exists on every `def-tools`, `sdc-tools clock-cells` and `pex-tools` command that runs in parallel; output never depends on it.
- Exit codes: `3` the requested backend is not installed, `4` the source is malformed, `1` any other error. An `is_clock` column is never written when its source could not be read. Unmatched or unclassifiable clock objects are warned about on stderr.

### `def-tools clock-data`

```bash
def-tools clock-data --source [BACKEND:]PATH [--report objects|windows|instances] \
  [--def design.def.gz] [--lib stdcell.lib ...] [--threads N] [--output PATH]
```

- `objects` (default): CSV `kind,instance,name,role` for every net, pin and instance the source describes.
- `windows`: every OTD timing window as CSV (`kind,instance,name,role,object_attrs,clock,edge,window_role,rise_early,rise_late,fall_early,fall_late,rise_slew_min,rise_slew_max,fall_slew_min,fall_slew_max,rise_res,rise_slack,fall_res,fall_slack,window_attrs`).
- `instances`: the resolved clock-tree instance names, one per line (needs `--def`; net/pin sources need `--lib`). Same format as `sdc-tools clock-cells`, so the two can be diffed.

### `def-tools net-shapes`, `cache create`, `clock-instances`, `configure-io`

```bash
def-tools clock-instances --sdc design.sdc --def design.def.gz -o clock_instances.csv [--threads N]   # legacy, see below
def-tools net-shapes --def design.def.gz --net VDD --layer met1 -o vdd_met1.csv [--threads N]
def-tools cache create --def design.def.gz -o design.ldefcache [--threads N]
def-tools net-shapes --cache-dir design.ldefcache --net VDD --layer met1 -o vdd_met1.csv
def-tools configure-io --target /scratch/$USER -o def-tools-io-config.toml
```

- `clock-instances` is the older report: it follows nets from the SDC clock roots through cells whose **macro name** contains `--clock-cell-pattern` (default `clk`). It needs `tclsh` (roots come from `sdc_tools`). For `is_clock`, prefer `sdc-tools clock-cells` or `--clock-data`, which use connectivity and Liberty, not names.
- `net-shapes` writes one CSV row per routed shape of `--net` on `--layer`:
  `net,section,route_status,shape_type,layer,width,x1,y1,x2,y2,llx,lly,urx,ury`.
  Coordinates are DEF database units (integer DBU). Wire segments carry their
  center-line endpoints in `x1,y1,x2,y2` and a bbox that includes width and
  wire-extension padding; `shape_type` is the special-wire `+ SHAPE` value
  (empty without one). `RECT` patches (both the
  in-path `RECT ( dx1 dy1 dx2 dy2 )` form and the net-level `+ RECT layer pt
  pt` form) carry their normalized lower-left/upper-right corners in
  `x1,y1,x2,y2`, `shape_type` = `RECT`, `width` = 0. The net-level form takes
  its `route_status` from a bare status right before it (DEF 5.8:
  `+ FIXED + RECT ...`, also `+ COVER`, `+ ROUTED`, `+ SHIELD net`), and is
  `ROUTED` otherwise. Regular-net wire `width` is always 0 — the width comes
  from technology LEF, which net-shapes does not read.
- Route pieces the reader cannot place on a layer are skipped and counted:
  segments after a via (the via's other layer needs via definitions, which
  net-shapes does not read) as `after_via`, `+ POLYGON` patches on the queried
  layer as `polygon`, and each `+ SUBNET` that carries wiring as `subnet`. When any count is
  non-zero the CLI prints one warning line to stderr:
  `warning: net-shapes skipped route pieces: after_via=N polygon=N subnet=N`.
  The Python writers return the same counts as `{"after_via", "polygon",
  "subnet"}`, and the `def_tools.net_shapes` iterator exposes them via
  `.skipped`. Cache queries return identical rows and counts to direct scans.

---

## `sdc-tools`

Requires `tclsh` >= 8.6 on `PATH` (or `TCLSH=/path/to/tclsh`).

```bash
sdc-tools clocks --sdc design.sdc [--format tsv|json] [--output PATH]
sdc-tools clock-cells --sdc design.sdc --def design.def.gz --lib stdcell.lib [--no-prune] [--format lines|json] [--threads N] [--output PATH]
```

- `clocks` lists created and generated clocks (`clock  period  waveform  sources  generated_from  divide_by  multiply_by`).
- `clock-cells` traces forward from clock roots through DEF connectivity using Liberty pin directions, `ff`/`latch` groups, `clock : true` pins and `clock_gating_integrated_cell`, emitting the clock-tree distribution cells for `def-tools instances --clock-cells`. Integrated clock gates are traced through clock pin -> gated clock. By default only cells from which a clock pin is reachable are reported; `--no-prune` reports every cell the trace touches.

---

## `pex-tools`

```bash
pex-tools summary --spef design.spef [--mode auto|stream] [--threads N]
pex-tools histogram --spef design.spef [--bins 20] [--mode auto|stream] [--threads N] [--output PATH]
pex-tools nets --spef design.spef [--top 20] [--mode auto|stream] [--threads N] [--output PATH]
```

- `--mode auto` parses `*D_NET`/`*R_NET` blocks in parallel with Rayon; `--mode stream` buffers one net block at a time for a low-memory pass.
