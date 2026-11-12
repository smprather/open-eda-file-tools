from __future__ import annotations

from importlib.metadata import version

from click.testing import CliRunner

from liberty_format import format_text
from liberty_tools.cli import main as liberty_main
from viewer.__main__ import main as view_main

LIB = """\
library (t) {
cell (A) {
area : 1;
}
}
"""


def test_liberty_tools_version_option():
    result = CliRunner().invoke(liberty_main, ["--version"])

    assert result.exit_code == 0
    assert result.output == f"liberty-tools, version {version('open-eda-file-tools')}\n"


def test_liberty_view_version_option():
    result = CliRunner().invoke(view_main, ["--version"])

    assert result.exit_code == 0
    assert result.output == f"liberty-view {version('open-eda-file-tools')}\n"


def test_format_subcommand_formats_to_stdout(tmp_path):
    path = tmp_path / "test.lib"
    path.write_text(LIB, encoding="utf-8")

    result = CliRunner().invoke(liberty_main, ["format", str(path)])

    assert result.exit_code == 0, result.output
    assert result.output == format_text(LIB)


def test_format_subcommand_check_reports_unformatted(tmp_path):
    path = tmp_path / "test.lib"
    path.write_text(LIB, encoding="utf-8")

    result = CliRunner().invoke(liberty_main, ["format", "--check", str(path)])

    assert result.exit_code == 1
    assert "would reformat" in result.output
