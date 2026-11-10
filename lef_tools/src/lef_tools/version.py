"""Version reporting for the ``lef-tools`` CLI.

``__version__`` is the installed distribution version. CalVer releases are cut
on the release commit (``YYYY.M.D``, with ``YYYY.M.D.N`` for a same-day
follow-up) and ``main`` keeps the released version until the next release, so
the installed metadata is authoritative and there is no git-derived fallback.
"""

from __future__ import annotations

from importlib import metadata

DIST_NAME = "lef-tools"


def get_version() -> str:
    """Return the installed distribution version."""
    try:
        return metadata.version(DIST_NAME)
    except metadata.PackageNotFoundError:
        return "0.0.0"
