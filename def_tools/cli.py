from __future__ import annotations

import os
from pathlib import Path
from typing import Any

import rich_click as click

from liberty_tools._native import (
    write_clock_instances_csv,
    write_clock_instances_csv_from_cache,
    write_def_cache,
    write_net_shapes_csv,
    write_net_shapes_csv_from_cache,
)

from .instances import (
    AVAILABLE_COLUMNS,
    CANONICAL_COLUMNS,
    FORMATS,
    effective_batch_size,
    expand_lef_argv,
    memory_ceiling,
    parse_column_spec,
    parse_size,
    peak_rss_bytes,
    read_header,
    rss_bytes,
    run_instances,
)
from .io_config import DEFAULT_PROBE_MIB, write_io_config
from .sdc import clock_roots_from_sdc
from .version import get_version

__all__ = [
    "AVAILABLE_COLUMNS",
    "CANONICAL_COLUMNS",
    "FORMATS",
    "effective_batch_size",
    "expand_lef_argv",
    "main",
    "memory_ceiling",
    "parse_column_spec",
    "parse_size",
    "peak_rss_bytes",
    "read_header",
    "rss_bytes",
]


class _SizeParam(click.ParamType):
    name = "size"

    def convert(
        self,
        value: Any,
        param: click.Parameter | None,
        ctx: click.Context | None,
    ) -> int:
        if isinstance(value, int):
            return value
        try:
            return parse_size(str(value))
        except ValueError as exc:
            self.fail(str(exc), param, ctx)


class _ColumnSpecParam(click.ParamType):
    name = "spec"

    def convert(
        self,
        value: Any,
        param: click.Parameter | None,
        ctx: click.Context | None,
    ) -> list[tuple[str, str]]:
        if isinstance(value, list):
            return value
        try:
            return parse_column_spec(str(value))
        except ValueError as exc:
            self.fail(str(exc), param, ctx)


class _VariadicLefCommand(click.RichCommand):
    def parse_args(self, ctx: click.Context, args: list[str]) -> list[str]:
        try:
            args[:] = expand_lef_argv(args)
        except ValueError as exc:
            raise click.BadParameter(str(exc), ctx=ctx, param_hint="'--lef'") from exc
        return super().parse_args(ctx, args)


@click.group(context_settings={"help_option_names": ["-h", "--help"]})
@click.version_option(version=get_version(), prog_name="def-tools")
def main() -> None:
    """Fast DEF query tools."""


@main.command(cls=_VariadicLefCommand)
@click.option(
    "--lef",
    "lefs",
    type=click.Path(dir_okay=False, path_type=Path),
    multiple=True,
    required=True,
    help="Cell LEF file(s) or glob pattern(s). Accepts multiple arguments after --lef.",
)
@click.option(
    "--def",
    "def_path",
    type=click.Path(dir_okay=False, path_type=Path),
    required=True,
    help="DEF design file (.def or .def.gz).",
)
@click.option(
    "--format",
    "fmt",
    type=click.Choice(FORMATS),
    default="csv",
    show_default=True,
    help="Output format (coordinates in microns).",
)
@click.option(
    "--output",
    type=click.Path(dir_okay=False, path_type=Path),
    default=None,
    help="Output path (defaults to stdout for csv/table, or instances.parquet for parquet).",
)
@click.option(
    "--columns",
    type=_ColumnSpecParam(),
    default=None,
    metavar="SPEC",
    help=(
        "Comma-separated columns in output order; rename with source=output, "
        "for example instance=instance_name,base_cell,llx=llx_um. "
        f"Sources: {','.join(AVAILABLE_COLUMNS)}. "
        f"Defaults to {','.join(CANONICAL_COLUMNS)}."
    ),
)
@click.option(
    "--batch-size",
    type=click.IntRange(min=1),
    default=131_072,
    show_default=True,
    help="Maximum rows buffered per Parquet row group.",
)
@click.option(
    "--max-memory",
    type=_SizeParam(),
    default=1024**3,
    show_default="1GiB",
    metavar="SIZE",
    help=(
        "Soft upper bound on process peak RSS, in binary units (1GB = 1 GiB); "
        "clamps the Parquet row group and aborts only after a 10% overshoot."
    ),
)
@click.option(
    "--threads",
    type=click.IntRange(min=1),
    help="Rayon worker threads for component parsing. Defaults to all logical CPUs.",
)
def instances(
    lefs: tuple[Path, ...],
    def_path: Path,
    fmt: str,
    output: Path | None,
    columns: list[tuple[str, str]] | None,
    batch_size: int,
    max_memory: int,
    threads: int | None,
) -> None:
    """Dump placed instance bounding boxes (in microns) as CSV, Parquet, or table."""
    rc = run_instances(
        lefs,
        def_path,
        fmt=fmt,
        output=output,
        columns=columns,
        batch_size=batch_size,
        max_memory=max_memory,
        threads=threads,
    )
    if rc != 0:
        raise SystemExit(rc)


@main.command(name="net-shapes")
@click.option(
    "--def",
    "def_path",
    type=click.Path(exists=True, dir_okay=False, path_type=Path),
    help="DEF design file. Plain .def and .def.gz are supported.",
)
@click.option(
    "--cache-dir",
    type=click.Path(exists=True, file_okay=False, path_type=Path),
    help="Binary DEF cache directory from `cache create`.",
)
@click.option("--net", required=True, help="Net name to query, for example VDD.")
@click.option("--layer", required=True, help="DEF layer name to query, for example met1.")
@click.option(
    "-o",
    "--output",
    type=click.Path(dir_okay=False, path_type=Path),
    help="CSV output path. Defaults to stdout.",
)
@click.option(
    "--threads",
    type=click.IntRange(min=1),
    help="Rayon worker threads for net record parsing. Defaults to all logical CPUs.",
)
def net_shapes(
    def_path: Path | None,
    cache_dir: Path | None,
    net: str,
    layer: str,
    output: Path | None,
    threads: int | None,
) -> None:
    """Write routed shape bboxes for one net/layer as CSV."""
    if (def_path is None) == (cache_dir is None):
        raise click.UsageError("provide exactly one of --def or --cache-dir")
    if threads is not None:
        os.environ["RAYON_NUM_THREADS"] = str(threads)
    if cache_dir is not None:
        write_net_shapes_csv_from_cache(cache_dir, net, layer, output)
    elif def_path is not None:
        write_net_shapes_csv(def_path, net, layer, output)


@main.group(name="cache")
def def_cache() -> None:
    """Create and use binary DEF cache directories."""


@def_cache.command(name="create")
@click.option(
    "--def",
    "def_path",
    type=click.Path(exists=True, dir_okay=False, path_type=Path),
    required=True,
    help="DEF design file. Plain .def and .def.gz are supported.",
)
@click.option(
    "-o",
    "--output",
    "cache_dir",
    type=click.Path(file_okay=False, path_type=Path),
    required=True,
    help="Output cache directory.",
)
def def_cache_create(def_path: Path, cache_dir: Path) -> None:
    """Convert a DEF into indexed binary cache files."""
    write_def_cache(def_path, cache_dir)


@main.command(name="clock-instances")
@click.option(
    "--sdc",
    "sdc_path",
    type=click.Path(exists=True, dir_okay=False, path_type=Path),
    required=True,
    help="SDC file with create_clock/create_generated_clock commands.",
)
@click.option(
    "--def",
    "def_path",
    type=click.Path(exists=True, dir_okay=False, path_type=Path),
    help="DEF design file. Plain .def and .def.gz are supported.",
)
@click.option(
    "--cache-dir",
    type=click.Path(exists=True, file_okay=False, path_type=Path),
    help="Binary DEF cache directory from `cache create`.",
)
@click.option(
    "--clock-cell-pattern",
    multiple=True,
    help="Substring identifying clock tree cells. Defaults to `clk`.",
)
@click.option(
    "-o",
    "--output",
    type=click.Path(dir_okay=False, path_type=Path),
    help="CSV output path. Defaults to stdout.",
)
@click.option(
    "--threads",
    type=click.IntRange(min=1),
    help="Rayon worker threads. Defaults to all logical CPUs.",
)
def clock_instances(
    sdc_path: Path,
    def_path: Path | None,
    cache_dir: Path | None,
    clock_cell_pattern: tuple[str, ...],
    output: Path | None,
    threads: int | None,
) -> None:
    """Write instances reached from SDC clock roots."""
    if (def_path is None) == (cache_dir is None):
        raise click.UsageError("provide exactly one of --def or --cache-dir")
    if threads is not None:
        os.environ["RAYON_NUM_THREADS"] = str(threads)
    roots = clock_roots_from_sdc(sdc_path)
    if not roots:
        raise click.ClickException(f"no create_clock roots found in {sdc_path}")
    patterns = list(clock_cell_pattern)
    if cache_dir is not None:
        write_clock_instances_csv_from_cache(cache_dir, roots, patterns, output)
    elif def_path is not None:
        write_clock_instances_csv(def_path, roots, patterns, output)


@main.command(name="configure-io")
@click.option(
    "--target",
    "targets",
    type=click.Path(file_okay=False, path_type=Path),
    multiple=True,
    required=True,
    help="Writable directory to benchmark. May be provided more than once.",
)
@click.option(
    "-o",
    "--output",
    type=click.Path(dir_okay=False, path_type=Path),
    default=Path("def-tools-io-config.toml"),
    show_default=True,
    help="Output TOML config path.",
)
@click.option(
    "--probe-mib",
    type=click.IntRange(min=1),
    default=DEFAULT_PROBE_MIB,
    show_default=True,
    help="Maximum benchmark file size per target.",
)
@click.option(
    "--engine",
    type=click.Choice(["auto", "internal", "fio"]),
    default="auto",
    show_default=True,
    help="Benchmark engine. Auto uses fio when available, otherwise internal.",
)
def configure_io(
    targets: tuple[Path, ...],
    output: Path,
    probe_mib: int,
    engine: str,
) -> None:
    """Benchmark storage targets and write optimized IO config."""
    results = write_io_config(list(targets), output, probe_mib=probe_mib, engine=engine)
    click.echo(f"wrote {output}")
    for result in results:
        status = "ok" if result.writable and not result.error else "skip"
        click.echo(
            f"{status} {result.path} kind={result.kind} fs={result.fs_type} "
            f"write={result.seq_write_mib_s:.1f}MiB/s read={result.seq_read_mib_s:.1f}MiB/s"
        )


if __name__ == "__main__":
    main()
