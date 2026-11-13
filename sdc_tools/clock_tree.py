"""Identify clock-tree instances by tracing forward from SDC clock roots."""

from __future__ import annotations

from collections.abc import Sequence
from dataclasses import dataclass
from pathlib import Path

from def_tools.clock_roles import cell_clock_roles, native_macro_roles
from liberty_tools._native import clock_tree_cells as _clock_tree_cells

from .clocks import Clock, clocks


@dataclass(frozen=True)
class ClockCells:
    """The clocks an SDC creates and the distribution cells they reach."""

    clocks: tuple[Clock, ...]
    cells: tuple[str, ...]


def clock_cells(
    def_path: Path | str,
    libs: Sequence[Path | str],
    sdc_path: Path | str,
    *,
    prune: bool = True,
) -> ClockCells:
    """Return the clocks and the clock-tree instances reachable from them.

    The trace follows clock-through pins from every clock root: all inputs of
    a combinational cell, only the clock pin of an integrated clock gate. It
    ends at flip-flops, latches and memories, which are not reported.

    With ``prune`` (the default) a cell is reported only when a clock pin is
    reachable from it, so the datapath logic a divider flop's output also
    feeds is not mistaken for clock tree. ``prune=False`` reports every cell
    the forward trace touches, including logic that only reaches data pins or
    output ports.
    """
    found = clocks(sdc_path)
    roots = sorted({source for clock in found for source in clock.sources})
    if not roots:
        return ClockCells(clocks=tuple(found), cells=())
    macros = native_macro_roles(cell_clock_roles(list(libs)))
    cells = _clock_tree_cells(str(def_path), roots, macros, prune)
    return ClockCells(clocks=tuple(found), cells=tuple(cells))
