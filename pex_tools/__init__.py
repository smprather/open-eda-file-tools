"""SPEF / DSPF parasitic extraction tools.

The Rust parser reads the file once, indexes ``*D_NET`` blocks, and parses
them in parallel (Rayon). ``mode="stream"`` buffers one net block at a time for
a much smaller memory footprint at the cost of speed.
"""

from __future__ import annotations

from os import PathLike

from liberty_tools._native import (
    PexCap,
    PexConn,
    PexFile,
    PexHeader,
    PexHistogram,
    PexNet,
    PexRes,
    PexSummary,
    parse_pex,
    pex_header,
    pex_histogram,
    pex_summaries,
)
from .version import get_version

__version__ = get_version()

_StrPath = str | PathLike[str]


def parse(
    path: _StrPath, keep_entries: bool = True, mode: str = "auto"
) -> PexFile:
    """Parse a SPEF/DSPF file (plain or ``.gz``) into header and per-net RC data.

    Args:
        path: Path to ``.spef``, ``.spef.gz``, ``.dspf``, or ``.dspf.gz``.
        keep_entries: When ``True`` (default), populates ``conns``, ``caps``,
            and ``res`` on each :class:`PexNet`. When ``False``, retains only
            per-net capacitance/resistance totals.
        mode: ``"auto"`` (parallel Rayon over net blocks) or ``"stream"``
            (low-memory single-block buffering; requires ``keep_entries=False``).
    """
    return parse_pex(path, keep_entries=keep_entries, mode=mode)


def summaries(path: _StrPath, mode: str = "auto") -> list[PexSummary]:
    """Return per-net capacitance/resistance totals without materializing elements.

    Args:
        path: Path to ``.spef``, ``.spef.gz``, ``.dspf``, or ``.dspf.gz``.
        mode: ``"auto"`` (parallel whole-file parse) or ``"stream"`` (low-memory
            single-block buffering).
    """
    return pex_summaries(path, mode=mode)


def histogram(
    path: _StrPath, bins: int = 20, mode: str = "auto"
) -> PexHistogram:
    """Compute a histogram of per-net total capacitance across all electrical nets."""
    return pex_histogram(path, bins=bins, mode=mode)


def header(path: _StrPath) -> PexHeader:
    """Parse only the SPEF/DSPF header and ``*PORTS`` without scanning nets."""
    return pex_header(path)

__all__ = [
    "PexCap",
    "PexConn",
    "PexFile",
    "PexHeader",
    "PexHistogram",
    "PexNet",
    "PexRes",
    "PexSummary",
    "__version__",
    "get_version",
    "header",
    "histogram",
    "parse",
    "summaries",
]
