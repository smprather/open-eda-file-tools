"""``liberty-tools`` command group.

``liberty-tools format`` is the strict, functionally-transparent Liberty
formatter; the group is the single entry point for Liberty command-line tools.
"""

from __future__ import annotations

import shutil
import sys
import tempfile
from collections.abc import Iterator
from importlib.metadata import PackageNotFoundError, version
from pathlib import Path
from typing import TextIO

import click

from liberty_format import TransparencyError, _reindent_lines, _verify_tokens

from . import _native
from ._text_input import open_text


def _package_version() -> str:
    try:
        return version("open-eda-file-tools")
    except PackageNotFoundError:
        return "0.0.0+unknown"


def _copy_lines(source: TextIO, copy: TextIO) -> Iterator[str]:
    for line in source:
        copy.write(line)
        yield line


def _stage_format(path: Path, before: Path, after: Path, indent: str) -> bool:
    # Text mode matches the CLI's UTF-8 and universal-newline input rules.
    with (
        open_text(path) as (source, compressed),
        before.open("w", encoding="utf-8", newline="\n") as normalized,
        after.open("w", encoding="utf-8", newline="\n") as formatted,
    ):
        formatted.writelines(_reindent_lines(_copy_lines(source, normalized), indent))
    _verify_tokens(_native._compare_token_files(before, after))
    return compressed


def _same_contents(before: Path, after: Path) -> bool:
    if before.stat().st_size != after.stat().st_size:
        return False
    with before.open("rb") as original, after.open("rb") as formatted:
        while chunk := original.read(65536):
            if chunk != formatted.read(65536):
                return False
    return True


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
    with tempfile.TemporaryDirectory(prefix="liberty-format-") as scratch:
        before = Path(scratch) / "input.lib"
        after = Path(scratch) / "formatted.lib"
        try:
            compressed = _stage_format(file, before, after, " " * indent)
        except TransparencyError as exc:
            raise click.ClickException(f"{file}: {exc}") from exc

        if check:
            if _same_contents(before, after):
                click.secho(f"{file}: already formatted", fg="green")
                return
            click.secho(f"{file}: would reformat", fg="yellow")
            sys.exit(1)

        if in_place:
            if compressed:
                raise click.ClickException("--in-place not supported for gzip input; use -o")
            if _same_contents(before, after):
                click.secho(f"{file}: unchanged", fg="green")
                return
            output = file

        if output is None:
            with after.open(encoding="utf-8", newline="\n") as formatted:
                while lines := formatted.readlines(65536):
                    click.echo("".join(lines), nl=False)
            return

        with (
            after.open(encoding="utf-8", newline="\n") as formatted,
            output.open("w", encoding="utf-8") as destination,
        ):
            shutil.copyfileobj(formatted, destination, length=65536)
        if in_place:
            click.secho(f"{file}: formatted", fg="cyan")
        else:
            click.secho(f"wrote {output}", fg="cyan")


def format_main(argv: list[str] | None = None) -> None:
    """``liberty-format`` console script: invoke the same command as
    ``liberty-tools format``.

    This is a thin entry point, not a second implementation: it runs the
    group's ``format`` command object directly so help, options and behaviour
    cannot drift.
    """
    args = list(sys.argv[1:] if argv is None else argv)
    if "--version" in args:
        click.echo(f"liberty-format, version {_package_version()}")
        return
    format_command.main(
        args=args,
        prog_name="liberty-format",
        standalone_mode=True,
    )


if __name__ == "__main__":
    main()
