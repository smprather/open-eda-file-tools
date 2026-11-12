"""Identify clock-tree instances by tracing forward from SDC clock roots."""

from __future__ import annotations

from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass
from pathlib import Path

from liberty_tools import parse_file
from liberty_tools._native import clock_tree_cells as _clock_tree_cells

from .clocks import Clock, clocks

MacroPins = tuple[list[str], list[str], bool]


@dataclass(frozen=True)
class ClockCells:
    """The clocks an SDC creates and the distribution cells they reach."""

    clocks: tuple[Clock, ...]
    cells: tuple[str, ...]


def _cell_pins(document) -> dict[str, MacroPins]:
    result: dict[str, MacroPins] = {}
    for name in document.cells():
        cell = document.cell(name)
        inputs: list[str] = []
        outputs: list[str] = []
        for pin_name in cell.pins():
            direction = cell.pin(pin_name).direction
            if direction == "input":
                inputs.append(pin_name)
            elif direction == "output":
                outputs.append(pin_name)
        sequential = cell.ff() is not None or cell.latch() is not None
        result[name] = (inputs, outputs, sequential)
    return result


def macro_pin_data(libs: list[Path]) -> dict[str, MacroPins]:
    """Pin directions and sequential type for every cell, one thread per lib.

    ``parse_file`` releases the GIL, so the libraries parse concurrently. Later
    libraries override earlier ones for a duplicated cell name.
    """
    if not libs:
        return {}
    with ThreadPoolExecutor(max_workers=len(libs)) as pool:
        documents = list(pool.map(lambda path: parse_file(str(path)), libs))
    merged: dict[str, MacroPins] = {}
    for document in documents:
        merged.update(_cell_pins(document))
    return merged


def clock_cells(def_path: Path | str, libs: list[Path], sdc_path: Path | str) -> ClockCells:
    """Return the clocks and the clock-tree instances reachable from them."""
    found = clocks(sdc_path)
    roots = sorted({source for clock in found for source in clock.sources})
    if not roots:
        return ClockCells(clocks=tuple(found), cells=())
    macros = [
        (name, inputs, outputs, sequential)
        for name, (inputs, outputs, sequential) in sorted(macro_pin_data(libs).items())
    ]
    cells = _clock_tree_cells(str(def_path), roots, macros)
    return ClockCells(clocks=tuple(found), cells=tuple(cells))
