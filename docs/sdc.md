# SDC Tools (`sdc_tools` / `sdc-tools`)

`sdc_tools` evaluates Synopsys Design Constraints (`.sdc`) files in a sandboxed
real Tcl interpreter (system `tclsh` >= 8.6; `TCLSH=/path/to/tclsh` overrides)
with stubbed SDC commands. Variables, `[command]` substitution, `foreach`,
`lmap`, `if`, and flow-generated Tcl work as authored, while unsupported
constraints are recorded and return `{}` instead of aborting.

## CLI

```bash
# List created and generated clocks (TSV or --format json)
uv run sdc-tools clocks --sdc design.sdc

# Forward-trace clock-tree distribution cells through DEF connectivity
uv run sdc-tools clock-cells \
  --sdc design.sdc \
  --def design.def.gz \
  --lib stdcell.lib \
  --output clock_cells.txt
```

## Python API

```python
import sdc_tools

for clk in sdc_tools.clocks("design.sdc"):
    print(clk.name, clk.period, clk.waveform, clk.sources, clk.generated_from)

trace = sdc_tools.clock_cells("design.def.gz", [Path("stdcell.lib")], "design.sdc")
print(trace.cells)   # tuple of clock-tree instance names
```

## Forward Clock-Tree Trace Semantics

- Seeds nets from the symbolic sources (`port:clk`, `pin:div/Q`, `net:clk`, `cell:u1`) of every clock in the SDC.
- Parses Liberty libraries concurrently across threads using `parse_file(..., skip_tables=True)` to obtain pin directions and `ff`/`latch` sequential status.
- Follows `input -> output` through combinational cells whose input pin sits on a clock net.
- Stops at sequential cells (`ff`/`latch` endpoints), which are not marked as clock-tree distribution cells.
- Never relies on macro-name substring heuristics: general buffers (`sg13g2_buf_16`) used on the clock tree are positively identified by connectivity.
