#!/usr/bin/env python3
"""Dump DEF instances as Parquet (default), CSV, or an aligned table.

Columns: instance name, base cell, and the bounding box in microns
(``llx, lly, urx, ury``). Microns are DEF database units divided by the
design's ``UNITS DISTANCE MICRONS`` value.

All three formats are written by the native extension. Parquet buffers
``--batch-size`` rows per row group; CSV and the aligned table stream.

``--max-memory`` (default 1 GiB) bounds the Parquet batch and aborts the run if
the process peak RSS exceeds it by more than 10% (a soft cap). Size suffixes
are binary: ``1GB`` means ``1 GiB``.

    uv run python examples/dump_instances.py \
        --lef merged.min.lef --def 17-fill.def.gz

    uv run python scripts/dump_instances.py --format csv \
        --lef merged.min.lef --def 17-fill.def.gz --output instances.csv

    uv run python scripts/dump_instances.py --format table \
        --lef merged.min.lef --def 17-fill.def.gz --output instances.table
"""

from __future__ import annotations

import argparse
import gzip
import re
import resource
import sys
import time
from collections.abc import Sequence
from pathlib import Path

from def_tools import (
    write_instance_bbox_csv,
    write_instance_parquet,
    write_instance_table,
)

FORMATS = ("parquet", "csv", "table")

# Conservative sizing for --max-memory. The native Parquet batch costs much
# less than this per row; 4 KiB/row leaves ample room for the Arrow arrays.
_ROW_BYTES_ESTIMATE = 4096
_FIXED_OVERHEAD_BYTES = 64 * 1024**2
_MIN_ROWS_PER_GROUP = 4096
# The limit is a budget, not a contract: overshooting by a few percent is fine.
_MEMORY_TOLERANCE = 0.10

_SIZE_RE = re.compile(r"^\s*([0-9]+(?:\.[0-9]+)?)\s*([kmgt]?i?b?)\s*$", re.IGNORECASE)
_SIZE_UNITS = {
    "": 1,
    "b": 1,
    "k": 1024,
    "kb": 1024,
    "kib": 1024,
    "m": 1024**2,
    "mb": 1024**2,
    "mib": 1024**2,
    "g": 1024**3,
    "gb": 1024**3,
    "gib": 1024**3,
    "t": 1024**4,
    "tb": 1024**4,
    "tib": 1024**4,
}


def parse_size(text: str) -> int:
    match = _SIZE_RE.match(text)
    if match is None:
        raise argparse.ArgumentTypeError(f"invalid size: {text!r}")
    value, unit = match.groups()
    return int(float(value) * _SIZE_UNITS[unit.lower()])


def rss_bytes() -> int:
    """Current resident set size, in bytes."""
    try:
        with open("/proc/self/status", encoding="ascii") as handle:
            for line in handle:
                if line.startswith("VmRSS:"):
                    return int(line.split()[1]) * 1024
    except OSError:
        pass
    return peak_rss_bytes()


def peak_rss_bytes() -> int:
    """Peak resident set size since process start, in bytes (Linux: KiB)."""
    return resource.getrusage(resource.RUSAGE_SELF).ru_maxrss * 1024


def memory_ceiling(max_bytes: int) -> int:
    """The abort threshold: the budget plus a little slack."""
    return int(max_bytes * (1.0 + _MEMORY_TOLERANCE))


def effective_batch_size(max_bytes: int, requested: int) -> int:
    """Clamp the requested Parquet row group to the memory budget."""
    room = max_bytes - rss_bytes() - _FIXED_OVERHEAD_BYTES
    cap = max(_MIN_ROWS_PER_GROUP, room // _ROW_BYTES_ESTIMATE)
    return min(requested, cap)


def read_header(path: Path) -> tuple[int, int]:
    """Return ``(dbu_per_micron, components)`` from a plain or gzipped DEF."""
    opener = gzip.open if path.suffix.lower() == ".gz" else open
    dbu: int | None = None
    components: int | None = None
    with opener(path, "rt", encoding="utf-8", errors="replace") as handle:
        for line in handle:
            tokens = line.split()
            if not tokens:
                continue
            head = tokens[0]
            if head == "UNITS":
                if len(tokens) >= 4 and tokens[1] == "DISTANCE" and tokens[2] == "MICRONS":
                    dbu = int(tokens[3])
            elif head == "COMPONENTS" and len(tokens) >= 2 and tokens[1].isdigit():
                components = int(tokens[1])
            if dbu is not None and components is not None:
                break
    if dbu is None or components is None:
        raise ValueError(f"{path}: missing UNITS DISTANCE MICRONS or COMPONENTS header")
    return dbu, components


def parse_args(argv: Sequence[str] | None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument(
        "--lef",
        type=Path,
        action="append",
        required=True,
        help="Cell LEF defining the placed macros. Repeat for several files.",
    )
    parser.add_argument("--def", dest="design", type=Path, required=True, help="DEF design.")
    parser.add_argument(
        "--format",
        choices=FORMATS,
        default="parquet",
        help="Output format (default: parquet).",
    )
    parser.add_argument(
        "--output",
        type=Path,
        default=None,
        help="Output path (default: instances.<format>).",
    )
    parser.add_argument(
        "--batch-size",
        type=int,
        default=131_072,
        help="Maximum rows buffered per Parquet row group (default: 131072).",
    )
    parser.add_argument(
        "--max-memory",
        type=parse_size,
        default=1024**3,
        metavar="SIZE",
        help=(
            "Soft upper bound on process peak RSS, in binary units (default: 1GB = 1 GiB); "
            "clamps the Parquet row group and aborts only after a 10%% overshoot."
        ),
    )
    args = parser.parse_args(argv)

    if args.output is None:
        args.output = Path(f"instances.{args.format}")
    if args.batch_size < 1:
        parser.error("--batch-size must be at least 1")
    return args


def report(
    args: argparse.Namespace,
    written: int,
    dbu_per_micron: int,
    elapsed: float,
    peak_kib: int,
    parquet_info: tuple[int, int] | None = None,
) -> None:
    print(f"instances: {written:,}")
    print(f"dbu/micron: {dbu_per_micron:,}")
    print(f"format: {args.format}")
    print(f"wall: {elapsed:.2f} s ({written / elapsed / 1e6:.2f} M instances/s)")
    print(f"memory limit: {args.max_memory / 1024**3:.2f} GiB (soft)")
    print(f"peak RSS: {peak_kib / 1024:.1f} MiB")
    if parquet_info is not None:
        group_size, row_groups = parquet_info
        print(f"parquet group size: {group_size:,} rows")
        print(
            f"parquet rows: {written:,} in {row_groups:,} row groups "
            f"(at most {group_size:,} rows per group)"
        )
    print(f"output: {args.output} ({args.output.stat().st_size / 1e6:.1f} MB)")


def main(argv: Sequence[str] | None = None) -> int:
    args = parse_args(argv)
    dbu_per_micron, expected = read_header(args.design)
    lefs = [str(path) for path in args.lef]

    if rss_bytes() > memory_ceiling(args.max_memory):
        print(
            f"error: process already uses {rss_bytes() / 1024**2:.0f} MiB, above the "
            f"{args.max_memory / 1024**2:.0f} MiB --max-memory limit "
            f"(+{int(_MEMORY_TOLERANCE * 100)}% tolerance)",
            file=sys.stderr,
        )
        return 1

    started = time.perf_counter()
    parquet_info: tuple[int, int] | None = None
    try:
        if args.format == "table":
            written = write_instance_table(lefs, args.design, args.output)
        elif args.format == "csv":
            written = write_instance_bbox_csv(lefs, args.design, args.output)
        else:
            batch_size = effective_batch_size(args.max_memory, args.batch_size)
            written, row_groups = write_instance_parquet(
                lefs, args.design, args.output, batch_size
            )
            parquet_info = (batch_size, row_groups)
    except (OSError, ValueError, RuntimeError) as exc:
        print(f"error: {exc}", file=sys.stderr)
        args.output.unlink(missing_ok=True)
        return 1

    elapsed = time.perf_counter() - started
    peak_kib = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss
    if written != expected:
        print(
            f"error: parsed {written} instances but the DEF header says {expected}",
            file=sys.stderr,
        )
        args.output.unlink(missing_ok=True)
        return 1
    if peak_rss_bytes() > memory_ceiling(args.max_memory):
        print(
            f"error: peak RSS reached {peak_rss_bytes() / 1024**2:.0f} MiB, above the "
            f"{args.max_memory / 1024**2:.0f} MiB --max-memory limit "
            f"(+{int(_MEMORY_TOLERANCE * 100)}% tolerance)",
            file=sys.stderr,
        )
        args.output.unlink(missing_ok=True)
        return 1

    report(args, written, dbu_per_micron, elapsed, peak_kib, parquet_info)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
