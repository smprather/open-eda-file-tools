#!/usr/bin/env python3
"""Verify that vendor/cargo is complete, intact, and tracked.

`cargo` validates every vendored crate against its `.cargo-checksum.json` and
aborts on the first missing or altered file, so a tree that was `git add`ed
while an ignore rule was active fails every from-source build. Run this after a
`cargo vendor` refresh (and in CI) to catch that class of breakage.

A checksum check alone cannot catch the original bug: the missing files were
present on disk but silently untracked. When the tree is inside a git checkout,
this also verifies that every manifest file is tracked.
"""

from __future__ import annotations

import hashlib
import json
import subprocess
import sys
from pathlib import Path

REPO_DIR = Path(__file__).resolve().parent.parent
VENDOR_DIR = REPO_DIR / "vendor" / "cargo"


def tracked_vendor_files() -> set[str] | None:
    """Paths under vendor/cargo tracked by git, or None outside a checkout."""
    try:
        result = subprocess.run(
            ["git", "-C", str(REPO_DIR), "ls-files", "-z", "--", "vendor/cargo"],
            capture_output=True,
            check=True,
        )
    except (OSError, subprocess.CalledProcessError):
        return None
    return {entry.decode() for entry in result.stdout.split(b"\0") if entry}


def main() -> int:
    manifests = sorted(VENDOR_DIR.glob("*/.cargo-checksum.json"))
    if not manifests:
        print(f"no vendored crates found under {VENDOR_DIR}", file=sys.stderr)
        return 1

    tracked = tracked_vendor_files()
    if tracked is None:
        print(
            "note: not a git checkout; skipping the tracked-file check",
            file=sys.stderr,
        )

    missing = 0
    mismatched = 0
    untracked = 0
    checked = 0
    for manifest in manifests:
        crate_dir = manifest.parent
        files: dict[str, str] = json.loads(manifest.read_text())["files"]
        for relative, expected in files.items():
            path = crate_dir / relative
            if tracked is not None:
                repo_relative = path.relative_to(REPO_DIR).as_posix()
                if repo_relative not in tracked:
                    print(f"{crate_dir.name}: not tracked by git: {relative}")
                    untracked += 1
            if not path.is_file():
                print(f"{crate_dir.name}: missing {relative}")
                missing += 1
                continue
            digest = hashlib.sha256(path.read_bytes()).hexdigest()
            if digest != expected:
                print(f"{crate_dir.name}: checksum mismatch {relative}")
                mismatched += 1
        if tracked is not None:
            manifest_relative = manifest.relative_to(REPO_DIR).as_posix()
            if manifest_relative not in tracked:
                print(f"{crate_dir.name}: manifest not tracked by git")
                untracked += 1
        checked += len(files)

    if missing or mismatched or untracked:
        print(
            f"FAIL: {missing} missing, {mismatched} mismatched, "
            f"{untracked} untracked files"
        )
        return 1

    note = "" if tracked is not None else " (tracked-file check skipped)"
    print(f"ok: {len(manifests)} crates, {checked} files present and unchanged{note}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
