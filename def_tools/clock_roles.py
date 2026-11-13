"""What each Liberty cell does with a clock: pass it on, gate it, or end it.

Both clock identifiers use this one model: the SDC forward trace
(``sdc_tools.clock_cells``) and the clock-data resolver
(``def_tools.clock_data``).
"""

from __future__ import annotations

import re
from collections.abc import Sequence
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass
from pathlib import Path

_IDENTIFIER = re.compile(r"[A-Za-z_][A-Za-z0-9_]*(?:\[[0-9]+\])?")

# (name, clock-through inputs, outputs, endpoint, clock pins): the tuple the
# native connectivity kernels take.
NativeMacroRole = tuple[str, list[str], list[str], bool, list[str]]


@dataclass(frozen=True)
class CellClockRole:
    """How one Liberty cell treats a clock arriving on its pins.

    ``inputs`` are the pins a clock propagates *through*: every input of a
    combinational cell, only the clock pin of an integrated clock gate, so an
    enable or test pin never carries a clock into the gate. ``endpoint`` cells
    (flip-flops, latches, memories and other cells with a ``clock : true`` pin)
    end a clock path and are never clock-tree cells. ``clock_pins`` are the
    pins a clock is delivered to.
    """

    inputs: tuple[str, ...]
    outputs: tuple[str, ...]
    endpoint: bool
    clock_pins: tuple[str, ...]
    clock_gate: bool
    sequential: bool


def _is_true(value: str | None) -> bool:
    return value is not None and value.strip().strip('"').lower() == "true"


def _expression_pins(expression: str | None, pins: Sequence[str]) -> list[str]:
    if not expression:
        return []
    names = set(_IDENTIFIER.findall(expression))
    return [pin for pin in pins if pin in names]


def cell_clock_role(cell) -> CellClockRole:
    """Derive the clock role of one ``liberty_tools`` cell."""
    inputs: list[str] = []
    outputs: list[str] = []
    flagged: dict[str, list[str]] = {
        "clock": [],
        "clock_gate_clock_pin": [],
        "clock_gate_enable_pin": [],
        "clock_gate_test_pin": [],
        "clock_gate_out_pin": [],
    }
    for pin_name in cell.pins():
        pin = cell.pin(pin_name)
        direction = pin.direction
        if direction == "input":
            inputs.append(pin_name)
        elif direction == "output":
            outputs.append(pin_name)
        for attribute, names in flagged.items():
            if _is_true(pin.get(attribute)):
                names.append(pin_name)

    ff = cell.ff()
    latch = cell.latch()
    sequential = ff is not None or latch is not None
    state_clocks: list[str] = []
    if ff is not None:
        state_clocks += _expression_pins(ff.clocked_on, inputs)
        state_clocks += _expression_pins(ff.clocked_on_also, inputs)
    if latch is not None:
        state_clocks += _expression_pins(latch.enable, inputs)
    clock_inputs = [pin for pin in flagged["clock"] if pin in inputs]

    if cell.get("clock_gating_integrated_cell") is not None:
        # The clock path of a gate is clock pin -> gated-clock output. Prefer
        # the explicit pin attributes; fall back to the clock the latch uses.
        not_clock = set(flagged["clock_gate_enable_pin"] + flagged["clock_gate_test_pin"])
        through = (
            [pin for pin in flagged["clock_gate_clock_pin"] if pin in inputs]
            or clock_inputs
            or state_clocks
            or [pin for pin in inputs if pin not in not_clock]
        )
        through = [pin for pin in dict.fromkeys(through) if pin not in not_clock]
        gated = [pin for pin in flagged["clock_gate_out_pin"] if pin in outputs]
        return CellClockRole(
            inputs=tuple(through),
            outputs=tuple(gated or outputs),
            endpoint=False,
            clock_pins=tuple(through),
            clock_gate=True,
            sequential=sequential,
        )

    clock_pins = tuple(dict.fromkeys(clock_inputs + state_clocks))
    return CellClockRole(
        inputs=tuple(inputs),
        outputs=tuple(outputs),
        endpoint=sequential or bool(clock_pins),
        clock_pins=clock_pins,
        clock_gate=False,
        sequential=sequential,
    )


def cell_clock_roles(libs: Sequence[Path | str]) -> dict[str, CellClockRole]:
    """Clock role of every cell in ``libs``, one thread per library.

    Only structural fields are read, so each library parses with
    ``skip_tables=True``; ``parse_file`` releases the GIL, so the set parses
    concurrently. A later library wins for a duplicated cell name.
    """
    from liberty_tools import parse_file

    if not libs:
        return {}
    with ThreadPoolExecutor(max_workers=len(libs)) as pool:
        documents = list(
            pool.map(lambda path: parse_file(str(path), skip_tables=True), libs)
        )
    roles: dict[str, CellClockRole] = {}
    for document in documents:
        for name in document.cells():
            roles[name] = cell_clock_role(document.cell(name))
    return roles


def native_macro_roles(roles: dict[str, CellClockRole]) -> list[NativeMacroRole]:
    """``roles`` in the tuple form the native connectivity kernels take."""
    return [
        (name, list(role.inputs), list(role.outputs), role.endpoint, list(role.clock_pins))
        for name, role in sorted(roles.items())
    ]
