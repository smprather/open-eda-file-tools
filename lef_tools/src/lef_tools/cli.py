from __future__ import annotations

from pathlib import Path

import click

from . import parse_lef
from .version import get_version


@click.group(context_settings={"help_option_names": ["-h", "--help"]})
@click.version_option(version=get_version(), prog_name="lef-tools")
def main() -> None:
    """Fast cell LEF query tools."""


@main.command()
@click.option(
    "--lef",
    "lefs",
    type=click.Path(exists=True, dir_okay=False, path_type=Path),
    multiple=True,
    required=True,
    help="Cell LEF library. May be provided more than once.",
)
@click.option(
    "--macro",
    "macro_names",
    multiple=True,
    help="Only load these macros. Repeat to name more than one.",
)
def macros(lefs: tuple[Path, ...], macro_names: tuple[str, ...]) -> None:
    """List macros and their micron dimensions as TSV."""
    library = parse_lef(list(lefs), macros=list(macro_names) or None)
    click.echo("macro\twidth_um\theight_um\torigin_x_um\torigin_y_um")
    for macro in library.macros():
        click.echo(
            f"{macro.name}\t{macro.width_microns:g}\t{macro.height_microns:g}"
            f"\t{macro.origin_x_microns:g}\t{macro.origin_y_microns:g}"
        )


@main.command()
@click.option(
    "--lef",
    "lefs",
    type=click.Path(exists=True, dir_okay=False, path_type=Path),
    multiple=True,
    required=True,
    help="Cell LEF library. May be provided more than once.",
)
@click.option("--macro", "macro_name", required=True, help="Macro to print.")
def show(lefs: tuple[Path, ...], macro_name: str) -> None:
    """Print one macro's dimensions, pins, ports and rectangles."""
    library = parse_lef(list(lefs), macros=[macro_name])
    macro = library.macro(macro_name)
    click.echo(
        f"{macro.name}: {macro.width_microns:g} x {macro.height_microns:g} um, "
        f"origin ({macro.origin_x_microns:g}, {macro.origin_y_microns:g})"
    )
    for pin in macro.pins():
        click.echo(f"  pin {pin.name} direction={pin.direction or '-'} use={pin.use or '-'}")
        for index, port in enumerate(pin.ports(), start=1):
            click.echo(
                f"    port {index} unsupported_geometry={port.has_unsupported_geometry}"
            )
            for rect in port.rects():
                click.echo(
                    f"      {rect.layer} {rect.x0_microns:g} {rect.y0_microns:g} "
                    f"{rect.x1_microns:g} {rect.y1_microns:g}"
                )
