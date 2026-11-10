"""Shared implementation for ``def-tools instances`` (and offline bare-Python runs)."""

from __future__ import annotations

import argparse
import glob
import gzip
import os
import re
import resource
import sys
import time
from collections.abc import Sequence
from pathlib import Path

from liberty_tools._native import (
    write_instance_bbox_csv,
    write_instance_parquet,
    write_instance_table,
)

FORMATS = ("csv", "parquet", "table")
CANONICAL_COLUMNS = ("instance", "base_cell", "llx", "lly", "urx", "ury")
AVAILABLE_COLUMNS = (
    "instance",
    "base_cell",
    "status",
    "orient",
    "llx",
    "lly",
    "urx",
    "ury",
)

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
        raise ValueError(f"invalid size: {text!r}")
    value, unit = match.groups()
    return int(float(value) * _SIZE_UNITS[unit.lower()])


def parse_column_spec(text: str) -> list[tuple[str, str]]:
    """Parse ``source[,source=output...]`` into ordered column pairs."""
    pairs: list[tuple[str, str]] = []
    for item in text.split(","):
        item = item.strip()
        if not item:
            raise ValueError("empty column entry")
        source, separator, output = item.partition("=")
        source = source.strip()
        output = output.strip() if separator else source
        if source not in AVAILABLE_COLUMNS:
            raise ValueError(
                f"unknown column {source!r}; choose from {', '.join(AVAILABLE_COLUMNS)}"
            )
        if not output:
            raise ValueError(f"empty output name for {source!r}")
        pairs.append((source, output))
    outputs = [output for _, output in pairs]
    if len(set(outputs)) != len(outputs):
        raise ValueError("duplicate output column name")
    return pairs


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


def expand_lef_patterns(patterns: Sequence[str]) -> list[str]:
    """Expand LEF paths and glob patterns in order, deduplicating existing files."""
    if not patterns:
        raise ValueError("requires at least one path or glob pattern")
    expanded: list[str] = []
    seen_existing: set[str] = set()
    for pattern in patterns:
        if not pattern:
            raise ValueError("requires at least one path or glob pattern")
        if glob.has_magic(pattern):
            matches = sorted(glob.glob(pattern, recursive=True))
            if not matches:
                raise ValueError(f"no files match pattern {pattern!r}")
        else:
            matches = [pattern]
        for candidate in matches:
            path = Path(candidate)
            if path.exists():
                resolved = os.path.realpath(path)
                if resolved in seen_existing:
                    continue
                seen_existing.add(resolved)
            expanded.append(candidate)
    return expanded


def expand_lef_argv(args: Sequence[str]) -> list[str]:
    """Normalize variadic ``--lef`` arguments and expand glob patterns into ``--lef <path>``."""
    expanded: list[str] = []
    seen_existing: set[str] = set()
    index = 0
    total = len(args)

    def emit_raw(patterns: Sequence[str]) -> None:
        for candidate in expand_lef_patterns(patterns):
            path = Path(candidate)
            if path.exists():
                resolved = os.path.realpath(path)
                if resolved in seen_existing:
                    continue
                seen_existing.add(resolved)
            expanded.extend(("--lef", candidate))

    while index < total:
        token = args[index]
        if token == "--":
            expanded.extend(args[index:])
            break
        if token == "--lef":
            index += 1
            values: list[str] = []
            while index < total and not args[index].startswith("-"):
                values.append(args[index])
                index += 1
            emit_raw(values)
            continue
        if token.startswith("--lef="):
            value = token.split("=", 1)[1]
            emit_raw([value])
            index += 1
            continue
        expanded.append(token)
        index += 1
    return expanded


def report_instances(
    fmt: str,
    columns: Sequence[tuple[str, str]] | None,
    output: Path,
    written: int,
    dbu_per_micron: int,
    elapsed: float,
    max_memory: int,
    peak_kib: int,
    parquet_info: tuple[int, int] | None = None,
) -> None:
    print(f"instances: {written:,}")
    print(f"dbu/micron: {dbu_per_micron:,}")
    print(f"format: {fmt}")
    selected = columns or [(name, name) for name in CANONICAL_COLUMNS]
    print("columns: " + ", ".join(out_name for _, out_name in selected))
    rate = written / elapsed / 1e6 if elapsed > 0 else 0.0
    print(f"wall: {elapsed:.2f} s ({rate:.2f} M instances/s)")
    print(f"memory limit: {max_memory / 1024**3:.2f} GiB (soft)")
    print(f"peak RSS: {peak_kib / 1024:.1f} MiB")
    if parquet_info is not None:
        group_size, row_groups = parquet_info
        print(f"parquet group size: {group_size:,} rows")
        print(
            f"parquet rows: {written:,} in {row_groups:,} row groups "
            f"(at most {group_size:,} rows per group)"
        )
    print(f"output: {output} ({output.stat().st_size / 1e6:.1f} MB)")


def run_instances(
    lefs: Sequence[Path],
    def_path: Path,
    fmt: str = "csv",
    output: Path | None = None,
    columns: Sequence[tuple[str, str]] | None = None,
    batch_size: int = 131_072,
    max_memory: int = 1024**3,
    threads: int | None = None,
) -> int:
    """Execute the instance dump and return exit code (0 on success, 1 on error)."""
    if threads is not None:
        os.environ["RAYON_NUM_THREADS"] = str(threads)
    if fmt == "parquet" and output is None:
        output = Path("instances.parquet")

    output_existed = output.exists() if output is not None else False
    if rss_bytes() > memory_ceiling(max_memory):
        print(
            f"error: process already uses {rss_bytes() / 1024**2:.0f} MiB, above the "
            f"{max_memory / 1024**2:.0f} MiB --max-memory limit "
            f"(+{int(_MEMORY_TOLERANCE * 100)}% tolerance)",
            file=sys.stderr,
        )
        return 1

    started = time.perf_counter()
    parquet_info: tuple[int, int] | None = None
    lef_list = [Path(p) for p in lefs]
    try:
        dbu_per_micron, expected = read_header(def_path)
        if fmt == "table":
            written = write_instance_table(lef_list, def_path, output, columns=columns)
        elif fmt == "csv":
            written = write_instance_bbox_csv(lef_list, def_path, output, columns=columns)
        else:
            assert output is not None
            eff_batch = effective_batch_size(max_memory, batch_size)
            written, row_groups = write_instance_parquet(
                lef_list, def_path, output, eff_batch, columns=columns
            )
            parquet_info = (eff_batch, row_groups)
    except (OSError, ValueError, RuntimeError) as exc:
        print(f"error: {exc}", file=sys.stderr)
        if output is not None and not output_existed:
            output.unlink(missing_ok=True)
        return 1

    elapsed = time.perf_counter() - started
    peak_kib = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss
    if written > expected:
        print(
            f"error: parsed {written} instances but the DEF header says {expected}",
            file=sys.stderr,
        )
        if output is not None:
            output.unlink(missing_ok=True)
        return 1
    if peak_rss_bytes() > memory_ceiling(max_memory):
        print(
            f"error: peak RSS reached {peak_rss_bytes() / 1024**2:.0f} MiB, above the "
            f"{max_memory / 1024**2:.0f} MiB --max-memory limit "
            f"(+{int(_MEMORY_TOLERANCE * 100)}% tolerance)",
            file=sys.stderr,
        )
        if output is not None:
            output.unlink(missing_ok=True)
        return 1

    if output is not None:
        report_instances(
            fmt,
            columns,
            output,
            written,
            dbu_per_micron,
            elapsed,
            max_memory,
            peak_kib,
            parquet_info,
        )
    return 0


def main(argv: Sequence[str] | None = None) -> int:
    """Stdlib CLI entry point used by ``scripts/run_example_offline.sh``."""
    raw = list(sys.argv[1:] if argv is None else argv)
    try:
        normalized = expand_lef_argv(raw)
    except ValueError as exc:
        print(f"error: --lef: {exc}", file=sys.stderr)
        return 2

    parser = argparse.ArgumentParser(prog="def-tools instances")
    parser.add_argument("--lef", type=Path, action="append", required=True)
    parser.add_argument("--def", dest="def_path", type=Path, required=True)
    parser.add_argument("--format", dest="fmt", choices=FORMATS, default="csv")
    parser.add_argument("--output", type=Path, default=None)
    parser.add_argument("--columns", type=parse_column_spec, default=None)
    parser.add_argument("--batch-size", type=int, default=131_072)
    parser.add_argument("--max-memory", type=parse_size, default=1024**3)
    parser.add_argument("--threads", type=int, default=None)
    args = parser.parse_args(normalized)
    if args.batch_size < 1:
        parser.error("--batch-size must be at least 1")
    return run_instances(
        args.lef,
        args.def_path,
        fmt=args.fmt,
        output=args.output,
        columns=args.columns,
        batch_size=args.batch_size,
        max_memory=args.max_memory,
        threads=args.threads,
    )


if __name__ == "__main__":
    raise SystemExit(main())
