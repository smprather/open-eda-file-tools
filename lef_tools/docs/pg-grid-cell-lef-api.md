# Public Cell-LEF API for PG Grid Generation

## Status

Implemented in `lef-tools`; this document is the design that was executed when
the cell LEF API was split out of the former `lefdef-tools` project.

Primary downstream consumer: `pg-grid-netlist-gen`.

## Summary

Add a typed public Python API that parses cell LEF libraries and exposes macro
dimensions, pin metadata, and rectangular port geometry. Geometry must remain
in LEF microns and must not depend on DEF database units.

The existing instance-bounding-box API must keep its current behavior. Its DEF
DBU conversion should happen after parsing the common micron-based LEF model.

## Motivation

`pg-grid-netlist-gen` is moving from hand-declared standard-cell YAML data to
LEF + Liberty + SPICE collateral. It needs LEF data for:

- Cell width, height, and origin.
- Pin names, directions, and `USE` classifications.
- Signal-pin access geometry for virtual Manhattan routing.
- Power and ground port geometry for PG-grid attachment.
- Cross-checking LEF pins against Liberty pins and SPICE subcircuit ports.

`lef-tools` already tokenizes LEF efficiently, but its private `MacroDef`
retains only size/origin converted into a caller-supplied DEF DBU. It currently
discards `PIN`, `PORT`, `LAYER`, and geometry statements.

## Goals

1. Parse one or more plain or gzipped LEF files.
2. Optionally materialize only requested macro names.
3. Expose immutable typed objects for libraries, macros, pins, ports, and
   rectangles.
4. Preserve all `RECT` geometry in each selected pin port.
5. Keep public geometry as floating-point microns.
6. Preserve existing duplicate-macro behavior and report duplicates.
7. Release the GIL during file parsing.
8. Keep all existing DEF and instance-bounding-box behavior compatible.

## Non-Goals

- Full technology-LEF routing-rule extraction.
- DEF placement or orientation transforms in this new API.
- Selecting a preferred pin access point.
- Boolean-function or timing-arc parsing; `liberty-tools` owns that domain.
- Polygon/path decomposition for the initial version.
- Parsing OBS geometry for the initial version.

Unsupported geometry may be skipped when other supported `RECT` geometry is
available. A selected pin with port geometry but no supported rectangles must
be distinguishable from a pin with no `PORT` group, so downstream callers can
raise a useful error.

## Proposed Python API

```python
from pathlib import Path

import lef_tools as ldt

library = ldt.parse_lef(
    [Path("lef/sky130_fd_sc_hs__buf_8.lef")],
    macros=["sky130_fd_sc_hs__buf_8"],
)

print(library.macro_names())
macro = library.macro("sky130_fd_sc_hs__buf_8")

print(macro.name)
print(macro.width_microns, macro.height_microns)
print(macro.origin_x_microns, macro.origin_y_microns)

pin = macro.pin("A")
print(pin.name, pin.direction, pin.use)

for port in pin.ports():
    for rect in port.rects():
        print(rect.layer, rect.x0_microns, rect.y0_microns)
```

### Entry Point

```python
def parse_lef(
    paths: str | Path | Sequence[str | Path],
    *,
    macros: Sequence[str] | None = None,
) -> LefLibrary: ...
```

Required behavior:

- Accept one path or a sequence of paths.
- Accept `.lef` and `.lef.gz` using existing input-opening behavior.
- Resolve no paths implicitly; callers own path resolution.
- When `macros` is provided, stream every file but materialize only matching
  `MACRO` bodies.
- Preserve deterministic file order and macro order.
- If multiple files define one macro, last definition wins, matching current
  behavior.
- Record every duplicated macro name once in `duplicate_macro_names()`.
- An empty path list is an error.
- A requested macro absent from all files is not a parse error; callers can
  detect it from `macro_names()` or `macro(name)`.

### `LefLibrary`

```python
class LefLibrary:
    def macro_names(self) -> list[str]: ...
    def macros(self) -> list[LefMacro]: ...
    def macro(self, name: str) -> LefMacro: ...
    def duplicate_macro_names(self) -> list[str]: ...
```

`macro(name)` should raise `KeyError` for an unknown name.

### `LefMacro`

```python
class LefMacro:
    @property
    def name(self) -> str: ...

    @property
    def width_microns(self) -> float: ...

    @property
    def height_microns(self) -> float: ...

    @property
    def origin_x_microns(self) -> float: ...

    @property
    def origin_y_microns(self) -> float: ...

    def pin_names(self) -> list[str]: ...
    def pins(self) -> list[LefPin]: ...
    def pin(self, name: str) -> LefPin: ...
```

Required behavior:

- `SIZE` is required.
- Missing `ORIGIN` defaults to `(0.0, 0.0)`.
- Preserve source pin order.
- Duplicate pin names within a macro are an error.
- `pin(name)` raises `KeyError` for an unknown pin.

### `LefPin`

```python
class LefPin:
    @property
    def name(self) -> str: ...

    @property
    def direction(self) -> str | None: ...

    @property
    def use(self) -> str | None: ...

    def ports(self) -> list[LefPort]: ...
```

Normalize `direction` and `use` to lowercase. Examples:

- `INPUT` -> `"input"`
- `OUTPUT` -> `"output"`
- `INOUT` -> `"inout"`
- `SIGNAL` -> `"signal"`
- `POWER` -> `"power"`
- `GROUND` -> `"ground"`
- `CLOCK` -> `"clock"`

Unknown legal LEF values should remain lowercase strings rather than being
rejected by a narrow enum.

### `LefPort`

```python
class LefPort:
    def rects(self) -> list[LefRect]: ...

    @property
    def has_unsupported_geometry(self) -> bool: ...
```

Preserve source rectangle order across layer changes. `PORT` may contain more
than one `LAYER` statement. Each `RECT` inherits the most recent layer.

`has_unsupported_geometry` is true when the port contains geometry statements
such as `POLYGON`, `PATH`, or `VIA` that this version does not expose.

### `LefRect`

```python
class LefRect:
    @property
    def layer(self) -> str: ...

    @property
    def x0_microns(self) -> float: ...

    @property
    def y0_microns(self) -> float: ...

    @property
    def x1_microns(self) -> float: ...

    @property
    def y1_microns(self) -> float: ...
```

Required behavior:

- Preserve layer spelling from input.
- Normalize coordinate order so `x0 <= x1` and `y0 <= y1`.
- Preserve negative and out-of-macro-bound coordinates. Standard-cell power
  rails commonly extend beyond macro boundaries.
- Reject non-finite numeric coordinates.

## Internal Model and DBU Compatibility

Do not expose the current DBU-scaled private `MacroDef` as the public model.
Recommended internal split:

```text
Parsed LEF model in microns
        |
        +-- public LefLibrary/LefMacro/LefPin/LefPort/LefRect
        |
        +-- existing instance_bboxes path converts macro size/origin to DEF DBU
```

This prevents a cell library from changing meaning based on a downstream DEF's
`UNITS DISTANCE MICRONS` value.

Existing bbox rounding behavior must remain unchanged. If conversion is moved,
continue using the current `round(value * dbu_per_micron)` rule.

## Parser Requirements

Inside a selected `MACRO`, recognize:

```lef
ORIGIN <x> <y> ;
SIZE <width> BY <height> ;
PIN <name>
  DIRECTION <value> ;
  USE <value> ;
  PORT
    LAYER <name> ;
    RECT <x0> <y0> <x1> <y1> ;
  END
END <pin-name>
END <macro-name>
```

Requirements:

- Skip unrelated macro and pin statements without losing nesting alignment.
- Parse more than one `PORT` per pin.
- Parse more than one layer per port.
- Preserve rectangles from every port.
- Handle comments and tokens already supported by the lexer.
- Error on malformed selected macro/pin/port structure with file path and
  useful context.
- Efficiently skip unselected macro bodies without allocating their contents.

## Error Contract

Use `ValueError` for malformed LEF data and `OSError` subclasses for file-open
or decompression failures where practical.

Parse errors must include:

- Input file path.
- Macro name when known.
- Pin name when known.
- Short description of expected/received syntax.

Examples:

```text
cells.lef: MACRO BUF missing SIZE
cells.lef: MACRO BUF PIN A: RECT appears before LAYER
cells.lef: MACRO BUF PIN A: malformed RECT coordinate 'x'
```

## Typing and Packaging

- Export all new symbols from `lef_tools.__init__` and `__all__`.
- Keep `py.typed` in built distributions.
- Add `.pyi` declarations or typed Python wrappers so downstream `ty` can see
  complete signatures and properties.
- Keep Python requirement `>=3.14`.
- No new runtime dependency is required.

## Threading and Performance

- Release the GIL while parsing.
- A few hundred small cell LEFs should parse comfortably below one minute.
- Macro filtering should avoid materializing unrequested cells.
- Do not add cache machinery for this initial API.

## Acceptance Tests

Add small synthetic tests covering all behavior below.

1. Parse macro name, size, and negative origin as micron floats.
2. Parse signal input/output pins and power/ground inout pins.
3. Parse multiple rectangles on one output pin.
4. Parse multiple layers within one port.
5. Parse multiple ports within one pin.
6. Preserve negative power-rail rectangle coordinates.
7. Normalize reversed rectangle coordinates.
8. Report unsupported geometry without dropping supported rectangles.
9. Parse `.lef.gz`.
10. Parse multiple files in input order.
11. Last duplicate macro wins and duplicate name is reported.
12. Macro filtering returns only requested macros.
13. Unknown `library.macro()` and `macro.pin()` raise `KeyError`.
14. Missing `SIZE`, malformed `RECT`, and `RECT` before `LAYER` include path,
    macro, and pin context.
15. Existing `instance_bboxes` tests remain unchanged and pass.
16. Rust tests, Python tests, lint, and type checks pass.

Suggested representative fixture:

```lef
VERSION 5.8 ;
MACRO BUF
  ORIGIN 0 0 ;
  SIZE 5.76 BY 3.33 ;
  PIN A
    DIRECTION INPUT ;
    USE SIGNAL ;
    PORT
      LAYER li1 ;
      RECT 0.125 1.35 1.43 1.78 ;
    END
  END A
  PIN X
    DIRECTION OUTPUT ;
    USE SIGNAL ;
    PORT
      LAYER li1 ;
      RECT 1.96 0.35 2.13 0.96 ;
      RECT 1.96 0.96 5.145 1.13 ;
    END
  END X
  PIN VGND
    DIRECTION INOUT ;
    USE GROUND ;
    PORT
      LAYER met1 ;
      RECT 0 -0.245 5.76 0.245 ;
    END
  END VGND
  PIN VPWR
    DIRECTION INOUT ;
    USE POWER ;
    PORT
      LAYER met1 ;
      RECT 0 3.085 5.76 3.575 ;
    END
  END VPWR
END BUF
END LIBRARY
```

## Downstream Integration Contract

`pg-grid-netlist-gen` will:

- Resolve collateral globs relative to its YAML file.
- Request only cells named by clock, data, and filler configuration.
- Match LEF macros to Liberty cells and SPICE `.subckt` names.
- Use macro dimensions for site-aligned placement.
- Use `direction` and `use` for pin classification.
- Select the closest rectangle pair between connected signal pins for virtual
  Manhattan-route endpoints.
- Reject a selected signal pin that has no usable rectangle.
- Perform orientation transforms and nanometer conversion itself.

No PG-specific routing, arc selection, electrical extraction, or placement
policy should be added to `lef-tools`.
