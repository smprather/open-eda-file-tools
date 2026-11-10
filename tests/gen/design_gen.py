"""Deterministic synthetic LEF/DEF generator.

The shape of the output is modelled on a real post-fill DEF from an
OpenROAD sky130hd flow (``swerv_wrapper``: 2.59M instances, 4.29M lines,
577 MB uncompressed / 51 MB gzipped).  That file is *not* in the repo --
see ``docs/test-collateral.md`` for its provenance and the measured
statistics reproduced here.

Design constraints:

* **Deterministic.**  ``seed`` fully determines the output bytes.
* **O(1) memory.**  Nothing is accumulated; every section is produced by
  re-walking the same seeded placement stream, so a 10M-instance design
  costs no more RAM than a 10-instance one.
* **Self-consistent.**  The LEF, the DEF and the manifest are emitted
  together and always agree.
* **Ground truth included.**  ``generate`` returns a manifest of what it
  actually wrote.  Tests assert against the manifest rather than
  re-deriving expectations, so a generator bug cannot silently agree
  with a parser bug.
"""

from __future__ import annotations

import gzip
import itertools
import json
import random
from collections import Counter, deque
from collections.abc import Iterator
from dataclasses import asdict, dataclass, field
from pathlib import Path
from typing import IO

DBU = 1000
SITE_NAME = "unithd"
SITE_W = 460  # dbu; sky130hd unithd site width
SITE_H = 2720  # dbu; row pitch

# Routing layers, bottom-up, with the track pitch each uses.
LAYERS = (("li1", 460), ("met1", 340), ("met2", 460), ("met3", 680), ("met4", 920), ("met5", 3400))


@dataclass(frozen=True)
class Cell:
    """A library cell the generator can place."""

    name: str
    sites: int
    weight: int  # relative frequency, taken from the reference design
    kind: str  # fill | tap | diode | logic
    cls: str
    inputs: tuple[str, ...] = ()
    output: str | None = None
    pg: tuple[str, ...] = ("VPWR", "VGND", "VPB", "VNB")

    @property
    def width(self) -> int:
        return self.sites * SITE_W


# Cell mix measured from the reference design.  85% of instances are
# fill/decap/tap: that ratio is what makes a real post-fill DEF enormous
# while staying trivially compressible, and it is the property most worth
# reproducing for parser and cache benchmarks.
CELLS: tuple[Cell, ...] = (
    Cell("sky130_ef_sc_hd__decap_12", 12, 1184935, "fill", "CORE SPACER"),
    Cell("sky130_fd_sc_hd__tapvpwrvgnd_1", 1, 335013, "tap", "CORE WELLTAP", pg=("VPWR", "VGND")),
    Cell("sky130_fd_sc_hd__fill_1", 1, 332534, "fill", "CORE SPACER"),
    Cell("sky130_fd_sc_hd__decap_6", 6, 263631, "fill", "CORE SPACER"),
    Cell("sky130_fd_sc_hd__fill_2", 2, 94248, "fill", "CORE SPACER"),
    Cell("sky130_fd_sc_hd__decap_8", 8, 69044, "fill", "CORE SPACER"),
    Cell("sky130_fd_sc_hd__nand2_1", 3, 64827, "logic", "CORE", ("A", "B"), "Y"),
    Cell("sky130_fd_sc_hd__decap_3", 3, 51607, "fill", "CORE SPACER"),
    Cell("sky130_fd_sc_hd__decap_4", 4, 44830, "fill", "CORE SPACER"),
    Cell("sky130_fd_sc_hd__diode_2", 2, 24431, "diode", "CORE ANTENNACELL", ("DIODE",)),
    Cell("sky130_fd_sc_hd__nand3_1", 4, 11463, "logic", "CORE", ("A", "B", "C"), "Y"),
    Cell("sky130_fd_sc_hd__inv_2", 3, 11338, "logic", "CORE", ("A",), "Y"),
    Cell("sky130_fd_sc_hd__nor2_1", 3, 8223, "logic", "CORE", ("A", "B"), "Y"),
    Cell("sky130_fd_sc_hd__dfrtp_1", 20, 7645, "logic", "CORE", ("CLK", "D", "RESET_B"), "Q"),
    Cell("sky130_fd_sc_hd__clkbuf_2", 4, 6069, "logic", "CORE", ("A",), "X"),
    Cell("sky130_fd_sc_hd__buf_6", 9, 5800, "logic", "CORE", ("A",), "X"),
    Cell("sky130_fd_sc_hd__mux2_1", 9, 5356, "logic", "CORE", ("A0", "A1", "S"), "X"),
    Cell("sky130_fd_sc_hd__buf_2", 4, 3674, "logic", "CORE", ("A",), "X"),
    Cell("sky130_fd_sc_hd__a22o_1", 7, 3445, "logic", "CORE", ("A1", "A2", "B1", "B2"), "X"),
    Cell("sky130_fd_sc_hd__or2_1", 5, 2735, "logic", "CORE", ("A", "B"), "X"),
    Cell("sky130_fd_sc_hd__o21ai_1", 4, 2526, "logic", "CORE", ("A1", "A2", "B1"), "Y"),
    # The reference design uses 229 distinct macros; the 208 beyond the
    # head of the histogram are almost all logic, and dropping them would
    # under-weight the NETS section.  These 20 carry that tail's combined
    # instance count, keeping the logic fraction at the measured 7.1%.
    Cell("sky130_fd_sc_hd__a21oi_1", 4, 7200, "logic", "CORE", ("A1", "A2", "B1"), "Y"),
    Cell("sky130_fd_sc_hd__dfxtp_1", 16, 4900, "logic", "CORE", ("CLK", "D"), "Q"),
    Cell("sky130_fd_sc_hd__inv_1", 3, 4400, "logic", "CORE", ("A",), "Y"),
    Cell("sky130_fd_sc_hd__o21a_1", 6, 3900, "logic", "CORE", ("A1", "A2", "B1"), "X"),
    Cell("sky130_fd_sc_hd__nor3_1", 4, 3500, "logic", "CORE", ("A", "B", "C"), "Y"),
    Cell("sky130_fd_sc_hd__a21o_1", 6, 3100, "logic", "CORE", ("A1", "A2", "B1"), "X"),
    Cell("sky130_fd_sc_hd__nand2_2", 5, 2893, "logic", "CORE", ("A", "B"), "Y"),
    Cell("sky130_fd_sc_hd__and2_0", 5, 2800, "logic", "CORE", ("A", "B"), "X"),
    Cell("sky130_fd_sc_hd__clkbuf_1", 3, 2547, "logic", "CORE", ("A",), "X"),
    Cell("sky130_fd_sc_hd__xor2_1", 7, 2500, "logic", "CORE", ("A", "B"), "X"),
    Cell("sky130_fd_sc_hd__buf_4", 6, 2388, "logic", "CORE", ("A",), "X"),
    Cell("sky130_fd_sc_hd__xnor2_1", 7, 2200, "logic", "CORE", ("A", "B"), "Y"),
    Cell("sky130_fd_sc_hd__clkbuf_4", 6, 1900, "logic", "CORE", ("A",), "X"),
    Cell("sky130_fd_sc_hd__o22ai_1", 5, 1700, "logic", "CORE", ("A1", "A2", "B1", "B2"), "Y"),
    Cell("sky130_fd_sc_hd__a211oi_1", 6, 1500, "logic", "CORE", ("A1", "A2", "B1", "C1"), "Y"),
    Cell("sky130_fd_sc_hd__or3_1", 5, 1300, "logic", "CORE", ("A", "B", "C"), "X"),
    Cell("sky130_fd_sc_hd__mux2i_1", 8, 1150, "logic", "CORE", ("A0", "A1", "S"), "Y"),
    Cell("sky130_fd_sc_hd__a31oi_1", 5, 1000, "logic", "CORE", ("A1", "A2", "A3", "B1"), "Y"),
    Cell("sky130_fd_sc_hd__nor2b_1", 5, 700, "logic", "CORE", ("A", "B_N"), "Y"),
    Cell("sky130_fd_sc_hd__conb_1", 3, 253, "logic", "CORE", (), "HI"),
)

CELLS_BY_NAME = {c.name: c for c in CELLS}
FILL_1 = CELLS_BY_NAME["sky130_fd_sc_hd__fill_1"]

# Signal-net pin-count distribution, measured from the reference design's
# NETS section (pin counts include the driver).
#: Cap on nets in flight while streaming, so memory stays O(1).
_MAX_OPEN_NETS = 8

NET_PIN_WEIGHTS: tuple[tuple[int, int], ...] = (
    (2, 118807),
    (3, 23434),
    (7, 20275),
    (4, 10970),
    (5, 4820),
    (6, 3665),
    (8, 814),
    (9, 345),
    (10, 213),
    (12, 227),
    (11, 142),
)


@dataclass(frozen=True)
class DesignSpec:
    """Knobs for a generated design.  ``instances`` is the only one most
    callers need; everything else is derived to stay self-consistent."""

    instances: int = 20_000
    seed: int = 0
    design: str = "generated_top"
    gzip: bool = True
    io_pins: int = 32
    #: Instances per PG connection line in SPECIALNETS.  The reference
    #: design wraps at 8, which is what makes its SPECIALNETS section
    #: 1.2M lines long for only two nets.
    pg_conns_per_line: int = 8


@dataclass(frozen=True)
class Placement:
    name: str
    cell: Cell
    x: int
    y: int
    orient: str
    status: str

    @property
    def bbox(self) -> tuple[int, int, int, int]:
        # Only N/FS are emitted, so width and height are never swapped.
        return (self.x, self.y, self.x + self.cell.width, self.y + SITE_H)


@dataclass
class Layout:
    sites_per_row: int
    rows: int
    die_w: int
    die_h: int
    instances: int
    per_macro: dict[str, int] = field(default_factory=dict)


def _origin_x() -> int:
    return 12 * SITE_W  # matches the reference design's core offset


def _origin_y() -> int:
    return 4 * SITE_H


def _sites_per_row(spec: DesignSpec) -> int:
    """Pick a row length that yields a roughly square die."""
    total_w = sum(c.weight for c in CELLS)
    avg_sites = sum(c.sites * c.weight for c in CELLS) / total_w
    total_sites = spec.instances * avg_sites
    # square die: sites_per_row * SITE_W == rows * SITE_H
    sites_per_row = int((total_sites * SITE_H / SITE_W) ** 0.5)
    return max(sites_per_row, max(c.sites for c in CELLS))


def _iter_placements(spec: DesignSpec) -> Iterator[Placement]:
    """Yield the design's instances.

    Called once per DEF section that needs them.  A fresh ``Random`` is
    seeded here, so every call reproduces the identical stream and no
    section has to be buffered.
    """
    rng = random.Random(spec.seed)
    cells = list(CELLS)
    # Precomputed cumulative weights: random.choices rebuilds them on every
    # call otherwise, which dominates runtime at millions of instances.
    cum = list(itertools.accumulate(c.weight for c in cells))
    sites_per_row = _sites_per_row(spec)
    x0, y0 = _origin_x(), _origin_y()

    emitted = 0
    row = 0
    site = 0
    counters: Counter[str] = Counter()

    while emitted < spec.instances:
        cell = rng.choices(cells, cum_weights=cum)[0]
        if site + cell.sites > sites_per_row:
            # Pad the row tail with 1-site filler, exactly as a real fill
            # step does, then move on.
            while site < sites_per_row and emitted < spec.instances:
                yield _make(FILL_1, row, site, x0, y0, counters)
                emitted += 1
                site += 1
            row += 1
            site = 0
            continue
        yield _make(cell, row, site, x0, y0, counters)
        emitted += 1
        site += cell.sites


def _make(cell: Cell, row: int, site: int, x0: int, y0: int, counters: Counter[str]) -> Placement:
    idx = counters[cell.kind]
    counters[cell.kind] += 1
    orient = "N" if row % 2 == 0 else "FS"
    if cell.kind == "fill":
        name, status = f"FILLER_{row}_{site}_{idx}", "FIXED"
    elif cell.kind == "tap":
        name, status = f"TAP_{idx}", "FIXED"
    elif cell.kind == "diode":
        name, status = f"ANTENNA_{idx}", "PLACED"
    else:
        # Hierarchical names with bus bits, to exercise DIVIDERCHAR and
        # BUSBITCHARS handling on the hot path.
        name = f"core/u_dec_{idx % 97}/reg[{idx % 64}]/U{idx}"
        status = "PLACED"
    return Placement(name, cell, x0 + site * SITE_W, y0 + row * SITE_H, orient, status)


def _layout(spec: DesignSpec) -> Layout:
    """Walk the placement stream once to learn the design's true extent."""
    sites_per_row = _sites_per_row(spec)
    per_macro: Counter[str] = Counter()
    max_x = 0
    rows = 0
    n = 0
    for p in _iter_placements(spec):
        per_macro[p.cell.name] += 1
        max_x = max(max_x, p.x + p.cell.width)
        rows = (p.y - _origin_y()) // SITE_H + 1
        n += 1
    return Layout(
        sites_per_row=sites_per_row,
        rows=rows,
        die_w=max_x + _origin_x(),
        die_h=_origin_y() + rows * SITE_H + _origin_y(),
        instances=n,
        per_macro=dict(per_macro),
    )


# --------------------------------------------------------------------------
# LEF emission
# --------------------------------------------------------------------------


def _write_lef(out: IO[str]) -> None:
    out.write(
        "VERSION 5.7 ;\n"
        'BUSBITCHARS "[]" ;\n'
        'DIVIDERCHAR "/" ;\n\n'
        "UNITS\n"
        "  TIME NANOSECONDS 1 ;\n"
        "  CAPACITANCE PICOFARADS 1 ;\n"
        "  RESISTANCE OHMS 1 ;\n"
        f"  DATABASE MICRONS {DBU} ;\n"
        "END UNITS\n\n"
        "MANUFACTURINGGRID 0.005 ;\n\n"
    )
    for name, pitch in LAYERS:
        direction = "VERTICAL" if name in ("li1", "met2", "met4") else "HORIZONTAL"
        out.write(
            f"LAYER {name}\n"
            "  TYPE ROUTING ;\n"
            f"  DIRECTION {direction} ;\n"
            f"  PITCH {pitch / DBU:.3f} ;\n"
            f"END {name}\n\n"
        )
    out.write(
        f"SITE {SITE_NAME}\n"
        "  SYMMETRY Y ;\n"
        "  CLASS CORE ;\n"
        f"  SIZE {SITE_W / DBU:.2f} BY {SITE_H / DBU:.2f} ;\n"
        f"END {SITE_NAME}\n\n"
    )
    for cell in CELLS:
        _write_macro(out, cell)
    out.write("END LIBRARY\n")


def _write_macro(out: IO[str], cell: Cell) -> None:
    w = cell.width / DBU
    h = SITE_H / DBU
    out.write(
        f"MACRO {cell.name}\n"
        f"  CLASS {cell.cls} ;\n"
        f"  FOREIGN {cell.name} ;\n"
        "  ORIGIN 0.000 0.000 ;\n"
        f"  SIZE {w:.3f} BY {h:.3f} ;\n"
        "  SYMMETRY X Y R90 ;\n"
        f"  SITE {SITE_NAME} ;\n"
    )
    span = w / max(len(cell.inputs) + 1, 1)
    for i, pin in enumerate(cell.inputs):
        _write_pin(out, pin, "INPUT", "SIGNAL", i * span + 0.09, 1.05, i * span + 0.35, 1.32)
    if cell.output:
        _write_pin(out, cell.output, "OUTPUT", "SIGNAL", w - 0.35, 1.05, w - 0.09, 1.32)
    for pg in cell.pg:
        use = "GROUND" if pg in ("VGND", "VNB") else "POWER"
        shape = "  SHAPE ABUTMENT ;\n" if pg in ("VGND", "VPWR") else ""
        y0, y1 = (-0.24, 0.24) if pg in ("VGND", "VNB") else (h - 0.24, h + 0.24)
        layer = "met1" if pg in ("VGND", "VPWR") else ("pwell" if use == "GROUND" else "nwell")
        out.write(
            f"  PIN {pg}\n"
            "    DIRECTION INOUT ;\n"
            f"    USE {use} ;\n"
            f"{shape}"
            "    PORT\n"
            f"      LAYER {layer} ;\n"
            f"        RECT 0.000 {y0:.3f} {w:.3f} {y1:.3f} ;\n"
            "    END\n"
            f"  END {pg}\n"
        )
    out.write(f"END {cell.name}\n\n")


def _write_pin(out: IO[str], pin: str, direction: str, use: str, x0, y0, x1, y1) -> None:
    out.write(
        f"  PIN {pin}\n"
        f"    DIRECTION {direction} ;\n"
        f"    USE {use} ;\n"
        "    PORT\n"
        "      LAYER li1 ;\n"
        f"        RECT {x0:.3f} {y0:.3f} {x1:.3f} {y1:.3f} ;\n"
        "    END\n"
        f"  END {pin}\n"
    )


# --------------------------------------------------------------------------
# Signal nets
# --------------------------------------------------------------------------


def _net_name(index: int, spec: DesignSpec) -> str:
    if index < spec.io_pins:
        return f"io_net_{index}"
    if index % 3 == 0:
        # Bus bits exercise BUSBITCHARS on the hot path.
        return f"bus_{index // 64}[{index % 64}]"
    return f"n_{index}"


def _iter_nets(spec: DesignSpec) -> Iterator[tuple[str, list[tuple[str, str]]]]:
    """Yield ``(net_name, [(instance, pin), ...])``.

    Each logic instance drives one new net and loads one open net per
    input, so instances sit on ~(1 + inputs) nets just as they do in a
    real netlist -- that fanout, not the raw instance count, is what sets
    the size of the NETS section.  Nets are grown in placement order, so
    their members are physically adjacent: the same locality a real
    placed design has, which is what makes cache benchmarks meaningful.

    At most ``_MAX_OPEN_NETS`` nets are in flight, keeping memory O(1).
    """
    rng = random.Random(spec.seed ^ 0x5EED)
    sizes = [s for s, _ in NET_PIN_WEIGHTS]
    cum = list(itertools.accumulate(w for _, w in NET_PIN_WEIGHTS))
    index = 0
    open_nets: deque[tuple[int, list[tuple[str, str]]]] = deque()

    for p in _iter_placements(spec):
        if p.cell.kind not in ("logic", "diode"):
            continue
        # Load existing open nets with this instance's input pins.
        for pin in p.cell.inputs:
            if not open_nets:
                break
            target, conns = open_nets[0]
            conns.append((p.name, pin))
            if len(conns) >= target:
                open_nets.popleft()
                yield _net_name(index, spec), conns
                index += 1
            else:
                open_nets.rotate(-1)
        # Then open a net driven by this instance, so it never loads its own.
        if p.cell.output:
            open_nets.append((rng.choices(sizes, cum_weights=cum)[0], [(p.name, p.cell.output)]))
            if len(open_nets) > _MAX_OPEN_NETS:
                _, conns = open_nets.popleft()
                if len(conns) >= 2:
                    yield _net_name(index, spec), conns
                    index += 1

    for _, conns in open_nets:
        if len(conns) >= 2:
            yield _net_name(index, spec), conns
            index += 1


def _io_net_names(spec: DesignSpec) -> list[str]:
    return [f"io_net_{i}" for i in range(spec.io_pins)]


# --------------------------------------------------------------------------
# DEF emission
# --------------------------------------------------------------------------


def _write_def(out: IO[str], spec: DesignSpec, layout: Layout) -> dict:
    counts: dict[str, int] = {}
    out.write(
        "VERSION 5.8 ;\n"
        'DIVIDERCHAR "/" ;\n'
        'BUSBITCHARS "[]" ;\n'
        f"DESIGN {spec.design} ;\n"
        f"UNITS DISTANCE MICRONS {DBU} ;\n"
        f"DIEAREA ( 0 0 ) ( {layout.die_w} {layout.die_h} ) ;\n"
    )
    _write_rows(out, layout)
    _write_tracks(out, layout)
    out.write(
        f"GCELLGRID X 0 DO {layout.die_w // (15 * SITE_W) + 1} STEP {15 * SITE_W} ;\n"
        f"GCELLGRID Y 0 DO {layout.die_h // (15 * SITE_W) + 1} STEP {15 * SITE_W} ;\n"
    )
    _write_vias(out)
    counts["components"] = _write_components(out, spec)
    counts["pins"] = _write_pins(out, spec, layout)
    counts["specialnets"] = _write_specialnets(out, spec, layout)
    counts["nets"] = _write_nets(out, spec)
    out.write("END DESIGN\n")
    return counts


def _write_rows(out: IO[str], layout: Layout) -> None:
    x0, y0 = _origin_x(), _origin_y()
    for row in range(layout.rows):
        orient = "N" if row % 2 == 0 else "FS"
        out.write(
            f"ROW ROW_{row} {SITE_NAME} {x0} {y0 + row * SITE_H} {orient} "
            f"DO {layout.sites_per_row} BY 1 STEP {SITE_W} 0 ;\n"
        )


def _write_tracks(out: IO[str], layout: Layout) -> None:
    for name, pitch in LAYERS:
        out.write(
            f"TRACKS X {pitch // 2} DO {layout.die_w // pitch} STEP {pitch} LAYER {name} ;\n"
            f"TRACKS Y {pitch // 2} DO {layout.die_h // pitch} STEP {pitch} LAYER {name} ;\n"
        )


def _write_vias(out: IO[str]) -> None:
    vias = (
        ("via2_3_1600_480_1_5_320_320", "M1M2_PR", 150, "met1 via met2", "85 165 55 85", "1 5"),
        ("via3_4_1600_480_1_4_400_400", "M2M3_PR", 200, "met2 via2 met3", "40 85 65 65", "1 4"),
        ("via4_5_1600_480_1_4_400_400", "M3M4_PR", 200, "met3 via3 met4", "90 60 100 65", "1 4"),
        (
            "via5_6_1600_1600_1_1_1600_1600",
            "M4M5_PR",
            800,
            "met4 via4 met5",
            "400 190 310 400",
            None,
        ),
    )
    out.write(f"VIAS {len(vias)} ;\n")
    for name, rule, cut, layers, enc, rowcol in vias:
        rc = f" + ROWCOL {rowcol} " if rowcol else " "
        out.write(
            f"    - {name} + VIARULE {rule} + CUTSIZE {cut} {cut} "
            f" + LAYERS {layers}  + CUTSPACING {cut} {cut}  + ENCLOSURE {enc} {rc} ;\n"
        )
    out.write("END VIAS\n")


def _write_components(out: IO[str], spec: DesignSpec) -> int:
    out.write(f"COMPONENTS {spec.instances} ;\n")
    n = 0
    for p in _iter_placements(spec):
        # Fill, decap and tap cells carry "+ SOURCE DIST"; in the reference
        # design that is 92% of all component records.
        source = " + SOURCE DIST" if p.cell.kind in ("fill", "tap") else ""
        out.write(
            f"    - {p.name} {p.cell.name}{source} + {p.status} ( {p.x} {p.y} ) {p.orient} ;\n"
        )
        n += 1
    out.write("END COMPONENTS\n")
    return n


def _write_pins(out: IO[str], spec: DesignSpec, layout: Layout) -> int:
    names = _io_net_names(spec)
    out.write(f"PINS {len(names) + 2} ;\n")
    for net in ("VGND", "VPWR"):
        use = "GROUND" if net == "VGND" else "POWER"
        out.write(
            f"    - {net} + NET {net} + SPECIAL + DIRECTION INOUT + USE {use}\n      + PORT\n"
        )
        for i in range(4):
            # Port rects are relative to the placement point, so they are
            # routinely negative in real files.
            out.write(
                f"        + LAYER met4 ( -800 {-223280 - i * 1000} ) ( 800 {223280 + i * 1000} )\n"
            )
        out.write(f"        + FIXED ( {layout.die_w // 2} {layout.die_h // 2} ) N ;\n")
    for i, net in enumerate(names):
        direction = "INPUT" if i % 3 else "OUTPUT"
        y = _origin_y() + (i * SITE_H * 3) % max(layout.die_h - _origin_y(), SITE_H)
        out.write(
            f"    - {net} + NET {net} + DIRECTION {direction} + USE SIGNAL\n"
            "      + PORT\n"
            "        + LAYER met3 ( -2000 -300 ) ( 2000 300 )\n"
            f"        + PLACED ( {layout.die_w - _origin_x()} {y} ) N ;\n"
        )
    out.write("END PINS\n")
    return len(names) + 2


def _pg_pins(cell: Cell, net: str) -> tuple[str, ...]:
    """Every pin on ``cell`` that carries ``net``.

    Cells tie power through both the rail pin and the well pin (VPWR+VPB,
    VGND+VNB), and the DEF lists each one, so a PG net names most
    instances twice.  Tap cells expose only the rails.  Getting this wrong
    halves the size of the SPECIALNETS section, which is 35% of a real
    post-fill DEF.
    """
    rails = ("VGND", "VNB") if net == "VGND" else ("VPWR", "VPB")
    return tuple(pin for pin in rails if pin in cell.pg)


def _write_specialnets(out: IO[str], spec: DesignSpec, layout: Layout) -> int:
    out.write("SPECIALNETS 2 ;\n")
    for net in ("VGND", "VPWR"):
        out.write(f"    - {net} ( PIN {net} )")
        col = 1
        for p in _iter_placements(spec):
            for pin in _pg_pins(p.cell, net):
                if col % spec.pg_conns_per_line == 0:
                    out.write("\n     ")
                out.write(f" ( {p.name} {pin} )")
                col += 1
        out.write("\n")
        _write_pg_routing(out, net, layout)
    out.write("END SPECIALNETS\n")
    return 2


def _write_pg_routing(out: IO[str], net: str, layout: Layout) -> None:
    """Emit FOLLOWPIN rails and STRIPE straps, terminating the net record."""
    x0 = _origin_x()
    x1 = x0 + layout.sites_per_row * SITE_W
    offset = 0 if net == "VGND" else SITE_H // 2

    def segments() -> Iterator[str]:
        for row in range(layout.rows):
            y = _origin_y() + row * SITE_H + offset
            yield f"met1 480 + SHAPE FOLLOWPIN ( {x0} {y} ) ( {x1} {y} )"
        # Vertical straps every 64 sites, as a PG grid does.
        stripe_pitch = 64 * SITE_W
        y_top = _origin_y() + layout.rows * SITE_H
        for x in range(x0 + (0 if net == "VGND" else stripe_pitch // 2), x1, stripe_pitch):
            yield f"met4 1600 + SHAPE STRIPE ( {x} {_origin_y()} ) ( {x} {y_top} )"

    prev: str | None = None
    first = True
    for seg in segments():
        if prev is not None:
            out.write(f"      {'+ ROUTED' if first else 'NEW'} {prev}\n")
            first = False
        prev = seg
    if prev is not None:
        out.write(f"      {'+ ROUTED' if first else 'NEW'} {prev} ;\n")


def _write_nets(out: IO[str], spec: DesignSpec) -> int:
    total = sum(1 for _ in _iter_nets(spec))
    out.write(f"NETS {total} ;\n")
    io_names = set(_io_net_names(spec))
    for name, conns in _iter_nets(spec):
        parts = []
        if name in io_names:
            parts.append(f"( PIN {name} )")
        parts.extend(f"( {inst} {pin} )" for inst, pin in conns)
        out.write(f"    - {name}")
        for i, part in enumerate(parts):
            if i and i % 8 == 0:
                out.write("\n     ")
            out.write(f" {part}")
        out.write(" + USE SIGNAL ;\n")
    out.write("END NETS\n")
    return total


# --------------------------------------------------------------------------
# Entry point
# --------------------------------------------------------------------------


def generate(spec: DesignSpec, out_dir: Path) -> dict:
    """Write ``design.lef``, ``design.def[.gz]`` and ``manifest.json``.

    Returns the manifest: the generator's own record of what it wrote.
    Tests assert against this rather than recomputing expectations, so a
    generator bug shows up as a mismatch instead of silently agreeing
    with a parser bug.
    """
    out_dir = Path(out_dir)
    out_dir.mkdir(parents=True, exist_ok=True)
    layout = _layout(spec)

    sdc_path = out_dir / "design.sdc"
    with sdc_path.open("w", encoding="utf-8") as fh:
        clock_net = _write_sdc(fh, spec)

    lef_path = out_dir / "design.lef"
    with lef_path.open("w", encoding="utf-8") as fh:
        _write_lef(fh)

    def_path = out_dir / ("design.def.gz" if spec.gzip else "design.def")
    if spec.gzip:
        with gzip.open(def_path, "wt", encoding="utf-8", compresslevel=6) as fh:
            counts = _write_def(fh, spec, layout)
    else:
        with def_path.open("w", encoding="utf-8") as fh:
            counts = _write_def(fh, spec, layout)

    manifest = {
        "spec": asdict(spec),
        "lef": lef_path.name,
        "def": def_path.name,
        "sdc": sdc_path.name,
        "clock_net": clock_net,
        "counts": counts,
        "layout": {
            "rows": layout.rows,
            "sites_per_row": layout.sites_per_row,
            "die": [0, 0, layout.die_w, layout.die_h],
        },
        "per_macro": layout.per_macro,
        "spot_checks": _spot_checks(spec),
        "bytes": {"lef": lef_path.stat().st_size, "def": def_path.stat().st_size},
    }
    manifest_path = out_dir / "manifest.json"
    manifest_path.write_text(json.dumps(manifest, indent=2, sort_keys=True), encoding="utf-8")
    return manifest


def _write_sdc(out: IO[str], spec: DesignSpec) -> str:
    """Write constraints over the design's I/O nets.

    Shape follows a real ORFS ``base.sdc``: a couple of ``create_clock``
    commands buried in ~140 lines of ``set_input_delay`` and
    ``set_driving_cell``, which is what the command splitter and
    tokenizer actually have to chew through.
    """
    names = _io_net_names(spec)
    clock = names[0]
    out.write(f"current_design {spec.design}\n")
    out.write(f"create_clock -name core_clk -period 10.0 [get_ports {{{clock}}}]\n")
    if len(names) > 1:
        out.write(f"create_clock -name io_clk -period 20.0 [get_ports {{{names[1]}}}]\n")
    for name in names[1:]:
        out.write(f"set_input_delay 2.0 -clock core_clk [get_ports {{{name}}}]\n")
        out.write(
            f"set_driving_cell -lib_cell sky130_fd_sc_hd__inv_2 -pin Y [get_ports {{{name}}}]\n"
        )
    out.write("set_output_delay 2.0 -clock core_clk [all_outputs]\n")
    out.write("set_max_fanout 10 [current_design]\n")
    return clock


def _spot_checks(spec: DesignSpec, count: int = 12) -> list[dict]:
    """Exact expected bboxes for a spread of instances, for tests to pin."""
    step = max(spec.instances // count, 1)
    picked = []
    for i, p in enumerate(_iter_placements(spec)):
        if i % step:
            continue
        picked.append(
            {
                "index": i,
                "name": p.name,
                "macro": p.cell.name,
                "orient": p.orient,
                "status": p.status,
                "bbox": list(p.bbox),
            }
        )
        if len(picked) == count:
            break
    return picked


def main(argv: list[str] | None = None) -> int:
    import argparse

    ap = argparse.ArgumentParser(description=(__doc__ or "").splitlines()[0])
    ap.add_argument("-o", "--out-dir", type=Path, required=True)
    ap.add_argument("-n", "--instances", type=int, default=DesignSpec.instances)
    ap.add_argument("-s", "--seed", type=int, default=DesignSpec.seed)
    ap.add_argument("--design", default=DesignSpec.design)
    ap.add_argument("--no-gzip", action="store_true")
    args = ap.parse_args(argv)

    spec = DesignSpec(
        instances=args.instances, seed=args.seed, design=args.design, gzip=not args.no_gzip
    )
    manifest = generate(spec, args.out_dir)
    print(json.dumps({k: manifest[k] for k in ("counts", "layout", "bytes")}, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
