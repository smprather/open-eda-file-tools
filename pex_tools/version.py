"""Version reporting for the ``pex-tools`` CLI."""

from __future__ import annotations

from importlib import metadata

DIST_NAME = "open-eda-file-tools"


def get_version() -> str:
    """Return the installed distribution version."""
    try:
        return metadata.version(DIST_NAME)
    except metadata.PackageNotFoundError:
        return "0.0.0"
