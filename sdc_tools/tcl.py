"""Evaluate Synopsys Design Constraints in a real Tcl interpreter.

SDC is freeform Tcl. A hand-written subset parser always lags variables,
``[command]`` substitution, loops and flow-generated scripts. This module runs
the file in a sandboxed ``tclsh`` whose SDC commands are stubs that record
their arguments. Unknown commands are recorded and return an empty list, so a
full SDC never aborts on a constraint we do not model yet.

Tcl >= 8.6 is an environment prerequisite (``TCLSH`` overrides discovery);
there is deliberately no static-parser fallback.
"""

from __future__ import annotations

import os
import shutil
from pathlib import Path

from liberty_tools import _native

TCL_MINIMUM = "8.6"


class SdcError(ValueError):
    """An SDC could not be evaluated."""


def find_tclsh() -> str:
    """Locate a Tcl interpreter; ``TCLSH`` wins over PATH discovery."""
    override = os.environ.get("TCLSH")
    if override:
        return override
    for name in ("tclsh8.6", "tclsh"):
        found = shutil.which(name)
        if found:
            return found
    raise SdcError(
        f"sdc-tools requires Tcl >= {TCL_MINIMUM} (tclsh); "
        "install it or set TCLSH to its path"
    )


def evaluate(path: Path) -> tuple[list[list[str]], str | None]:
    """Run ``path`` and return ``(records, error)``.

    Each record is the command name followed by its Tcl words, with
    collections represented symbolically (``port:clk``, ``pin:u/CLK``, ...).
    """
    try:
        return _native.evaluate_sdc(path)
    except ValueError as exc:
        raise SdcError(str(exc)) from exc
