# PEX Tools (`pex_tools` / `pex-tools`)

`pex_tools` reads SPEF and DSPF parasitic-exchange files (`.spef`, `.spef.gz`,
`.dspf`, `.dspf.gz`). The Rust parser reads the header and `*NAME_MAP`
sequentially, then parses `*D_NET` and `*R_NET` blocks in parallel with Rayon.
`--threads N` (or `pex_tools.set_threads(N)`) limits the parse to N CPUs; the
default is all logical CPUs.

## Performance Modes

| `--mode` | Memory | Speed | Best for |
| --- | --- | --- | --- |
| `auto` (default) | Whole file + per-net results | Fastest (parallel Rayon across net blocks) | Files that fit comfortably in RAM (e.g. 95 MB `vga_lcd` in 0.39 s, 216 MiB RSS) |
| `stream` | Single net block buffered at a time | ~2× slower, ~5× lower peak RSS | Multi-GB SPEF files or memory-constrained hosts (95 MB `vga_lcd` in 0.87 s, 44 MiB RSS) |

## CLI

```bash
# Header and aggregate parasitics
uv run pex-tools summary --spef design.spef

# Per-net total capacitance histogram (TSV)
uv run pex-tools histogram --spef design.spef --bins 20 --output cap.hist.tsv

# Top-N nets ranked by total capacitance (TSV)
uv run pex-tools nets --spef design.spef --top 20
```

## Python API

```python
import pex_tools

info = pex_tools.header("design.spef")
nets = pex_tools.summaries("design.spef", mode="auto")
hist = pex_tools.histogram("design.spef", bins=20, mode="auto")
parsed = pex_tools.parse("design.spef")

for net in parsed.nets:
    for cap in net.caps:
        print(cap.id, cap.node1, cap.node2, cap.value, cap.ext)
```

## Rust API

The `open_eda_file_tools_core::pex` module accepts plain and gzipped SPEF and
DSPF files. `header` reads the metadata and resolves ports. `summaries`
returns each electrical net’s declared total capacitance, ground and
coupling capacitance, and resistor count and sum in the file’s units.
`parse` also exposes individual connections, capacitors, and resistors
when `keep_entries` is enabled.

```rust
use std::path::Path;
use open_eda_file_tools_core::pex::{self, SummaryMode};

let path = Path::new("design.spef.gz");
let header = pex::header(path)?;
for net in pex::summaries(path, SummaryMode::Stream)? {
    println!("{}: C={} coupling={} R={}", net.name, net.total_cap,
             net.coupling_cap, net.res_total);
}
```

See the [Rust core reference](ai-api.md#8-rust-core-open-eda-file-tools-core--plain-rust-no-python)
for parallel summaries, full parsing, and histograms.

## Supported Syntax & Extensions

- Header: `*SPEF` / `*DSPF`, design, vendor, program, version, divider/delimiter, and `T_UNIT` / `C_UNIT` / `R_UNIT` / `L_UNIT`.
- `*NAME_MAP` and `*PORTS`.
- `*D_NET` and `*R_NET` blocks with `*CONN`, `*CAP` (ground and coupling), `*RES`, `*INDUC`, `*END`.
- DSPF connection attributes (`*C x y`, `*L`, `*S`, `*D cell`), exposing coordinates as `conn.x` and `conn.y`.
- Inline SPICE `$` extension comments preserved verbatim on `cap.ext`, `res.ext`, and `net.ext` (for example `layer=M2 x=1 y=2`).
