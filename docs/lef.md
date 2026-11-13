# LEF Tools (`lef_tools` / `lef-tools`)

`lef_tools` parses cell LEF (Library Exchange Format) libraries into typed
macros, pins, ports, and rectangles through the shared Rust backend. All
geometry stays in **LEF microns**; a DEF's `UNITS DISTANCE MICRONS` never
modifies it. Plain `.lef` and gzipped `.lef.gz` inputs are supported.

## Python API

`parse_lef` accepts a single path or a sequence of paths. Passing `macros=`
streams each file and materializes only the named `MACRO` bodies. When multiple
files define the same macro, the last definition wins and every duplicated name
is reported once by `library.duplicate_macro_names()`.

```python
from lef_tools import parse_lef

library = parse_lef(["sky130_fd_sc_hd__buf_8.lef"], macros=["sky130_fd_sc_hd__buf_8"])
macro = library.macro("sky130_fd_sc_hd__buf_8")

print(macro.width_microns, macro.height_microns)
print(macro.origin_x_microns, macro.origin_y_microns)

for pin in macro.pins():
    print(pin.name, pin.direction, pin.use)
    for port in pin.ports():
        print(port.has_unsupported_geometry)
        for rect in port.rects():
            print(rect.layer, rect.x0_microns, rect.y0_microns, rect.x1_microns, rect.y1_microns)
```

## CLI

```bash
uv run lef-tools macros --lef cells.lef
uv run lef-tools show --lef cells.lef --macro sky130_fd_sc_hd__buf_8
```

## Key Invariants

- **Coordinates are always in LEF microns.**
- **Unsupported port geometry (`POLYGON`, `PATH`, `VIA`) sets `port.has_unsupported_geometry = True`** while still returning all `RECT` shapes in the port.
- **Rectangles are normalized** so `x0 <= x1` and `y0 <= y1`, preserving negative and out-of-boundary coordinates (power rails routinely extend past the macro boundary).
- **`PROPERTYDEFINITIONS` sections are skipped** so property-type declarations (`MACRO <prop> STRING ;`) are never mistaken for macro definitions.
