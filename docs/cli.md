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

While running, `liberty-view` serves a JSON API for scripting against a large
library without re-parsing:

- `GET /api/meta` — library name, units, cell count, templates.
- `GET /api/cells?filter=<substr>&offset=0&limit=200` — filtered cell list.
- `GET /api/cell/{cell_name}` — pins, buses, bundles, arcs, and table metadata.
- `GET /api/cell/{cell_name}/source` — raw Liberty source of that cell.
- `GET /api/cell/{cell_name}/pin/{pin_name}/table/{arc_idx}/{table_name}` — full table values.

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
  [--lib stdcell.lib ...] \
  [--clock-cells clock_cells.txt] \
  [--format csv|parquet|table] \
  [--columns SPEC] \
  [--batch-size 131072] \
  [--max-memory 1GiB] \
  [--threads N] \
  [--output PATH]
```

- `--lef` accepts multiple files or glob patterns after a single `--lef` flag or repeated `--lef` flags.
- Coordinates (`llx, lly, urx, ury`) are in **microns** (`DBU / UNITS DISTANCE MICRONS`).
- Default columns are `instance,base_cell,llx,lly,urx,ury`, plus `is_seq` when `--lib` is given and `is_clock` when `--clock-cells` is given.
- `--columns` selects and renames columns (`instance=inst_name,base_cell=cell,status,orient,is_seq,is_clock,llx=llx_um`). Requesting `is_seq` without `--lib` or `is_clock` without `--clock-cells` is an error.

### `def-tools net-shapes`, `cache create`, `clock-instances`, `configure-io`

```bash
def-tools net-shapes --def design.def.gz --net VDD --layer met1 -o vdd_met1.csv
def-tools cache create --def design.def.gz -o design.ldefcache
def-tools net-shapes --cache-dir design.ldefcache --net VDD --layer met1 -o vdd_met1.csv
def-tools configure-io --target /scratch/$USER -o def-tools-io-config.toml
```

---

## `sdc-tools`

Requires `tclsh` >= 8.6 on `PATH` (or `TCLSH=/path/to/tclsh`).

```bash
sdc-tools clocks --sdc design.sdc [--format tsv|json] [--output PATH]
sdc-tools clock-cells --sdc design.sdc --def design.def.gz --lib stdcell.lib [--format lines|json] [--output PATH]
```

- `clocks` lists created and generated clocks (`clock  period  waveform  sources  generated_from  divide_by  multiply_by`).
- `clock-cells` traces forward from clock roots through DEF connectivity using Liberty pin directions and `ff`/`latch` groups, emitting the clock-tree distribution cells for `def-tools instances --clock-cells`.

---

## `pex-tools`

```bash
pex-tools summary --spef design.spef [--mode auto|stream]
pex-tools histogram --spef design.spef [--bins 20] [--mode auto|stream] [--output PATH]
pex-tools nets --spef design.spef [--top 20] [--mode auto|stream] [--output PATH]
```

- `--mode auto` parses `*D_NET`/`*R_NET` blocks in parallel with Rayon; `--mode stream` buffers one net block at a time for a low-memory pass.
