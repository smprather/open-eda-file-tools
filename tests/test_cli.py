from __future__ import annotations

import importlib.metadata
from importlib.metadata import version

import pytest
from click.testing import CliRunner

from def_tools import version as def_version
from def_tools.cli import main as def_main
from lef_tools import version as lef_version
from lef_tools.cli import main as lef_main
from liberty_format.__main__ import main as format_main
from viewer.__main__ import main as view_main


def test_liberty_format_version_option() -> None:
    result = CliRunner().invoke(format_main, ["--version"])

    assert result.exit_code == 0
    assert result.output == f"liberty-format {version('open-eda-file-tools')}\n"


def test_liberty_view_version_option() -> None:
    result = CliRunner().invoke(view_main, ["--version"])

    assert result.exit_code == 0
    assert result.output == f"liberty-view {version('open-eda-file-tools')}\n"


def test_lef_tools_version_option() -> None:
    result = CliRunner().invoke(lef_main, ["--version"])

    assert result.exit_code == 0
    assert result.output == f"lef-tools, version {version('open-eda-file-tools')}\n"


def test_def_tools_version_option() -> None:
    result = CliRunner().invoke(def_main, ["--version"])

    assert result.exit_code == 0
    assert result.output == f"def-tools, version {version('open-eda-file-tools')}\n"


def test_get_version_reads_installed_metadata(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(def_version.metadata, "version", lambda _: "2026.11.9")
    monkeypatch.setattr(lef_version.metadata, "version", lambda _: "2026.11.9")

    assert def_version.get_version() == "2026.11.9"
    assert lef_version.get_version() == "2026.11.9"


def test_get_version_falls_back_when_not_installed(monkeypatch: pytest.MonkeyPatch) -> None:
    def missing(name: str) -> str:
        raise importlib.metadata.PackageNotFoundError(name)

    monkeypatch.setattr(def_version.metadata, "version", missing)
    monkeypatch.setattr(lef_version.metadata, "version", missing)

    assert def_version.get_version() == "0.0.0"
    assert lef_version.get_version() == "0.0.0"
