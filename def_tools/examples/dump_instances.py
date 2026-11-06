#!/usr/bin/env python3
"""Example: dump DEF instances as Parquet (default), CSV, or an aligned table.

This is a thin wrapper around ``def_tools.dump_instances``, the module behind
the ``def-dump-instances`` console script. Prefer the console script: its
shebang hard-references the interpreter that owns the installed package, so a
different ``python3`` earlier on ``PATH`` cannot run it.

Columns: instance name, base cell, and the bounding box in microns
(``llx, lly, urx, ury``). Microns are DEF database units divided by the
design's ``UNITS DISTANCE MICRONS`` value. ``--max-memory`` (default 1 GiB)
bounds the Parquet batch and aborts the run if the process peak RSS exceeds it
by more than 10% (a soft cap).

Ways to run it, most pinned first:

    def-dump-instances --lef merged.min.lef --def 17-fill.def.gz

    uv run python examples/dump_instances.py \
        --lef merged.min.lef --def 17-fill.def.gz

    uv run --project /path/to/def_tools python \
        /path/to/def_tools/examples/dump_instances.py \
        --lef merged.min.lef --def 17-fill.def.gz

For a read-only checkout or an air-gapped run, use
``def_tools/scripts/run_example_offline.sh dump_instances.py --lef ...`` from a
writable working directory; it selects an interpreter new enough for the
package, and ``uv run`` writes an editable install into the source tree.

Run under an interpreter the package is not installed in, or one older than
3.14, exits with a message instead of an import traceback.
"""

from __future__ import annotations

import sys

DISTRIBUTION = "def-tools"
REQUIRES_PYTHON = (3, 14)
_HINT = (
    "run it with the interpreter that owns the package:\n"
    "  def-dump-instances --lef ... --def ...\n"
    "  uv run --project <def_tools> python examples/dump_instances.py --lef ... --def ...\n"
    "  <def_tools>/scripts/run_example_offline.sh dump_instances.py --lef ... --def ..."
)


def _die(message: str) -> SystemExit:
    return SystemExit(f"{DISTRIBUTION} example: {message}\n{_HINT}")


if sys.version_info < REQUIRES_PYTHON:
    raise _die(
        f"needs Python >= {'.'.join(map(str, REQUIRES_PYTHON))}, but "
        f"{sys.executable} is {sys.version.split()[0]}"
    )

try:
    from def_tools.dump_instances import main
except ModuleNotFoundError as exc:
    if not (exc.name or "").startswith("def_tools"):
        raise
    raise _die(f"{DISTRIBUTION} is not importable from {sys.executable}") from None


if __name__ == "__main__":
    raise SystemExit(main())
