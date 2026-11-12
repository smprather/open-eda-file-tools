"""SDC (Synopsys Design Constraints) clock extraction.

The SDC is evaluated in a sandboxed Tcl interpreter (system ``tclsh`` >= 8.6);
SDC commands are stubs, so variables, loops, command substitution and
flow-generated scripts work without a hand-written Tcl subset parser.
"""

from __future__ import annotations

from .clock_tree import ClockCells, clock_cells
from .clocks import Clock, clocks
from .tcl import SdcError, evaluate, find_tclsh
from .version import get_version

__version__ = get_version()

__all__ = [
    "Clock",
    "ClockCells",
    "SdcError",
    "__version__",
    "clock_cells",
    "clocks",
    "evaluate",
    "find_tclsh",
    "get_version",
]
