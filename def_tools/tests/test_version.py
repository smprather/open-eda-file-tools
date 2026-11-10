from __future__ import annotations

import importlib.metadata

import pytest
from click.testing import CliRunner

from def_tools import version
from def_tools.cli import main


def test_get_version_reads_installed_metadata(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(version.metadata, "version", lambda _: "2026.11.5")

    assert version.get_version() == "2026.11.5"


def test_get_version_falls_back_when_not_installed(monkeypatch: pytest.MonkeyPatch) -> None:
    def missing(name: str) -> str:
        raise importlib.metadata.PackageNotFoundError(name)

    monkeypatch.setattr(version.metadata, "version", missing)

    assert version.get_version() == "0.0.0"


def test_cli_version_flag() -> None:
    result = CliRunner().invoke(main, ["--version"])

    assert result.exit_code == 0, result.output
    assert result.output.startswith("def-tools, version ")
