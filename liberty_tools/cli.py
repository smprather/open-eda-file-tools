"""``liberty-tools`` command group.

``liberty-tools format`` is the strict, functionally-transparent Liberty
formatter; the group is the single entry point for Liberty command-line tools.
"""

from __future__ import annotations

import gzip
import sys
from importlib.metadata import PackageNotFoundError, version
from pathlib import Path

import click

from liberty_format import TransparencyError, format_text


def _package_version() -> str:
    try:
        return version("open-eda-file-tools")
    except PackageNotFoundError:
        return "0.0.0+unknown"


def _read(path: Path) -> str:
    if path.suffix == ".gz":
        with gzip.open(path, "rt", encoding="utf-8") as handle:
            return handle.read()
    return path.read_text(encoding="utf-8")


@click.group(context_settings={"help_option_names": ["-h", "--help"]})
@click.version_option(version=_package_version(), prog_name="liberty-tools")
def main() -> None:
    """Liberty (.lib) parsing and formatting tools."""


@main.command(name="format")
@click.argument("file", type=click.Path(exists=True, dir_okay=False, path_type=Path))
@click.option(
    "-o",
    "--output",
    type=click.Path(dir_okay=False, path_type=Path),
    help="Write formatted output here (default: stdout).",
)
@click.option(
    "-i",
    "--in-place",
    is_flag=True,
    help="Rewrite the input file in place.",
)
@click.option(
    "--check",
    is_flag=True,
    help="Don't write; exit non-zero if the file is not already formatted.",
)
@click.option(
    "--indent",
    type=int,
    default=2,
    show_default=True,
    help="Spaces per brace-nesting level.",
)
def format_command(
    file: Path, output: Path | None, in_place: bool, check: bool, indent: int
) -> None:
    """Reindent a Liberty (.lib or .lib.gz) file by brace depth.

    Strictly applies one indent level per brace nesting, trims trailing
    whitespace, and collapses blank-line runs. The result is guaranteed
    functionally transparent: it is accepted only if it lexes to the exact
    same token stream as the input (otherwise the file is left untouched).
    """
    text = _read(file)
    try:
        formatted = format_text(text, indent=" " * indent)
    except TransparencyError as exc:
        raise click.ClickException(f"{file}: {exc}") from exc

    if check:
        if formatted == text:
            click.secho(f"{file}: already formatted", fg="green")
            return
        click.secho(f"{file}: would reformat", fg="yellow")
        sys.exit(1)

    if in_place:
        if file.suffix == ".gz":
            raise click.ClickException("--in-place not supported for .gz; use -o")
        if formatted == text:
            click.secho(f"{file}: unchanged", fg="green")
        else:
            file.write_text(formatted, encoding="utf-8")
            click.secho(f"{file}: formatted", fg="cyan")
    elif output:
        output.write_text(formatted, encoding="utf-8")
        click.secho(f"wrote {output}", fg="cyan")
    else:
        click.echo(formatted, nl=False)


if __name__ == "__main__":
    main()
