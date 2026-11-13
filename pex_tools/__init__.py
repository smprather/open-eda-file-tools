"""SPEF / DSPF parasitic extraction tools.

The Rust parser reads the file once, indexes ``*D_NET`` blocks, and parses
them in parallel (Rayon). ``mode="stream"`` buffers one net block at a time for
a much smaller memory footprint at the cost of speed.
"""

from __future__ import annotations

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


def parse(path, keep_entries: bool = True, mode: str = "auto") -> PexFile:
    """Parse a SPEF/DSPF file into header and net data."""
    return parse_pex(path, keep_entries=keep_entries, mode=mode)


def summaries(path, mode: str = "auto") -> list[PexSummary]:
    """Per-net totals only; much smaller than full entries."""
    return pex_summaries(path, mode=mode)


def histogram(path, bins: int = 20, mode: str = "auto") -> PexHistogram:
    """Histogram of per-net total capacitance."""
    return pex_histogram(path, bins=bins, mode=mode)


def header(path) -> PexHeader:
    """The file header, without parsing nets."""
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
