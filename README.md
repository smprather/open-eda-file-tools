<p align="center">
  <img src="docs/assets/logo.svg" width="128" alt="open-eda-file-tools logo">
</p>

# open-eda-file-tools

Fast, Rust-backed readers for the files a digital chip design flow produces —
Liberty, LEF, DEF, SDC and SPEF/DSPF — with a Python API and a command-line
tool for each. One package, one version, one install.

| Format | Python package | Commands | Guide |
| --- | --- | --- | --- |
| Liberty `.lib` | `liberty_tools` | `liberty-tools format`, `liberty-format`, `liberty-view` | [docs/liberty.md](docs/liberty.md) |
| Cell LEF `.lef` | `lef_tools` | `lef-tools` | [docs/lef.md](docs/lef.md) |
| DEF `.def` | `def_tools` | `def-tools` | [docs/def.md](docs/def.md) |
| SDC `.sdc` | `sdc_tools` | `sdc-tools` | [docs/sdc.md](docs/sdc.md) |
| SPEF / DSPF `.spef`, `.dspf` | `pex_tools` | `pex-tools` | [docs/pex.md](docs/pex.md) |

Liberty, LEF, DEF and SPEF/DSPF inputs are read plain or gzipped.

**Using this from an AI coding agent?** Read [docs/ai-api.md](docs/ai-api.md)
first. It is a single page with a task-to-entry-point table, every typed
signature, and the invariants that are easy to get wrong.

## Install

```bash
# as a library
python -m pip install "git+https://github.com/smprather/open-eda-file-tools.git"
uv add "open-eda-file-tools @ git+https://github.com/smprather/open-eda-file-tools.git"

# as command-line tools
uv tool install "open-eda-file-tools @ git+https://github.com/smprather/open-eda-file-tools.git"
```

There are no published wheels yet, so the install builds the Rust extension
from source. You need:

- Python 3.14 or newer
- a Rust toolchain (`cargo`) and a C compiler
- `tclsh` 8.6 or newer, only for `sdc-tools` (set `TCLSH` to point at a specific one)

More detail is in [docs/installation.md](docs/installation.md).

## A quick tour

### Liberty

```python
import liberty_tools as lt

doc = lt.parse_file("cells.lib.gz")
arc = doc.cell("and2_x1").pin("y").timing_arcs(related_pin="a")[0]
table = arc.table("cell_rise")

table.lookup(load=0.108, transition=0.0966)   # one value, addressed by axis role
doc.to_polars(cell="and2_x1", table="cell_rise")   # long-form Polars rows
```

Beyond timing tables, the library reads switching energy (`internal_power`),
CCS current waveforms and CCS-noise stages, filters arcs by Boolean `when`
condition, writes edited attributes back without disturbing the rest of the
file, and compares two libraries arc by arc. For large files:

```python
lt.parse_file(path, cells=["and2_x1"])    # skip every other cell while parsing
lt.parse_file(path, skip_tables=True)     # structure only, no table bodies
lt.LibraryIndex.open(path).cell("and2_x1")  # index once, parse cells on demand
```

Two commands work on Liberty files directly:

```bash
liberty-view cells.lib          # browse cells, tables, waveforms and logic symbols in a browser
liberty-tools format cells.lib  # re-indent by brace depth without changing a token
```

### LEF

```python
from lef_tools import parse_lef

macro = parse_lef(["cells.lef"]).macro("sky130_fd_sc_hd__buf_1")
print(macro.width_microns, macro.height_microns)
for pin in macro.pins():
    print(pin.name, pin.direction, pin.use)
```

```bash
lef-tools macros --lef cells.lef
lef-tools show --lef cells.lef --macro sky130_fd_sc_hd__buf_1
```

### DEF

```bash
# every placed instance with its bounding box, in microns
def-tools instances --lef "pdk/*.lef" --def design.def.gz --format parquet --output instances.parquet

# routed shapes of one net on one layer
def-tools net-shapes --def design.def.gz --net VDD --layer met1 -o vdd_met1.csv

# build a binary cache once, then query it repeatedly
def-tools cache create --def design.def.gz -o design.ldefcache
def-tools net-shapes --cache-dir design.ldefcache --net VDD --layer met1 -o vdd_met1.csv
```

```python
from def_tools import instance_bboxes

for bbox in instance_bboxes(["cells.lef"], "design.def.gz"):
    print(bbox.instance, bbox.llx, bbox.lly, bbox.urx, bbox.ury)   # database units
```

### SDC

```bash
sdc-tools clocks --sdc design.sdc
```

```text
clock	period	waveform	sources	generated_from	divide_by	multiply_by
clk	10	0 5	port:clk
```

The SDC is evaluated in a sandboxed Tcl interpreter with the SDC commands
stubbed out, so variables, loops and flow-generated Tcl behave as written.
Commands the tool does not model are recorded and ignored rather than failing.

### SPEF / DSPF

```bash
pex-tools summary --spef design.spef            # units and total / ground / coupling capacitance
pex-tools histogram --spef design.spef --bins 20
pex-tools nets --spef design.spef --top 20      # largest nets by capacitance
```

Net blocks are parsed in parallel. Add `--mode stream` to hold one net block in
memory at a time when the file is larger than you want to load.

### Putting them together

The tools are built to chain. This produces a table of every placed instance
with its bounding box, whether it is sequential, and whether it sits on a clock
tree:

```bash
# clock-tree cells, traced forward from the SDC clock roots through DEF connectivity
sdc-tools clock-cells --sdc design.sdc --def design.def.gz --lib cells.lib --output clock_cells.txt

# instances, with is_seq from Liberty and is_clock from the trace above
def-tools instances --lef "pdk/*.lef" --def design.def.gz \
    --lib cells.lib --clock-cells clock_cells.txt \
    --format parquet --output instances.parquet
```

Clock membership comes from connectivity, never from cell-name patterns.

## Things worth knowing before you trust a number

- **Liberty axis order is per table.** One library puts input transition on
  `index_1`, another puts output load there. Assuming an order does not raise;
  it reads the wrong cell. Ask `table.axis_roles()`, or use
  `table.lookup(load=..., transition=...)`.
- **`internal_power` values are energy.** Multiply by
  `doc.energy_unit_joules()` to get joules.
- **`when="A"` is an implication test, not a string match.** It matches arcs
  conditioned on `A & B` and `A & !B`, and does not match `A | B`.
- **Units differ by entry point.** LEF geometry is always microns. The DEF
  Python iterators return integer database units; `def-tools instances` and the
  instance writers convert to microns.
- **Optional columns are never faked.** `is_seq` needs `--lib` and `is_clock`
  needs `--clock-cells`; asking for either without its input is an error.
- **The Liberty writer only touches what you changed.** Everything else in the
  file, comments and unmodelled groups included, is kept byte for byte.
- **Comparison reports what it cannot compare.** Tables with mismatched units,
  axis roles or shapes are listed separately instead of being diffed.

## Performance

Measured with the release build:

| Operation | Result |
| --- | --- |
| Full parse of a 96.8 MB Liberty library | about 1.5 s, about 150 MB peak RSS |
| `LibraryIndex.open` on the same file | about 0.15 s, cells parsed on demand |
| `skip_tables=True` on the IHP sg13g2 standard-cell library | 23.8 ms → 15.3 ms |

Parsing releases the GIL, so several libraries can be parsed at once from
Python threads. DEF, SPEF and DSPF work is batched and parallelized in Rust.

## Documentation

| Document | Contents |
| --- | --- |
| [docs/ai-api.md](docs/ai-api.md) | Single-page reference for agents: decision table, typed signatures, invariants, recipes |
| [docs/cli.md](docs/cli.md) | Every command and option |
| [docs/liberty.md](docs/liberty.md) | Liberty tables, units, writer, comparison, `when` filtering, viewer |
| [docs/lef.md](docs/lef.md), [docs/def.md](docs/def.md), [docs/sdc.md](docs/sdc.md), [docs/pex.md](docs/pex.md) | Per-tool guides |
| [docs/installation.md](docs/installation.md) | Prerequisites and install options |
| [docs/tools.md](docs/tools.md) | How the project is organized and how to add a tool |
| [BUILD.md](BUILD.md), [VERSIONING_PLAN.md](VERSIONING_PLAN.md) | Building wheels and the release scheme |

Every package ships `py.typed` and `.pyi` stubs.

## Development

```bash
git clone https://github.com/smprather/open-eda-file-tools.git
cd open-eda-file-tools
uv sync --dev
uv run maturin develop --release   # repeat after any Rust change
```

```bash
uv run pytest -q
cargo test
cargo fmt --all --check
uv run ruff check .
uv run ty check
```

Parsing lives in Rust under `src/`; the Python packages at the repository root
are thin wrappers over one PyO3 extension module. No design files are
committed: tests generate their LEF, DEF and SDC inputs with
`tests/gen/design_gen.py`. The large-file Liberty smoke tests run only when
`generic80_ss_125c_1p116v_0p84v.lib` is present in the repository root.

Documentation is part of the change. A new flag, column, function or behavior
updates `docs/ai-api.md`, `docs/cli.md`, the relevant guide and the `.pyi`
stubs in the same commit.

## Versioning and license

Releases use calendar versions (`YYYY.M.D`, or `YYYY.M.D.N` for a second
release on the same day), and every tool is released together under that one
version. Licensed under the [MIT License](LICENSE).
