from __future__ import annotations

import gzip
import stat
from importlib.metadata import version

import pytest
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


@pytest.mark.parametrize("compressed", [False, True])
def test_format_cli_normalizes_newlines_and_writes_plain_utf8(tmp_path, compressed):
    path = tmp_path / ("input.lib.gz" if compressed else "input.lib")
    output = tmp_path / "formatted.lib.gz"
    source = LIB.replace("area", "/* μ */ area").replace("\n", "\r\n").encode()
    path.write_bytes(gzip.compress(source) if compressed else source)
    result = CliRunner().invoke(liberty_main, ["format", str(path), "-o", str(output)])
    assert result.exit_code == 0, result.output
    assert output.read_bytes() == (
        "library (t) {\n  cell (A) {\n    /* μ */ area : 1;\n  }\n}\n".encode()
    )


def test_format_cli_follows_symlink_and_preserves_mode(tmp_path):
    target = tmp_path / "target.lib"
    target.write_text(LIB, encoding="utf-8")
    target.chmod(0o640)
    link = tmp_path / "link.lib"
    link.symlink_to(target)

    result = CliRunner().invoke(liberty_main, ["format", "--in-place", str(link)])

    assert result.exit_code == 0, result.output
    assert link.is_symlink()
    assert target.read_text(encoding="utf-8") == "library (t) {\n  cell (A) {\n    area : 1;\n  }\n}\n"
    assert stat.S_IMODE(target.stat().st_mode) == 0o640


@pytest.mark.parametrize("destination", ["stdout", "output", "in-place"])
def test_format_cli_failed_verification_leaves_destination_untouched(tmp_path, destination):
    # Leading Unicode whitespace is a word to the lexer and is removed by
    # reindentation. The token verification must prevent it being emitted.
    path = tmp_path / "input.lib"
    source = LIB.replace("area", "\u2002area")
    path.write_text(source, encoding="utf-8")
    output = tmp_path / "output.lib"
    output.write_bytes(b"sentinel")
    args = ["format", str(path)]
    if destination == "output":
        args.extend(["-o", str(output)])
    elif destination == "in-place":
        args.append("--in-place")

    result = CliRunner().invoke(liberty_main, args)

    assert result.exit_code == 1
    assert "formatting changed the token stream" in result.stderr
    assert result.stdout == ""
    assert output.read_bytes() == b"sentinel"
    assert path.read_text(encoding="utf-8") == source


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
