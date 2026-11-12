"""``sdc-tools`` command line."""

from __future__ import annotations

import json
from dataclasses import asdict
from pathlib import Path

import rich_click as click

from .clock_tree import clock_cells as find_clock_cells
from .clocks import Clock, clocks
from .version import get_version


@click.group(context_settings={"help_option_names": ["-h", "--help"]})
@click.version_option(version=get_version(), prog_name="sdc-tools")
def main() -> None:
    """Synopsys Design Constraints query tools."""


def _render(clocks_found: list[Clock], fmt: str) -> str:
    if fmt == "json":
        return json.dumps([asdict(clock) for clock in clocks_found], indent=2) + "\n"
    lines = ["clock\tperiod\twaveform\tsources\tgenerated_from\tdivide_by\tmultiply_by"]
    for clock in clocks_found:
        lines.append(
            "\t".join(
                [
                    clock.name,
                    "" if clock.period is None else f"{clock.period:g}",
                    " ".join(f"{value:g}" for value in clock.waveform),
                    " ".join(clock.sources),
                    clock.generated_from or "",
                    "" if clock.divide_by is None else str(clock.divide_by),
                    "" if clock.multiply_by is None else str(clock.multiply_by),
                ]
            )
        )
    return "\n".join(lines) + "\n"


@main.command(name="clocks")
@click.option(
    "--sdc",
    "sdc_path",
    type=click.Path(exists=True, dir_okay=False, path_type=Path),
    required=True,
    help="SDC file to evaluate.",
)
@click.option(
    "--format",
    "fmt",
    type=click.Choice(["tsv", "json"]),
    default="tsv",
    show_default=True,
    help="Output format.",
)
@click.option(
    "--output",
    type=click.Path(dir_okay=False, path_type=Path),
    help="Write here instead of stdout.",
)
def clocks_command(sdc_path: Path, fmt: str, output: Path | None) -> None:
    """List the clocks created by an SDC."""
    rendered = _render(clocks(sdc_path), fmt)
    if output is None:
        click.echo(rendered, nl=False)
    else:
        output.write_text(rendered, encoding="utf-8")


@main.command(name="clock-cells")
@click.option(
    "--sdc",
    "sdc_path",
    type=click.Path(exists=True, dir_okay=False, path_type=Path),
    required=True,
    help="SDC file that creates the clock roots.",
)
@click.option(
    "--def",
    "def_path",
    type=click.Path(exists=True, dir_okay=False, path_type=Path),
    required=True,
    help="DEF design with the instance connectivity.",
)
@click.option(
    "--lib",
    "libs",
    type=click.Path(exists=True, dir_okay=False, path_type=Path),
    multiple=True,
    required=True,
    help="Liberty library for pin directions and sequential types.",
)
@click.option(
    "--format",
    "fmt",
    type=click.Choice(["lines", "json"]),
    default="lines",
    show_default=True,
    help="One instance per line, or a JSON list.",
)
@click.option(
    "--output",
    type=click.Path(dir_okay=False, path_type=Path),
    help="Write here instead of stdout.",
)
def clock_cells_command(
    sdc_path: Path, def_path: Path, libs: tuple[Path, ...], fmt: str, output: Path | None
) -> None:
    """List the clock-tree instances reachable from the SDC clock roots."""
    result = find_clock_cells(def_path, list(libs), sdc_path)
    if fmt == "json":
        rendered = json.dumps(list(result.cells), indent=2) + "\n"
    else:
        rendered = "\n".join(result.cells)
        rendered = rendered + "\n" if rendered else ""
    if output is None:
        click.echo(rendered, nl=False)
    else:
        output.write_text(rendered, encoding="utf-8")


if __name__ == "__main__":
    main()
