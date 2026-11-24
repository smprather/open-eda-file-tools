# SDC Tools (`sdc_tools` / `sdc-tools`)

`sdc_tools` evaluates plain or gzip-compressed Synopsys Design Constraints
(`.sdc` or `.sdc.gz`) in a sandboxed real Tcl interpreter. It uses system
`tclsh` 8.6 or newer, with an optional `TCLSH=/path/to/tclsh` override.
The reader detects compression by its gzip signature. The interpreter runs
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
- Parses Liberty libraries concurrently across threads (at most `--threads N`, default all CPUs) using `parse_file(..., skip_tables=True)` to obtain each cell's clock role (`def_tools.clock_roles`): pin directions, `ff`/`latch` groups, `clock : true` pins and `clock_gating_integrated_cell`.
- Follows `input -> output` through combinational cells whose input pin sits on a clock net.
- **Integrated clock gates are traced through**, clock pin -> gated-clock output (`clock_gate_clock_pin` / `clock_gate_out_pin`, falling back to the `clock : true` pin or the latch's enable clock). A clock arriving on a gate's enable or test pin does not make it a clock cell.
- Stops at the clock inputs of endpoints such as flip-flops, latches, and memories. An endpoint is included when its output drives a clock net, as with the `Q` pin of a clock divider.
- **Prunes to clock pins (default).** Keeps paths to clock pins and sequential drivers on explicit clock roots. A generated clock often also fans out into ordinary datapath logic. The pruned trace omits paths that reach only data pins. Use `--no-prune` or `prune=False` to keep the forward trace when clocks leave the block through output ports.
- Never relies on macro-name substring heuristics: general buffers (`sg13g2_buf_16`) used on the clock tree are positively identified by connectivity.

Rust applications can call `sdc::clocks` and pass its symbolic sources
to `def::clock_tree_cells`. This uses the same Tcl evaluator and requires
a system `tclsh` but no Python runtime. [Rust API example](ai-api.md#8-rust-core-open-eda-file-tools-core--plain-rust-no-python)
