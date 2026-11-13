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


def test_liberty_format_wrapper_version(capsys):
    from liberty_tools.cli import format_main

    format_main(["--version"])

    assert (
        capsys.readouterr().out
        == f"liberty-format, version {version('open-eda-file-tools')}\n"
    )


def test_liberty_format_wrapper_formats(tmp_path, capsys):
    import pytest

    from liberty_tools.cli import format_main

    path = tmp_path / "test.lib"
    path.write_text(LIB, encoding="utf-8")

    with pytest.raises(SystemExit) as excinfo:
        format_main([str(path)])

    assert excinfo.value.code == 0
    assert capsys.readouterr().out == format_text(LIB)


def test_liberty_format_wrapper_check_exits_one(tmp_path, capsys):
    import pytest

    from liberty_tools.cli import format_main

    path = tmp_path / "test.lib"
    path.write_text(LIB, encoding="utf-8")

    with pytest.raises(SystemExit) as excinfo:
        format_main(["--check", str(path)])

    assert excinfo.value.code == 1
    assert "would reformat" in capsys.readouterr().out


def test_docs_cover_every_cli_entry_point():
    from pathlib import Path
    import tomllib

    repo = Path(__file__).resolve().parent.parent
    scripts = tomllib.loads((repo / "pyproject.toml").read_text(encoding="utf-8"))[
        "project"
    ]["scripts"]
    ai_api = (repo / "docs" / "ai-api.md").read_text(encoding="utf-8")
    cli_doc = (repo / "docs" / "cli.md").read_text(encoding="utf-8")

    for script_name in scripts:
        assert script_name in ai_api, f"{script_name} missing from docs/ai-api.md"
        assert script_name in cli_doc, f"{script_name} missing from docs/cli.md"
