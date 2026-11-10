# lef-tools

Parse and query cell LEF (Library Exchange Format) files.

`lef-tools` is part of the
[open-eda-file-tools](https://github.com/smprather/open-eda-file-tools)
project and lives in the `lef_tools/` directory. The directory keeps its own
Python package, Rust crate and development virtualenv because its backend is a
separate extension module; it is a library and CLI of the one project, which
shares a single version across all tools.

## Install

```bash
cd lef_tools
uv sync
uv run maturin develop --release
```

## Python API

```python
from lef_tools import parse_lef

library = parse_lef(["sky130_fd_sc_hd__buf_8.lef"], macros=["sky130_fd_sc_hd__buf_8"])
macro = library.macro("sky130_fd_sc_hd__buf_8")

print(macro.width_microns, macro.height_microns)

for pin in macro.pins():
    print(pin.name, pin.direction, pin.use)
    for port in pin.ports():
        for rect in port.rects():
            print(rect.layer, rect.x0_microns, rect.y0_microns,
                  rect.x1_microns, rect.y1_microns)
```

`parse_lef` accepts one path or a sequence of paths, plain `.lef` or gzipped
`.lef.gz`. Every coordinate stays in LEF microns; a DEF's
`UNITS DISTANCE MICRONS` never changes it.

`POLYGON`, `PATH`, and `VIA` geometry is not exposed yet; a port containing it
sets `port.has_unsupported_geometry` without dropping the `RECT`s alongside it.
Rectangle coordinates are normalized so `x0 <= x1` and `y0 <= y1`, but negative
and out-of-macro-boundary values are preserved — standard-cell power rails
routinely extend past the macro boundary.

Malformed LEF raises `ValueError` naming the file, macro, and pin; unreadable
files raise an `OSError` subclass. Unknown names passed to `library.macro()` or
`macro.pin()` raise `KeyError`.

## Command line

```bash
lef-tools macros --lef cells.lef        # TSV of macro dimensions
lef-tools show --lef cells.lef --macro sky130_fd_sc_hd__buf_8
```
