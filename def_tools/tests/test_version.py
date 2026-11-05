from __future__ import annotations

from click.testing import CliRunner

from def_tools.cli import main
from def_tools.version import _git_root, _project_root, _version_from_describe


def test_project_root_is_above_the_src_tree(tmp_path):
    source = tmp_path / "src" / "def_tools"
    source.mkdir(parents=True)

    assert _project_root(source / "version.py") == tmp_path


def test_git_root_requires_repository_at_the_project_root(tmp_path):
    project = tmp_path / "def_tools"
    project.mkdir()

    assert _git_root(project) is None

    (project / ".git").mkdir()
    assert _git_root(project) == project


def test_version_from_exact_tag():
    assert _version_from_describe("v1.2.3-0-gabc1234") == "1.2.3"


def test_version_from_commits_after_tag():
    assert _version_from_describe("v1.2.3-4-gabc1234") == "1.2.4.dev4+gabc1234"


def test_version_from_dirty_commit_after_tag():
    assert _version_from_describe("v1.2.3-4-gabc1234-dirty") == "1.2.4.dev4+gabc1234.dirty"


def test_version_from_calver_tag():
    assert _version_from_describe("v2026.9.0-0-gabc1234") == "2026.9.0"


def test_version_from_commits_after_calver_tag():
    assert _version_from_describe("v2026.9.0-4-gabc1234") == "2026.9.1.dev4+gabc1234"


def test_cli_version_flag():
    result = CliRunner().invoke(main, ["--version"])

    assert result.exit_code == 0, result.output
    assert result.output.startswith("def-tools, version ")
