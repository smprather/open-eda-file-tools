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
- Parses Liberty libraries concurrently across threads using `parse_file(..., skip_tables=True)` to obtain each cell's clock role (`def_tools.clock_roles`): pin directions, `ff`/`latch` groups, `clock : true` pins and `clock_gating_integrated_cell`.
- Follows `input -> output` through combinational cells whose input pin sits on a clock net.
- **Integrated clock gates are traced through**, clock pin -> gated-clock output (`clock_gate_clock_pin` / `clock_gate_out_pin`, falling back to the `clock : true` pin or the latch's enable clock). A clock arriving on a gate's enable or test pin does not make it a clock cell.
- Stops at endpoints — flip-flops, latches, and memories or other cells with a `clock : true` pin — which are not marked as clock-tree distribution cells.
- **Prunes to clock pins (default).** A cell is reported only when a clock pin is reachable from it. A generated clock on a divider flop's `Q` usually also fans out into ordinary datapath logic; that cone ends at data pins and is dropped. `--no-prune` / `prune=False` reports every cell the forward trace touches (use it when clocks leave the block through output ports with no clocked sink inside).
- Never relies on macro-name substring heuristics: general buffers (`sg13g2_buf_16`) used on the clock tree are positively identified by connectivity.
