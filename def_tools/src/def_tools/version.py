from __future__ import annotations

import re
import subprocess
from importlib import metadata
from pathlib import Path

DIST_NAME = "def-tools"
# CalVer tags: vYYYY.M.MICRO (e.g. v2026.9.0, no zero-padding: cargo semver
# and PEP 440 both reject/strip leading zeros). Pre-CalVer semver tags are
# deliberately not matched.
_VERSION_RE = re.compile(r"^v?(\d+)\.(\d+)\.(\d+)(.*)$")


def get_version() -> str:
    """Return installed version, or a git-derived version in a standalone clone.

    Only the tool's own project directory may supply the git metadata. Inside
    the shared open-eda-file-tools repository the enclosing git root belongs to
    the monorepo, so its tags and commit count must not leak into this
    distribution's ``__version__``.
    """
    package = _package_version()
    root = _git_root(_project_root(Path(__file__).resolve()))
    if root is None:
        return package
    return _git_version(root, package) or package


def _project_root(file: Path) -> Path:
    """The directory holding this tool's ``pyproject.toml`` and ``Cargo.toml``."""
    return file.parents[2]


def _git_root(project_root: Path) -> Path | None:
    return project_root if (project_root / ".git").exists() else None


def _package_version() -> str:
    try:
        return metadata.version(DIST_NAME)
    except metadata.PackageNotFoundError:
        return "0.0.0"


def _git_version(root: Path, package_version: str) -> str | None:
    description = _git(
        root,
        "describe",
        "--tags",
        "--long",
        "--dirty",
        "--match",
        "v[0-9][0-9][0-9][0-9]*",
    )
    if description:
        parsed = _version_from_describe(description)
        if parsed:
            return parsed
    return _version_from_hash(root, package_version)


def _version_from_describe(description: str) -> str | None:
    dirty = description.endswith("-dirty")
    if dirty:
        description = description.removesuffix("-dirty")

    parts = description.rsplit("-", 2)
    if len(parts) != 3:
        return None
    tag, distance_text, hash_text = parts
    if not distance_text.isdigit() or not hash_text.startswith("g"):
        return None

    base = _strip_v(tag)
    distance = int(distance_text)
    version = base if distance == 0 else f"{_bump_micro(base)}.dev{distance}+{hash_text}"
    return _append_dirty(version) if dirty else version


def _version_from_hash(root: Path, package_version: str) -> str | None:
    commit = _git(root, "rev-parse", "--short", "HEAD")
    count = _git(root, "rev-list", "--count", "HEAD")
    if not commit or not count or not count.isdigit():
        return None
    version = f"{_bump_micro(package_version)}.dev{count}+g{commit}"
    dirty = _git(root, "status", "--porcelain", "--untracked-files=no")
    return _append_dirty(version) if dirty else version


def _strip_v(version: str) -> str:
    return version[1:] if version.startswith("v") else version


def _bump_micro(version: str) -> str:
    match = _VERSION_RE.match(version)
    if match is None:
        return version
    major, minor, patch, suffix = match.groups()
    if suffix:
        return _strip_v(version)
    return f"{major}.{minor}.{int(patch) + 1}"


def _append_dirty(version: str) -> str:
    return f"{version}.dirty" if "+" in version else f"{version}+dirty"


def _git(root: Path, *args: str) -> str:
    try:
        proc = subprocess.run(
            ["git", "-C", str(root), *args],
            check=False,
            capture_output=True,
            text=True,
            timeout=5,
        )
    except OSError, subprocess.TimeoutExpired:
        return ""
    if proc.returncode != 0:
        return ""
    return proc.stdout.strip()
