"""``pex-tools`` command line: SPEF/DSPF queries and summaries."""

from __future__ import annotations

from pathlib import Path

import rich_click as click

from . import header, histogram, summaries
from .version import get_version

MODE = click.Choice(["auto", "stream"], case_sensitive=False)


@click.group(context_settings={"help_option_names": ["-h", "--help"]})
@click.version_option(version=get_version(), prog_name="pex-tools")
def main() -> None:
    """SPEF and DSPF parasitic-exchange tools."""


@main.command(name="summary")
@click.option(
    "--spef",
    "spef_path",
    type=click.Path(exists=True, dir_okay=False, path_type=Path),
    required=True,
    help="SPEF or DSPF file (plain or .gz).",
)
@click.option("--mode", type=MODE, default="auto", show_default=True)
def summary(spef_path: Path, mode: str) -> None:
    """Print the header and aggregate parasitics."""
    info = header(spef_path)
    nets = summaries(spef_path, mode=mode)
    total = sum(net.total_cap for net in nets)
    ground = sum(net.ground_cap for net in nets)
    coupling = sum(net.coupling_cap for net in nets)
    resistors = sum(net.res_count for net in nets)
    click.echo(f"design: {info.design}")
    click.echo(f"standard: {info.standard}")
    click.echo(
        f"units: T={info.t_unit} C={info.c_unit} R={info.r_unit} L={info.l_unit}"
    )
    click.echo(f"nets: {len(nets)}")
    click.echo(f"total_cap: {total:.9g}")
    click.echo(f"ground_cap: {ground:.9g}")
    click.echo(f"coupling_cap: {coupling:.9g}")
    click.echo(f"resistors: {resistors}")


@main.command(name="histogram")
@click.option(
    "--spef",
    "spef_path",
    type=click.Path(exists=True, dir_okay=False, path_type=Path),
    required=True,
    help="SPEF or DSPF file (plain or .gz).",
)
@click.option("--bins", type=click.IntRange(min=1), default=20, show_default=True)
@click.option("--mode", type=MODE, default="auto", show_default=True)
@click.option(
    "--output",
    type=click.Path(dir_okay=False, path_type=Path),
    help="Write TSV here instead of stdout.",
)
def histogram_command(
    spef_path: Path, bins: int, mode: str, output: Path | None
) -> None:
    """Histogram of per-net total capacitance."""
    hist = histogram(spef_path, bins=bins, mode=mode)
    lines = ["bin_low\tbin_high\tcount"]
    for index, count in enumerate(hist.counts):
        lines.append(f"{hist.bounds[index]:.9g}\t{hist.bounds[index + 1]:.9g}\t{count}")
    rendered = "\n".join(lines) + "\n"
    if output is None:
        click.echo(rendered, nl=False)
    else:
        output.write_text(rendered, encoding="utf-8")


@main.command(name="nets")
@click.option(
    "--spef",
    "spef_path",
    type=click.Path(exists=True, dir_okay=False, path_type=Path),
    required=True,
    help="SPEF or DSPF file (plain or .gz).",
)
@click.option("--top", type=click.IntRange(min=1), default=20, show_default=True)
@click.option("--mode", type=MODE, default="auto", show_default=True)
@click.option(
    "--output",
    type=click.Path(dir_okay=False, path_type=Path),
    help="Write TSV here instead of stdout.",
)
def nets_command(spef_path: Path, top: int, mode: str, output: Path | None) -> None:
    """Largest nets by total capacitance."""
    ranked = sorted(summaries(spef_path, mode=mode), key=lambda net: -net.total_cap)
    lines = ["net\ttotal_cap\tground_cap\tcoupling_cap\tcaps\tresistors"]
    for net in ranked[:top]:
        lines.append(
            f"{net.name}\t{net.total_cap:.9g}\t{net.ground_cap:.9g}"
            f"\t{net.coupling_cap:.9g}\t{net.cap_count}\t{net.res_count}"
        )
    rendered = "\n".join(lines) + "\n"
    if output is None:
        click.echo(rendered, nl=False)
    else:
        output.write_text(rendered, encoding="utf-8")


if __name__ == "__main__":
    main()
