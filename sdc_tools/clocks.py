"""Clock extraction from SDC (``create_clock`` / ``create_generated_clock``)."""

from __future__ import annotations

from dataclasses import dataclass, field
from pathlib import Path

from liberty_tools import _native


@dataclass(frozen=True)
class Clock:
    """One clock definition.

    ``period`` and ``waveform`` are in the SDC's time unit (ns by convention).
    ``sources`` are symbolic endpoints: ``port:clk``, ``pin:div/Q``,
    ``net:clk``, ``cell:u1``.
    """

    name: str
    period: float | None
    waveform: tuple[float, ...] = ()
    sources: tuple[str, ...] = field(default_factory=tuple)
    generated_from: str | None = None
    divide_by: int | None = None
    multiply_by: int | None = None
    edges: tuple[float, ...] = ()
    invert: bool = False


def clocks(path: Path | str) -> list[Clock]:
    """Return the clocks an SDC creates, in file order."""
    return [
        Clock(
            name=name,
            period=period,
            waveform=tuple(waveform),
            sources=tuple(sources),
            generated_from=master,
            divide_by=divide,
            multiply_by=multiply,
            edges=tuple(edges),
            invert=invert,
        )
        for name, period, waveform, sources, master, divide, multiply, edges, invert
        in _native.sdc_clocks(path)
    ]
