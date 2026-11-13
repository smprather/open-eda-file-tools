"""Clock roots for ``def-tools clock-instances``, read through ``sdc_tools``.

The SDC is evaluated by the real Tcl interpreter in ``sdc_tools``; there is no
second, hand-written SDC parser here.
"""

from __future__ import annotations

from pathlib import Path

ClockRoot = tuple[str, str, str]


def clock_roots_from_sdc(path: Path) -> list[ClockRoot]:
    """``(clock, root type, root name)`` for every clock source in the SDC.

    Root types are ``port``, ``pin`` and ``net``. Needs ``tclsh`` >= 8.6.
    """
    from sdc_tools import clocks

    roots: list[ClockRoot] = []
    for clock in clocks(path):
        for source in clock.sources:
            kind, _, name = source.partition(":")
            if kind in ("port", "pin", "net") and name:
                roots.append((clock.name, kind, name))
    return roots
